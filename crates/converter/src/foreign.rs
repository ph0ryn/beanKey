use std::borrow::Cow;
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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
    Io {
        path: PathBuf,
        source: io::Error,
    },
    MissingEncoding(PathBuf),
    UnsupportedEncoding(String),
    InvalidEncoding {
        path: PathBuf,
        encoding: &'static str,
    },
    InvalidDictionary {
        affix: PathBuf,
        dictionary: PathBuf,
        message: String,
    },
    WorkerStopped,
}

impl fmt::Display for HunspellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::MissingEncoding(path) => write!(
                formatter,
                "Hunspell affix file {} has no SET encoding directive",
                path.display()
            ),
            Self::UnsupportedEncoding(encoding) => {
                write!(
                    formatter,
                    "unsupported Hunspell dictionary encoding {encoding}"
                )
            }
            Self::InvalidEncoding { path, encoding } => write!(
                formatter,
                "Hunspell dictionary file {} contains invalid {encoding} data",
                path.display()
            ),
            Self::InvalidDictionary {
                affix,
                dictionary,
                message,
            } => write!(
                formatter,
                "failed to parse Hunspell dictionary {} with {}: {message}",
                dictionary.display(),
                affix.display()
            ),
            Self::WorkerStopped => write!(formatter, "Hunspell completion worker stopped"),
        }
    }
}

impl Error for HunspellError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

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
                    let suggestions = dictionary.suggestions(&request.input);
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

struct HunspellDictionary(spellbook::Dictionary);

impl HunspellDictionary {
    fn open(base: impl AsRef<Path>) -> Result<Self, HunspellError> {
        let affix = base.as_ref().with_extension("aff");
        let dictionary = base.as_ref().with_extension("dic");
        let affix_bytes = read_dictionary_file(&affix)?;
        let label = affix_bytes
            .split(|byte| *byte == b'\n')
            .find_map(|line| {
                let mut fields = line
                    .split(u8::is_ascii_whitespace)
                    .filter(|field| !field.is_empty());
                (fields.next()? == b"SET").then(|| fields.next()).flatten()
            })
            .ok_or_else(|| HunspellError::MissingEncoding(affix.clone()))?;
        let encoding = Encoding::for_label(label).ok_or_else(|| {
            HunspellError::UnsupportedEncoding(String::from_utf8_lossy(label).into_owned())
        })?;
        let affix_text = decode_dictionary_file(&affix, &affix_bytes, encoding)?;
        let dictionary_bytes = read_dictionary_file(&dictionary)?;
        let dictionary_text = decode_dictionary_file(&dictionary, &dictionary_bytes, encoding)?;
        let invalid_dictionary = |message: String| HunspellError::InvalidDictionary {
            affix: affix.clone(),
            dictionary: dictionary.clone(),
            message,
        };
        let mut lines = dictionary_text.lines();
        if lines
            .next()
            .and_then(|line| line.trim().parse::<usize>().ok())
            .is_none()
        {
            return Err(invalid_dictionary(
                "dictionary has no entry count header".into(),
            ));
        }
        // Spellbook treats the entry count as a capacity hint and accepts empty dictionaries.
        // An empty or header-only resource is not a usable completion dictionary.
        if !lines.any(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        }) {
            return Err(invalid_dictionary("dictionary has no entries".into()));
        }
        let dictionary = spellbook::Dictionary::new(&affix_text, &dictionary_text)
            .map_err(|error| invalid_dictionary(error.to_string()))?;
        Ok(Self(dictionary))
    }

    fn suggestions(&self, word: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        // N-gram suggestions scan the dictionary and are too costly for interactive completion.
        self.0
            .suggester()
            .with_ngram_suggestions(false)
            .suggest(word, &mut suggestions);
        suggestions
    }
}

fn read_dictionary_file(path: &Path) -> Result<Vec<u8>, HunspellError> {
    fs::read(path).map_err(|source| HunspellError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn decode_dictionary_file<'a>(
    path: &Path,
    bytes: &'a [u8],
    encoding: &'static Encoding,
) -> Result<Cow<'a, str>, HunspellError> {
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    if had_errors {
        return Err(HunspellError::InvalidEncoding {
            path: path.to_path_buf(),
            encoding: encoding.name(),
        });
    }
    Ok(text)
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
