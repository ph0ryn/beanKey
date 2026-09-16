use std::collections::HashSet;
use std::error::Error;
use std::ffi::{CStr, CString, c_char};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc;

use encoding_rs::Encoding;

use crate::{Candidate, ComposingCount, ComposingText, DictionaryEntry, InputPiece};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyboardLanguage {
    #[default]
    None,
    Japanese,
    EnglishUs,
    Greek,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForeignLanguage {
    EnglishUs,
    Greek,
}

pub trait ForeignCompletionProvider: Send + Sync {
    fn completions(&self, language: ForeignLanguage, input: &str) -> Vec<String>;
}

#[derive(Debug)]
pub enum HunspellError {
    InvalidPath(PathBuf),
    CreateFailed { affix: PathBuf, dictionary: PathBuf },
    UnsupportedEncoding(String),
    WorkerStopped,
}

impl fmt::Display for HunspellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(
                formatter,
                "Hunspell path is not valid UTF-8 or contains a null byte: {}",
                path.display()
            ),
            Self::CreateFailed { affix, dictionary } => write!(
                formatter,
                "failed to load Hunspell dictionary {} with {}",
                dictionary.display(),
                affix.display()
            ),
            Self::UnsupportedEncoding(encoding) => {
                write!(
                    formatter,
                    "unsupported Hunspell dictionary encoding {encoding}"
                )
            }
            Self::WorkerStopped => write!(formatter, "Hunspell completion worker stopped"),
        }
    }
}

impl Error for HunspellError {}

pub struct HunspellCompleter {
    requests: mpsc::Sender<CompletionRequest>,
}

impl HunspellCompleter {
    pub fn open(
        english_us_base: impl AsRef<Path>,
        greek_base: impl AsRef<Path>,
    ) -> Result<Self, HunspellError> {
        let english_us_base = english_us_base.as_ref().to_path_buf();
        let greek_base = greek_base.as_ref().to_path_buf();
        let (requests, receiver) = mpsc::channel::<CompletionRequest>();
        let (ready, readiness) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("beankey-hunspell".into())
            .spawn(move || {
                let dictionaries = (|| {
                    Ok::<_, HunspellError>((
                        HunspellDictionary::open(english_us_base)?,
                        HunspellDictionary::open(greek_base)?,
                    ))
                })();
                let (english_us, greek) = match dictionaries {
                    Ok(dictionaries) => dictionaries,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(request) = receiver.recv() {
                    let dictionary = match request.language {
                        ForeignLanguage::EnglishUs => &english_us,
                        ForeignLanguage::Greek => &greek,
                    };
                    let suggestions = dictionary.suggestions(&request.input).unwrap_or_default();
                    let _ = request.response.send(suggestions);
                }
            })
            .map_err(|_| HunspellError::WorkerStopped)?;
        readiness
            .recv()
            .map_err(|_| HunspellError::WorkerStopped)??;
        Ok(Self { requests })
    }
}

impl ForeignCompletionProvider for HunspellCompleter {
    fn completions(&self, language: ForeignLanguage, input: &str) -> Vec<String> {
        let (response, receiver) = mpsc::sync_channel(1);
        if self
            .requests
            .send(CompletionRequest {
                language,
                input: input.to_owned(),
                response,
            })
            .is_err()
        {
            return Vec::new();
        }
        receiver
            .recv()
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| starts_with_ignoring_case(candidate, input))
            .collect()
    }
}

struct CompletionRequest {
    language: ForeignLanguage,
    input: String,
    response: mpsc::SyncSender<Vec<String>>,
}

struct HunspellDictionary {
    handle: HunspellHandle,
    encoding: &'static Encoding,
}

impl HunspellDictionary {
    fn open(base: impl AsRef<Path>) -> Result<Self, HunspellError> {
        let affix = base.as_ref().with_extension("aff");
        let dictionary = base.as_ref().with_extension("dic");
        let affix_c = path_to_c_string(&affix)?;
        let dictionary_c = path_to_c_string(&dictionary)?;
        // SAFETY: Both strings remain alive for the call and Hunspell copies or reads the files
        // during construction. A null return is handled below.
        let handle = unsafe { Hunspell_create(affix_c.as_ptr(), dictionary_c.as_ptr()) };
        let handle =
            NonNull::new(handle).ok_or(HunspellError::CreateFailed { affix, dictionary })?;
        let handle = HunspellHandle(handle);
        // SAFETY: The handle is valid and Hunspell owns the returned null-terminated string for
        // the lifetime of the handle.
        let encoding = unsafe { Hunspell_get_dic_encoding(handle.0.as_ptr()) };
        let encoding = if encoding.is_null() {
            return Err(HunspellError::UnsupportedEncoding("<null>".into()));
        } else {
            // SAFETY: Hunspell_get_dic_encoding returned a non-null null-terminated string.
            unsafe { CStr::from_ptr(encoding) }.to_string_lossy()
        };
        let encoding = Encoding::for_label(encoding.as_bytes())
            .ok_or_else(|| HunspellError::UnsupportedEncoding(encoding.into_owned()))?;
        Ok(Self { handle, encoding })
    }

    fn suggestions(&self, word: &str) -> Result<Vec<String>, ()> {
        let (word, _, had_encoding_errors) = self.encoding.encode(word);
        if had_encoding_errors {
            return Err(());
        }
        let word = CString::new(word.as_ref()).map_err(|_| ())?;
        let mut list: *mut *mut c_char = std::ptr::null_mut();
        // SAFETY: The valid handle remains on its owning worker thread and Hunspell initializes
        // list according to its C API contract.
        let raw_count =
            unsafe { Hunspell_suggest(self.handle.0.as_ptr(), &mut list, word.as_ptr()) };
        if raw_count <= 0 || list.is_null() {
            return Ok(Vec::new());
        }
        let count = usize::try_from(raw_count).map_err(|_| ())?;
        // SAFETY: Hunspell returned count initialized pointers and keeps them valid until
        // Hunspell_free_list is called below.
        let pointers = unsafe { std::slice::from_raw_parts(list, count) };
        let mut output = Vec::with_capacity(count);
        for &pointer in pointers {
            if pointer.is_null() {
                continue;
            }
            // SAFETY: Each non-null item returned by Hunspell is a null-terminated string.
            let value = unsafe { CStr::from_ptr(pointer) }.to_bytes();
            let (value, had_decoding_errors) = self.encoding.decode_without_bom_handling(value);
            if !had_decoding_errors {
                output.push(value.into_owned());
            }
        }
        // SAFETY: list and count are the unmodified values returned by Hunspell_suggest for
        // this handle, and the Rust strings above own their contents.
        unsafe {
            Hunspell_free_list(self.handle.0.as_ptr(), &mut list, raw_count);
        }
        Ok(output)
    }
}

struct HunspellHandle(NonNull<Hunhandle>);

impl Drop for HunspellHandle {
    fn drop(&mut self) {
        // SAFETY: The pointer was returned by Hunspell_create and is destroyed exactly once.
        unsafe { Hunspell_destroy(self.0.as_ptr()) };
    }
}

#[repr(C)]
struct Hunhandle {
    _private: [u8; 0],
}

#[link(name = "hunspell-1.7")]
unsafe extern "C" {
    fn Hunspell_create(affix: *const c_char, dictionary: *const c_char) -> *mut Hunhandle;
    fn Hunspell_destroy(handle: *mut Hunhandle);
    fn Hunspell_get_dic_encoding(handle: *mut Hunhandle) -> *mut c_char;
    fn Hunspell_suggest(
        handle: *mut Hunhandle,
        suggestions: *mut *mut *mut c_char,
        word: *const c_char,
    ) -> i32;
    fn Hunspell_free_list(handle: *mut Hunhandle, suggestions: *mut *mut *mut c_char, count: i32);
}

pub(crate) fn foreign_predictions(
    composing: &ComposingText,
    keyboard_language: KeyboardLanguage,
    provider: &dyn ForeignCompletionProvider,
    penalty: f32,
) -> Vec<Candidate> {
    let input = composing
        .input()
        .iter()
        .filter_map(|element| match &element.piece {
            InputPiece::Character(value) => Some(value.as_str()),
            _ => None,
        })
        .collect::<String>();
    let mut output = if input
        .chars()
        .all(|character| character.is_ascii_alphabetic())
    {
        predictions_for_language(
            composing,
            &input,
            ForeignLanguage::EnglishUs,
            provider,
            penalty,
        )
    } else {
        Vec::new()
    };
    if keyboard_language == KeyboardLanguage::Greek {
        output.extend(predictions_for_language(
            composing,
            &input,
            ForeignLanguage::Greek,
            provider,
            penalty,
        ));
    }
    unique(output)
}

fn predictions_for_language(
    composing: &ComposingText,
    input: &str,
    language: ForeignLanguage,
    provider: &dyn ForeignCompletionProvider,
    penalty: f32,
) -> Vec<Candidate> {
    if input.is_empty() {
        return Vec::new();
    }
    let completions = provider.completions(language, input);
    if completions.is_empty() {
        return Vec::new();
    }
    let mut output = Vec::with_capacity(completions.len() + 1);
    output.push(foreign_candidate(
        input,
        input,
        penalty,
        composing.input().len(),
    ));
    let delta = -10.0 / completions.len() as f32;
    let mut value = -5.0 + penalty;
    for completion in completions {
        output.push(foreign_candidate(
            &completion,
            &completion,
            value,
            composing.input().len(),
        ));
        value += delta;
    }
    output
}

fn foreign_candidate(word: &str, ruby: &str, value: f32, input_count: usize) -> Candidate {
    let entry = DictionaryEntry {
        word: word.to_owned(),
        ruby: ruby.to_owned(),
        left_id: 1288,
        right_id: 1288,
        meaning_id: 501,
        base_value: value,
        adjustment: 0.0,
        metadata: Default::default(),
    };
    Candidate::single(
        word.to_owned(),
        value,
        ComposingCount::Input(input_count),
        501,
        vec![entry],
    )
}

fn path_to_c_string(path: &Path) -> Result<CString, HunspellError> {
    path.to_str()
        .and_then(|path| CString::new(path).ok())
        .ok_or_else(|| HunspellError::InvalidPath(path.to_path_buf()))
}

fn starts_with_ignoring_case(candidate: &str, input: &str) -> bool {
    candidate.to_lowercase().starts_with(&input.to_lowercase())
}

fn unique(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.text.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputStyle, InputTableRegistry};

    struct FixedProvider;

    impl ForeignCompletionProvider for FixedProvider {
        fn completions(&self, language: ForeignLanguage, input: &str) -> Vec<String> {
            match (language, input) {
                (ForeignLanguage::EnglishUs, "hel") => vec!["hell".into(), "hello".into()],
                (ForeignLanguage::Greek, "καλ") => vec!["καλά".into(), "καλό".into()],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn builds_scored_english_and_greek_completion_candidates() {
        let tables = InputTableRegistry::new();
        let mut english = ComposingText::new();
        english.insert_str("hel", InputStyle::Direct, &tables);
        let predictions =
            foreign_predictions(&english, KeyboardLanguage::EnglishUs, &FixedProvider, -5.0);
        assert_eq!(
            predictions
                .iter()
                .map(|candidate| (candidate.text.as_str(), candidate.value))
                .collect::<Vec<_>>(),
            [("hel", -5.0), ("hell", -10.0), ("hello", -15.0)]
        );

        let mut greek = ComposingText::new();
        greek.insert_str("καλ", InputStyle::Direct, &tables);
        let predictions =
            foreign_predictions(&greek, KeyboardLanguage::Greek, &FixedProvider, -5.0);
        assert_eq!(
            predictions
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["καλ", "καλά", "καλό"]
        );
    }
}
