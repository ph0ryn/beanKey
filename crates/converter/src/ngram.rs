use std::error::Error;
use std::ffi::{CString, c_char, c_void};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use tokenizers::Tokenizer;

use crate::{TokenProbabilityModel, ZenzInferenceError, ZenzLanguageModel};

const VOCABULARY_SIZE: usize = 6_000;
const START_TOKEN: i32 = 2;
const END_TOKEN: i32 = 3;
const KEY_VALUE_DELIMITER: u8 = 128;
const PREDICTIVE_DELIMITER: u8 = 129;
const ENCODING_RADIX: usize = 126;

#[derive(Debug)]
pub enum NGramError {
    InvalidPath(PathBuf),
    InvalidConfiguration {
        n: usize,
        discount: f64,
    },
    LoadTrie(PathBuf),
    SearchTrie,
    InvalidTrieEntry,
    CountOverflow,
    Tokenizer(String),
    UnexpectedVocabularySize(usize),
    UnexpectedSpecialToken {
        token: &'static str,
        id: Option<u32>,
    },
}

impl fmt::Display for NGramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(
                formatter,
                "N-gram path is not valid UTF-8 or contains a null byte: {}",
                path.display()
            ),
            Self::InvalidConfiguration { n, discount } => {
                write!(
                    formatter,
                    "invalid N-gram configuration n={n}, d={discount}"
                )
            }
            Self::LoadTrie(path) => write!(formatter, "failed to load {}", path.display()),
            Self::SearchTrie => write!(formatter, "Marisa predictive search failed"),
            Self::InvalidTrieEntry => write!(formatter, "Marisa trie contains an invalid entry"),
            Self::CountOverflow => write!(formatter, "N-gram count exceeds UInt32"),
            Self::Tokenizer(message) => write!(formatter, "tokenizer error: {message}"),
            Self::UnexpectedVocabularySize(size) => {
                write!(formatter, "expected a 6000-token vocabulary, found {size}")
            }
            Self::UnexpectedSpecialToken { token, id } => {
                write!(
                    formatter,
                    "expected tokenizer token {token} at its fixed ID, found {id:?}"
                )
            }
        }
    }
}

impl Error for NGramError {}

pub struct ZenzTokenizer {
    tokenizer: Tokenizer,
}

impl ZenzTokenizer {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NGramError> {
        let tokenizer = Tokenizer::from_file(path.as_ref())
            .map_err(|error| NGramError::Tokenizer(error.to_string()))?;
        let vocabulary_size = tokenizer.get_vocab_size(true);
        if vocabulary_size != VOCABULARY_SIZE {
            return Err(NGramError::UnexpectedVocabularySize(vocabulary_size));
        }
        for (token, expected) in [("<s>", START_TOKEN), ("</s>", END_TOKEN)] {
            let actual = tokenizer.token_to_id(token);
            if actual != u32::try_from(expected).ok() {
                return Err(NGramError::UnexpectedSpecialToken { token, id: actual });
            }
        }
        Ok(Self { tokenizer })
    }

    pub fn encode(&self, text: &str) -> Result<Vec<i32>, NGramError> {
        self.tokenizer
            .encode(text, false)
            .map(|encoding| {
                encoding
                    .get_ids()
                    .iter()
                    .map(|token| i32::try_from(*token).expect("6000-token ID fits i32"))
                    .collect()
            })
            .map_err(|error| NGramError::Tokenizer(error.to_string()))
    }

    pub fn decode(&self, tokens: &[i32]) -> Result<String, NGramError> {
        let tokens: Option<Vec<u32>> = tokens
            .iter()
            .map(|token| u32::try_from(*token).ok())
            .collect();
        self.tokenizer
            .decode(
                &tokens.ok_or_else(|| NGramError::Tokenizer("negative token ID".into()))?,
                true,
            )
            .map_err(|error| NGramError::Tokenizer(error.to_string()))
    }

    pub fn vocabulary_size(&self) -> usize {
        VOCABULARY_SIZE
    }
}

pub struct EfficientNGram {
    n: usize,
    discount: f64,
    c_abc: MarisaTrie,
    u_abx: MarisaTrie,
    u_xbc: MarisaTrie,
    r_xbx: MarisaTrie,
}

impl EfficientNGram {
    pub fn open(prefix: impl AsRef<Path>, n: usize, discount: f64) -> Result<Self, NGramError> {
        if n < 2 || !discount.is_finite() {
            return Err(NGramError::InvalidConfiguration { n, discount });
        }
        let prefix = normalize_prefix(prefix.as_ref())?;
        Ok(Self {
            n,
            discount,
            c_abc: MarisaTrie::load(with_suffix(&prefix, "_c_abc.marisa"))?,
            u_abx: MarisaTrie::load(with_suffix(&prefix, "_u_abx.marisa"))?,
            u_xbc: MarisaTrie::load(with_suffix(&prefix, "_u_xbc.marisa"))?,
            r_xbx: MarisaTrie::load(with_suffix(&prefix, "_r_xbx.marisa"))?,
        })
    }

    pub fn n(&self) -> usize {
        self.n
    }

    pub fn discount(&self) -> f64 {
        self.discount
    }

    pub fn probabilities(&self, history: &[i32]) -> Result<Vec<f64>, NGramError> {
        let history: Option<Vec<usize>> = history
            .iter()
            .map(|token| {
                usize::try_from(*token)
                    .ok()
                    .filter(|token| *token < VOCABULARY_SIZE)
            })
            .collect();
        let history = history.ok_or(NGramError::InvalidTrieEntry)?;
        self.bulk_predict(&history)
    }

    fn bulk_predict(&self, history: &[usize]) -> Result<Vec<f64>, NGramError> {
        let context_size = self.n - 1;
        let mut context = if history.len() >= context_size {
            history[history.len() - context_size..].to_vec()
        } else {
            let mut value = vec![START_TOKEN as usize; context_size - history.len()];
            value.extend_from_slice(history);
            value
        };

        let unique_continuations = self.u_abx.get_value(&context)?.unwrap_or(0);
        let (counts, total_count) = self.c_abc.bulk_values(&context)?;
        let mut lower = Vec::with_capacity(context_size.saturating_sub(1));
        for _ in 1..context_size {
            context.remove(0);
            let distinct_predecessors = self.r_xbx.get_value(&context)?.unwrap_or(0);
            let (continuation_counts, continuation_total) = self.u_xbc.bulk_values(&context)?;
            lower.push((
                continuation_counts,
                continuation_total,
                distinct_predecessors,
            ));
        }

        let main_denominator = (total_count != 0).then_some(f64::from(total_count));
        let main_gamma = main_denominator.map_or(1.0, |denominator| {
            self.discount * f64::from(unique_continuations) / denominator
        });
        let prepared_lower: Vec<_> = lower
            .into_iter()
            .map(|(values, total, distinct)| {
                let denominator = (total != 0).then_some(f64::from(total));
                let gamma = denominator.map_or(1.0, |denominator| {
                    self.discount * f64::from(distinct) / denominator
                });
                (values, denominator, gamma)
            })
            .collect();

        let mut probabilities = Vec::with_capacity(VOCABULARY_SIZE);
        for token in 0..VOCABULARY_SIZE {
            let alpha = main_denominator.map_or(0.0, |denominator| {
                (f64::from(counts[token]) - self.discount).max(0.0) / denominator
            });
            let mut lower_probability = 0.0;
            let mut coefficient = 1.0;
            for (values, denominator, gamma) in &prepared_lower {
                let lower_alpha = denominator.map_or(0.0, |denominator| {
                    (f64::from(values[token]) - self.discount).max(0.0) / denominator
                });
                lower_probability += lower_alpha * coefficient;
                coefficient *= gamma;
            }
            lower_probability += coefficient / VOCABULARY_SIZE as f64;
            probabilities.push(alpha + main_gamma * lower_probability);
        }
        Ok(probabilities)
    }
}

impl TokenProbabilityModel for EfficientNGram {
    fn probabilities(&self, prefix: &[i32], vocabulary_size: usize) -> Option<Vec<f32>> {
        if vocabulary_size != VOCABULARY_SIZE {
            return None;
        }
        EfficientNGram::probabilities(self, prefix)
            .ok()
            .map(|values| values.into_iter().map(|value| value as f32).collect())
    }
}

pub struct NGramLanguageModel {
    model: EfficientNGram,
    tokenizer: ZenzTokenizer,
}

impl NGramLanguageModel {
    pub fn open(
        prefix: impl AsRef<Path>,
        tokenizer: impl AsRef<Path>,
        n: usize,
        discount: f64,
    ) -> Result<Self, NGramError> {
        Ok(Self {
            model: EfficientNGram::open(prefix, n, discount)?,
            tokenizer: ZenzTokenizer::open(tokenizer)?,
        })
    }

    pub fn model(&self) -> &EfficientNGram {
        &self.model
    }
}

impl ZenzLanguageModel for NGramLanguageModel {
    fn vocabulary_size(&self) -> usize {
        self.tokenizer.vocabulary_size()
    }

    fn eos_token(&self) -> i32 {
        END_TOKEN
    }

    fn tokenize(&mut self, text: &str, _add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        self.tokenizer
            .encode(text)
            .map_err(|error| ZenzInferenceError(error.to_string()))
    }

    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        self.tokenizer
            .decode(&[token])
            .map(String::into_bytes)
            .map_err(|error| ZenzInferenceError(error.to_string()))
    }

    fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        let probabilities = self
            .model
            .probabilities(tokens)
            .map_err(|error| ZenzInferenceError(error.to_string()))?;
        let mut logits: Vec<_> = probabilities
            .into_iter()
            .map(|probability| probability.max(1e-20).ln() as f32)
            .collect();
        let maximum = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let normalization = maximum
            + logits
                .iter()
                .map(|value| (*value - maximum).exp())
                .sum::<f32>()
                .ln();
        for value in &mut logits {
            *value -= normalization;
        }
        Ok(logits)
    }
}

fn normalize_prefix(prefix: &Path) -> Result<PathBuf, NGramError> {
    let value = prefix
        .to_str()
        .ok_or_else(|| NGramError::InvalidPath(prefix.to_path_buf()))?;
    Ok(PathBuf::from(value.trim_end_matches('_')))
}

fn with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut value = prefix.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

struct MarisaTrie(NonNull<BeanKeyMarisaTrie>);

impl MarisaTrie {
    fn load(path: PathBuf) -> Result<Self, NGramError> {
        let path_string = path
            .to_str()
            .ok_or_else(|| NGramError::InvalidPath(path.clone()))?;
        let path_c =
            CString::new(path_string).map_err(|_| NGramError::InvalidPath(path.clone()))?;
        // SAFETY: path_c is a valid null-terminated string for the duration of the call. The C++
        // boundary catches exceptions and returns null on failure.
        let trie = unsafe { beankey_marisa_load(path_c.as_ptr()) };
        NonNull::new(trie)
            .map(Self)
            .ok_or(NGramError::LoadTrie(path))
    }

    fn get_value(&self, key: &[usize]) -> Result<Option<u32>, NGramError> {
        let mut query = encode_key(key)?;
        query.push(KEY_VALUE_DELIMITER);
        let values = self.search(&query)?;
        values
            .first()
            .map(|entry| decode_value(entry.get(query.len()..).unwrap_or_default()))
            .transpose()
    }

    fn bulk_values(&self, prefix: &[usize]) -> Result<(Vec<u32>, u32), NGramError> {
        let mut query = encode_key(prefix)?;
        query.push(PREDICTIVE_DELIMITER);
        let mut values = vec![0; VOCABULARY_SIZE];
        let mut sum = 0_u32;
        for entry in self.search(&query)? {
            let suffix = entry
                .get(query.len()..)
                .ok_or(NGramError::InvalidTrieEntry)?;
            if suffix.len() < 8 || suffix[2] != KEY_VALUE_DELIMITER {
                continue;
            }
            let token = decode_token(suffix[0], suffix[1])?;
            let value = decode_value(&suffix[3..])?;
            values[token] = value;
            sum = sum.checked_add(value).ok_or(NGramError::CountOverflow)?;
        }
        Ok((values, sum))
    }

    fn search(&self, query: &[u8]) -> Result<Vec<Vec<u8>>, NGramError> {
        unsafe extern "C" fn collect(value: *const u8, size: usize, context: *mut c_void) -> bool {
            if value.is_null() && size != 0 {
                return false;
            }
            // SAFETY: search passes the address of the live output Vec as context, and Marisa
            // keeps the key bytes valid for the duration of this callback.
            let output = unsafe { &mut *context.cast::<Vec<Vec<u8>>>() };
            if size == 0 {
                output.push(Vec::new());
            } else {
                // SAFETY: The callback contract supplies size readable bytes and value is non-null.
                let value = unsafe { std::slice::from_raw_parts(value, size) };
                output.push(value.to_vec());
            }
            true
        }

        let mut output = Vec::<Vec<u8>>::new();
        // SAFETY: self owns a live read-only trie, query remains valid for the call, collect obeys
        // the callback ABI, and output remains live and exclusively borrowed through context.
        let succeeded = unsafe {
            beankey_marisa_predictive_search(
                self.0.as_ptr(),
                query.as_ptr(),
                query.len(),
                collect,
                (&mut output as *mut Vec<Vec<u8>>).cast(),
            )
        };
        succeeded.then_some(output).ok_or(NGramError::SearchTrie)
    }
}

// SAFETY: A loaded Marisa trie is immutable. Each search creates its own Agent in the C++ shim,
// and destruction happens only after the last Rust owner is dropped.
unsafe impl Send for MarisaTrie {}
// SAFETY: See the Send implementation; concurrent searches do not mutate the trie.
unsafe impl Sync for MarisaTrie {}

impl Drop for MarisaTrie {
    fn drop(&mut self) {
        // SAFETY: The pointer was returned by beankey_marisa_load and is freed exactly once.
        unsafe { beankey_marisa_free(self.0.as_ptr()) };
    }
}

fn encode_key(key: &[usize]) -> Result<Vec<u8>, NGramError> {
    let mut output = Vec::with_capacity(key.len() * 2);
    for token in key {
        if *token >= VOCABULARY_SIZE {
            return Err(NGramError::InvalidTrieEntry);
        }
        output.push(u8::try_from(token / ENCODING_RADIX + 1).expect("6000-token quotient fits u8"));
        output.push(u8::try_from(token % ENCODING_RADIX + 1).expect("token remainder fits u8"));
    }
    Ok(output)
}

fn decode_token(high: u8, low: u8) -> Result<usize, NGramError> {
    let high = high.checked_sub(1).ok_or(NGramError::InvalidTrieEntry)?;
    let low = low.checked_sub(1).ok_or(NGramError::InvalidTrieEntry)?;
    let token = usize::from(high) * ENCODING_RADIX + usize::from(low);
    (token < VOCABULARY_SIZE)
        .then_some(token)
        .ok_or(NGramError::InvalidTrieEntry)
}

fn decode_value(value: &[u8]) -> Result<u32, NGramError> {
    let mut decoded = 0_u32;
    for byte in value.get(..5).ok_or(NGramError::InvalidTrieEntry)? {
        let digit = byte.checked_sub(1).ok_or(NGramError::InvalidTrieEntry)?;
        decoded = decoded
            .checked_mul(ENCODING_RADIX as u32)
            .and_then(|value| value.checked_add(u32::from(digit)))
            .ok_or(NGramError::CountOverflow)?;
    }
    Ok(decoded)
}

#[repr(C)]
struct BeanKeyMarisaTrie {
    _private: [u8; 0],
}

type MarisaVisitor = unsafe extern "C" fn(*const u8, usize, *mut c_void) -> bool;

unsafe extern "C" {
    fn beankey_marisa_load(path: *const c_char) -> *mut BeanKeyMarisaTrie;
    fn beankey_marisa_free(trie: *mut BeanKeyMarisaTrie);
    fn beankey_marisa_predictive_search(
        trie: *const BeanKeyMarisaTrie,
        query: *const u8,
        query_size: usize,
        visitor: MarisaVisitor,
        visitor_context: *mut c_void,
    ) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Candidate, CandidateEvaluation, ComposingCount, DictionaryEntry, DictionaryMetadata,
        PrefixConstraint, ZenzEvaluationRequest, ZenzPersonalization, ZenzVersionConfig,
        evaluate_candidate,
    };

    struct PersonalizationTestModel;

    impl ZenzLanguageModel for PersonalizationTestModel {
        fn vocabulary_size(&self) -> usize {
            VOCABULARY_SIZE
        }

        fn eos_token(&self) -> i32 {
            END_TOKEN
        }

        fn tokenize(
            &mut self,
            text: &str,
            _add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            Ok(if text == "candidate" {
                vec![10, 21]
            } else {
                vec![5]
            })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(format!("{token}").into_bytes())
        }

        fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            let mut logits = vec![-10.0; VOCABULARY_SIZE];
            if tokens.last() == Some(&10) {
                logits[20] = 1.0;
                logits[21] = 0.9;
            } else {
                logits[10] = 1.0;
            }
            Ok(logits)
        }
    }

    #[test]
    fn key_and_value_encoding_matches_the_fixed_upstream_format() {
        assert_eq!(
            encode_key(&[0, 125, 126, 5999]).unwrap(),
            [1, 1, 1, 126, 2, 1, 48, 78]
        );
        assert_eq!(decode_token(48, 78).unwrap(), 5999);
        assert_eq!(decode_value(&[1, 1, 1, 1, 43]).unwrap(), 42);
    }

    #[test]
    fn rejects_out_of_range_tokens_and_truncated_values() {
        assert!(encode_key(&[VOCABULARY_SIZE]).is_err());
        assert!(decode_token(0, 1).is_err());
        assert!(decode_value(&[1, 2]).is_err());
    }

    #[test]
    fn personal_ngram_probabilities_can_change_the_zenz_token_choice() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ngram");
        let base = EfficientNGram::open(fixture.join("lm"), 2, 0.75).unwrap();
        let personal = EfficientNGram::open(fixture.join("personal"), 2, 0.75).unwrap();
        let candidate = Candidate::single(
            "candidate".into(),
            -1.0,
            ComposingCount::Input(1),
            500,
            vec![DictionaryEntry {
                word: "candidate".into(),
                ruby: "candidate".into(),
                left_id: 0,
                right_id: 0,
                meaning_id: 500,
                base_value: -1.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            }],
        );
        let mut model = PersonalizationTestModel;
        let result = evaluate_candidate(
            &mut model,
            ZenzEvaluationRequest {
                input: "input",
                input_cursor_position: None,
                candidate: &candidate,
                request_rich_candidates: false,
                prefix_constraint: &PrefixConstraint::default(),
                personalization: Some(ZenzPersonalization {
                    alpha: 0.5,
                    base: &base,
                    personal: &personal,
                }),
                version: &ZenzVersionConfig::default(),
            },
        )
        .unwrap();

        assert!(matches!(result, CandidateEvaluation::Pass { .. }));
    }
}
