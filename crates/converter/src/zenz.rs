use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::hash::Hash;

use unicode_segmentation::UnicodeSegmentation;

use crate::kana::{to_hiragana, to_katakana};
use crate::{Candidate, DictionaryMetadata};

const INPUT_TAG: char = '\u{EE00}';
const OUTPUT_TAG: char = '\u{EE01}';
const CONTEXT_TAG: char = '\u{EE02}';
const PROFILE_TAG: char = '\u{EE03}';
const TOPIC_TAG: char = '\u{EE04}';
const STYLE_TAG: char = '\u{EE05}';
const PREFERENCE_TAG: char = '\u{EE06}';
const RIGHT_CONTEXT_TAG: char = '\u{EE07}';
pub const ALIGNMENT_SEPARATOR: char = '\u{EE08}';

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PrefixConstraint {
    pub bytes: Vec<u8>,
    pub has_eos: bool,
    pub ignore_memory_and_user_dictionary: bool,
    pub fixed_prefix: Option<FixedPrefix>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FixedPrefix {
    pub byte_count: usize,
    pub ruby_count: usize,
}

impl PrefixConstraint {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            ..Self::default()
        }
    }

    pub fn normalized(
        bytes: Vec<u8>,
        default_has_eos: bool,
        ignore_memory_and_user_dictionary: bool,
    ) -> Self {
        let separator = ALIGNMENT_SEPARATOR.to_string().into_bytes();
        let alignment = bytes
            .windows(separator.len())
            .position(|window| window == separator);
        match alignment {
            Some(index) => Self {
                bytes: bytes[..index].to_vec(),
                has_eos: true,
                ignore_memory_and_user_dictionary,
                fixed_prefix: None,
            },
            None => Self {
                bytes,
                has_eos: default_has_eos,
                ignore_memory_and_user_dictionary,
                fixed_prefix: None,
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() && !self.has_eos
    }

    pub(crate) fn can_continue(&self, candidate: &[u8]) -> bool {
        if self.has_eos {
            candidate.len() <= self.bytes.len() && self.bytes.starts_with(candidate)
        } else {
            self.bytes.starts_with(candidate) || candidate.starts_with(&self.bytes)
        }
    }

    pub(crate) fn is_satisfied_by(&self, candidate: &[u8]) -> bool {
        if self.has_eos {
            candidate == self.bytes
        } else {
            candidate.starts_with(&self.bytes)
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ZenzV2Config {
    pub profile: Option<String>,
    pub left_context: Option<String>,
    pub max_left_context_length: Option<usize>,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ZenzV3Config {
    pub profile: Option<String>,
    pub topic: Option<String>,
    pub style: Option<String>,
    pub preference: Option<String>,
    pub left_context: Option<String>,
    pub right_context: Option<String>,
    pub max_left_context_length: Option<usize>,
    pub max_right_context_length: Option<usize>,
    pub enable_alignment_separator: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ZenzVersionConfig {
    V2(ZenzV2Config),
    V3(ZenzV3Config),
}

impl Default for ZenzVersionConfig {
    fn default() -> Self {
        Self::V3(ZenzV3Config::default())
    }
}

pub struct ZenzPromptBuilder;

impl ZenzPromptBuilder {
    pub fn input_prediction(
        left_context: &str,
        composing_text: &str,
        version: &ZenzVersionConfig,
    ) -> Option<String> {
        let ZenzVersionConfig::V3(config) = version else {
            return None;
        };
        let mut prompt = v3_conditions(config).concat();
        let left_context = suffix(left_context, config.max_left_context_length.unwrap_or(40));
        let right_context = prefix(
            config.right_context.as_deref().unwrap_or_default(),
            config.max_right_context_length.unwrap_or(40),
        );
        if !left_context.is_empty() {
            prompt.push(CONTEXT_TAG);
            prompt.push_str(&left_context);
        }
        if !right_context.is_empty() {
            prompt.push(RIGHT_CONTEXT_TAG);
            prompt.push_str(&right_context);
        }
        prompt.push(INPUT_TAG);
        prompt.push_str(&to_katakana(composing_text));
        Some(prompt)
    }

    pub fn typo_correction_prefix(left_context: &str) -> String {
        if left_context.is_empty() {
            INPUT_TAG.to_string()
        } else {
            format!("{CONTEXT_TAG}{left_context}{INPUT_TAG}")
        }
    }

    pub fn candidate_evaluation(
        input: &str,
        input_cursor_position: Option<usize>,
        user_dictionary_prompt: &str,
        version: &ZenzVersionConfig,
    ) -> String {
        let mut conditions = Vec::new();
        if !user_dictionary_prompt.is_empty() {
            conditions.push(format!("辞書:{user_dictionary_prompt}"));
        }

        match version {
            ZenzVersionConfig::V2(config) => {
                if let Some(profile) = config.profile.as_deref().filter(|value| !value.is_empty()) {
                    conditions.push(format!("プロフィール:{}", suffix(profile, 25)));
                }
                let left_context = suffix(
                    config.left_context.as_deref().unwrap_or_default(),
                    config.max_left_context_length.unwrap_or(40),
                );
                if !conditions.is_empty() {
                    format!(
                        "{INPUT_TAG}{input}{CONTEXT_TAG}{}・発言:{left_context}{OUTPUT_TAG}",
                        conditions.join("・")
                    )
                } else if !left_context.is_empty() {
                    format!("{INPUT_TAG}{input}{CONTEXT_TAG}{left_context}{OUTPUT_TAG}")
                } else {
                    format!("{INPUT_TAG}{input}{OUTPUT_TAG}")
                }
            }
            ZenzVersionConfig::V3(config) => {
                let mut prompt = v3_conditions(config).concat();
                if !user_dictionary_prompt.is_empty() {
                    prompt.insert_str(0, &format!("辞書:{user_dictionary_prompt}"));
                }
                let left_context = suffix(
                    config.left_context.as_deref().unwrap_or_default(),
                    config.max_left_context_length.unwrap_or(40),
                );
                let right_context = prefix(
                    config.right_context.as_deref().unwrap_or_default(),
                    config.max_right_context_length.unwrap_or(40),
                );
                if !left_context.is_empty() {
                    prompt.push(CONTEXT_TAG);
                    prompt.push_str(&left_context);
                }
                if !right_context.is_empty() {
                    prompt.push(RIGHT_CONTEXT_TAG);
                    prompt.push_str(&right_context);
                }
                prompt.push(INPUT_TAG);
                prompt.push_str(&aligned_input(
                    input,
                    input_cursor_position,
                    config.enable_alignment_separator,
                ));
                prompt.push(OUTPUT_TAG);
                prompt
            }
        }
    }

    pub fn candidate_text_for_evaluation(
        candidate_text: &str,
        input: &str,
        input_cursor_position: Option<usize>,
        version: &ZenzVersionConfig,
    ) -> String {
        let append_separator = matches!(
            version,
            ZenzVersionConfig::V3(ZenzV3Config {
                enable_alignment_separator: true,
                ..
            })
        ) && should_insert_alignment_separator(input, input_cursor_position);
        if append_separator {
            format!("{candidate_text}{ALIGNMENT_SEPARATOR}")
        } else {
            candidate_text.to_owned()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZenzInferenceError {
    Message(String),
    ContextCapacity { tokens: usize, capacity: usize },
}

impl ZenzInferenceError {
    pub fn is_context_capacity_exceeded(&self) -> bool {
        matches!(self, Self::ContextCapacity { .. })
    }
}

impl fmt::Display for ZenzInferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::ContextCapacity { tokens, capacity } => write!(
                formatter,
                "inference requires {tokens} tokens, exceeding context capacity {capacity}"
            ),
        }
    }
}

impl Error for ZenzInferenceError {}

pub trait ZenzLanguageModel: Send {
    fn vocabulary_size(&self) -> usize;
    fn eos_token(&self) -> i32;
    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError>;
    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError>;
    fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError>;

    fn logits_for_suffix(
        &mut self,
        tokens: &[i32],
        start_index: usize,
        _sequence: ZenzInferenceSequence,
    ) -> Result<Vec<f32>, ZenzInferenceError> {
        if start_index >= tokens.len() {
            return Err(ZenzInferenceError::Message(format!(
                "logits start index {start_index} is outside {} input tokens",
                tokens.len()
            )));
        }
        let vocabulary_size = self.vocabulary_size();
        let mut logits = Vec::with_capacity((tokens.len() - start_index) * vocabulary_size);
        for index in start_index..tokens.len() {
            let row = self.next_logits(&tokens[..=index])?;
            if row.len() != vocabulary_size {
                return Err(ZenzInferenceError::Message(format!(
                    "model returned {} logits for a vocabulary of {vocabulary_size}",
                    row.len()
                )));
            }
            logits.extend(row);
        }
        Ok(logits)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZenzInferenceSequence {
    Evaluation,
    InputPrediction,
}

pub trait TokenProbabilityModel {
    fn probabilities(&self, prefix: &[i32], vocabulary_size: usize) -> Option<Vec<f32>>;
}

pub struct ZenzPersonalization<'a> {
    pub alpha: f32,
    pub base: &'a dyn TokenProbabilityModel,
    pub personal: &'a dyn TokenProbabilityModel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlternativeConstraint {
    pub probability_ratio: f32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandidateEvaluation {
    Pass {
        score: f32,
        alternatives: Vec<AlternativeConstraint>,
    },
    FixRequired(Vec<u8>),
    WholeResult(String),
}

pub struct ZenzEvaluationRequest<'a> {
    pub input: &'a str,
    pub input_cursor_position: Option<usize>,
    pub candidate: &'a Candidate,
    pub request_rich_candidates: bool,
    pub prefix_constraint: &'a PrefixConstraint,
    pub personalization: Option<ZenzPersonalization<'a>>,
    pub version: &'a ZenzVersionConfig,
}

/// A small group of alternatives for the same ordinary or partial conversion task.
/// Prediction and typo-correction candidates must use their own task scorers.
pub struct ZenzComparisonRequest<'a> {
    pub input: &'a str,
    pub input_cursor_position: Option<usize>,
    pub candidates: &'a [Candidate],
    pub prefix_constraint: &'a PrefixConstraint,
    pub version: &'a ZenzVersionConfig,
    /// Total chosen-token rows across the group, including task termination.
    pub token_budget: usize,
}

const MAX_COMPARISON_CANDIDATES: usize = 4;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EvaluationCacheKey {
    prompt: String,
    candidate_text_for_evaluation: String,
    original_candidate_text: String,
    prefix_constraint: PrefixConstraint,
    request_rich_candidates: bool,
    reuses_addressed_prefix: bool,
}

#[derive(Clone, Debug)]
struct CacheEntry<V> {
    value: V,
    access_index: u64,
}

#[derive(Debug)]
struct LruCache<K, V> {
    capacity: usize,
    access_index: u64,
    entries: HashMap<K, CacheEntry<V>>,
}

impl<K: Clone + Eq + Hash, V: Clone> LruCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            access_index: 0,
            entries: HashMap::new(),
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.get_mut(key)?;
        self.access_index = self.access_index.wrapping_add(1);
        entry.access_index = self.access_index;
        Some(entry.value.clone())
    }

    fn insert(&mut self, key: K, value: V) {
        self.access_index = self.access_index.wrapping_add(1);
        if !self.entries.contains_key(&key)
            && self.entries.len() >= self.capacity
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.access_index)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            CacheEntry {
                value,
                access_index: self.access_index,
            },
        );
    }
}

#[derive(Debug)]
pub struct ZenzEvaluator {
    evaluations: LruCache<EvaluationCacheKey, CandidateEvaluation>,
    prompt_tokens: LruCache<String, Vec<i32>>,
    previous_evaluation_prompt: String,
}

impl Default for ZenzEvaluator {
    fn default() -> Self {
        Self {
            evaluations: LruCache::new(256),
            prompt_tokens: LruCache::new(128),
            previous_evaluation_prompt: String::new(),
        }
    }
}

impl ZenzEvaluator {
    /// Returns teacher-forced sequence log probabilities, never dictionary costs
    /// or greedy acceptance scores. All alternatives share one request-wise
    /// explicit user-dictionary prompt and the same task termination.
    ///
    /// An inapplicable task, invalid fixed prefix, exhausted total token budget,
    /// or context-capacity limit skips the whole group. Tokenization preflight
    /// finishes before any logits are requested. This opt-in path neither
    /// changes greedy evaluation state nor reranks candidates itself.
    pub fn compare_candidates(
        &mut self,
        model: &mut dyn ZenzLanguageModel,
        request: ZenzComparisonRequest<'_>,
    ) -> Result<Option<Vec<f64>>, ZenzInferenceError> {
        match self.compare_candidates_within_budget(model, request) {
            Err(ZenzInferenceError::ContextCapacity { .. }) => Ok(None),
            result => result,
        }
    }

    fn compare_candidates_within_budget(
        &mut self,
        model: &mut dyn ZenzLanguageModel,
        request: ZenzComparisonRequest<'_>,
    ) -> Result<Option<Vec<f64>>, ZenzInferenceError> {
        let Some(first) = request.candidates.first() else {
            return Ok(None);
        };
        let input_count = UnicodeSegmentation::graphemes(request.input, true).count();
        let consumed_count = request.input_cursor_position.unwrap_or(input_count);
        if request.candidates.len() > MAX_COMPARISON_CANDIDATES
            || request.token_budget == 0
            || consumed_count == 0
            || consumed_count > input_count
            || request.candidates.iter().any(|candidate| {
                normalize_for_model(&candidate.text).is_empty()
                    || candidate.ruby_count != consumed_count
                    || candidate.composing_count != first.composing_count
                    || candidate.is_typo_correction
            })
        {
            return Ok(None);
        }

        let mut input = request.input.to_owned();
        let mut cursor = request.input_cursor_position;
        let mut version = request.version.clone();
        let mut candidates = request.candidates.to_vec();
        if let Some(fixed) = request.prefix_constraint.fixed_prefix {
            let Some(bytes) = request.prefix_constraint.bytes.get(..fixed.byte_count) else {
                return Ok(None);
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Ok(None);
            };
            if fixed.byte_count == 0
                || fixed.ruby_count == 0
                || fixed.ruby_count > consumed_count
                || candidates
                    .iter()
                    .any(|candidate| candidate.text.get(..fixed.byte_count) != Some(text))
            {
                return Ok(None);
            }
            input = drop_graphemes(&input, fixed.ruby_count)
                .expect("fixed reading was checked against the consumed span");
            cursor = cursor.map(|position| position - fixed.ruby_count);
            version = appending_left_context(&version, text);
            for candidate in &mut candidates {
                candidate.text = candidate.text[fixed.byte_count..].to_owned();
                candidate.entries = dropping_fixed_entries(&candidate.entries, fixed.byte_count);
                candidate.ruby_count -= fixed.ruby_count;
            }
        }

        let is_partial = should_insert_alignment_separator(&input, cursor);
        let uses_alignment = is_partial
            && matches!(
                &version,
                ZenzVersionConfig::V3(ZenzV3Config {
                    enable_alignment_separator: true,
                    ..
                })
            );
        // Without alignment framing, EOS would incorrectly turn a partial
        // conversion into a full-result task. Do not compare those tasks.
        if is_partial && !uses_alignment {
            return Ok(None);
        }

        let user_entries: BTreeSet<_> = candidates
            .iter()
            .flat_map(|candidate| &candidate.entries)
            .filter(|entry| entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY))
            .map(|entry| (entry.word.as_str(), to_hiragana(&entry.ruby)))
            .collect();
        let user_dictionary_prompt = user_entries
            .into_iter()
            .map(|(word, ruby)| format!("{word}({ruby})"))
            .collect::<String>();
        let prompt = ZenzPromptBuilder::candidate_evaluation(
            &input,
            cursor,
            &user_dictionary_prompt,
            &version,
        );
        let prompt_tokens = if let Some(tokens) = self.prompt_tokens.get(&prompt) {
            tokens
        } else {
            let tokens = tokenize_for_model(model, &prompt, true)?;
            self.prompt_tokens.insert(prompt, tokens.clone());
            tokens
        };
        if prompt_tokens.is_empty() {
            return Ok(None);
        }
        let vocabulary_size = model.vocabulary_size();
        if vocabulary_size == 0 {
            return Err(ZenzInferenceError::Message(
                "candidate comparison requires a nonempty vocabulary".into(),
            ));
        }
        let mut tokenized = Vec::with_capacity(candidates.len());
        let mut total_tokens = 0usize;
        for candidate in &candidates {
            let text = ZenzPromptBuilder::candidate_text_for_evaluation(
                &candidate.text,
                &input,
                cursor,
                &version,
            );
            // Tokenize text and any alignment terminal together, rather than
            // assuming a tokenizer boundary between them.
            let mut tokens = tokenize_for_model(model, &text, false)?;
            if !text.is_empty() && tokens.is_empty() {
                return Ok(None);
            }
            if !uses_alignment {
                tokens.push(model.eos_token());
            }
            if tokens.iter().any(|&token| {
                usize::try_from(token)
                    .ok()
                    .is_none_or(|token| token >= vocabulary_size)
            }) {
                return Err(ZenzInferenceError::Message(
                    "candidate comparison token is outside the vocabulary".into(),
                ));
            }
            let Some(next_total) = total_tokens.checked_add(tokens.len()) else {
                return Ok(None);
            };
            if next_total > request.token_budget {
                return Ok(None);
            }
            total_tokens = next_total;
            tokenized.push(tokens);
        }

        let mut scores = Vec::with_capacity(tokenized.len());
        for tokens in tokenized {
            let expected_logits = tokens.len().checked_mul(vocabulary_size).ok_or_else(|| {
                ZenzInferenceError::Message("comparison logits size overflow".into())
            })?;
            let mut evaluation_tokens = prompt_tokens.clone();
            evaluation_tokens.extend_from_slice(&tokens[..tokens.len() - 1]);
            let logits = model.logits_for_suffix(
                &evaluation_tokens,
                prompt_tokens.len() - 1,
                ZenzInferenceSequence::Evaluation,
            )?;
            if logits.len() != expected_logits {
                return Err(ZenzInferenceError::Message(format!(
                    "model returned {} logits for {} comparison tokens and a vocabulary of {vocabulary_size}",
                    logits.len(),
                    tokens.len()
                )));
            }
            let mut score = 0.0;
            for (row, token) in logits.chunks_exact(vocabulary_size).zip(tokens) {
                score += chosen_token_log_probability(row, token)?;
            }
            scores.push(score);
        }
        Ok(Some(scores))
    }

    pub fn evaluate(
        &mut self,
        model: &mut dyn ZenzLanguageModel,
        request: ZenzEvaluationRequest<'_>,
    ) -> Result<CandidateEvaluation, ZenzInferenceError> {
        let Some(fixed_prefix) = request.prefix_constraint.fixed_prefix else {
            return self.evaluate_as_is(model, request);
        };
        if fixed_prefix.byte_count == 0
            || request.candidate.text.len() < fixed_prefix.byte_count
            || request.prefix_constraint.bytes.len() < fixed_prefix.byte_count
            || !request
                .candidate
                .text
                .as_bytes()
                .starts_with(&request.prefix_constraint.bytes[..fixed_prefix.byte_count])
            || UnicodeSegmentation::graphemes(request.input, true).count() < fixed_prefix.ruby_count
            || request
                .input_cursor_position
                .is_some_and(|position| position < fixed_prefix.ruby_count)
        {
            return self.evaluate_as_is(model, request);
        }

        let fixed = request.prefix_constraint.bytes[..fixed_prefix.byte_count].to_vec();
        let fixed_text = std::str::from_utf8(&fixed).map_err(|error| {
            ZenzInferenceError::Message(format!("fixed prefix is not UTF-8: {error}"))
        })?;
        let remaining_input = drop_graphemes(request.input, fixed_prefix.ruby_count)
            .expect("fixed prefix ruby count was checked above");
        let remaining_text = request
            .candidate
            .text
            .get(fixed_prefix.byte_count..)
            .expect("fixed prefix byte count must be on a UTF-8 boundary")
            .to_owned();
        let mut remaining_candidate = request.candidate.clone();
        remaining_candidate.text = remaining_text;
        remaining_candidate.entries =
            dropping_fixed_entries(&request.candidate.entries, fixed_prefix.byte_count);
        let mut remaining_constraint = request.prefix_constraint.clone();
        remaining_constraint.bytes.drain(..fixed_prefix.byte_count);
        remaining_constraint.fixed_prefix = None;
        let remaining_version = appending_left_context(request.version, fixed_text);
        let remaining_request = ZenzEvaluationRequest {
            input: &remaining_input,
            input_cursor_position: request
                .input_cursor_position
                .map(|position| position - fixed_prefix.ruby_count),
            candidate: &remaining_candidate,
            request_rich_candidates: request.request_rich_candidates,
            prefix_constraint: &remaining_constraint,
            personalization: request.personalization,
            version: &remaining_version,
        };
        let result = self.evaluate_as_is(model, remaining_request)?;
        Ok(prepend_fixed(&fixed, fixed_text, result))
    }

    fn evaluate_as_is(
        &mut self,
        model: &mut dyn ZenzLanguageModel,
        request: ZenzEvaluationRequest<'_>,
    ) -> Result<CandidateEvaluation, ZenzInferenceError> {
        let user_dictionary_prompt = request
            .candidate
            .entries
            .iter()
            .filter(|entry| entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY))
            .map(|entry| format!("{}({})", entry.word, to_hiragana(&entry.ruby)))
            .collect::<String>();
        let prompt = ZenzPromptBuilder::candidate_evaluation(
            request.input,
            request.input_cursor_position,
            &user_dictionary_prompt,
            request.version,
        );
        let normalized_prompt = normalize_for_model(&prompt);
        let candidate_text = ZenzPromptBuilder::candidate_text_for_evaluation(
            &request.candidate.text,
            request.input,
            request.input_cursor_position,
            request.version,
        );
        let reuses_addressed_prefix = self.previous_evaluation_prompt == normalized_prompt
            && !request.request_rich_candidates;
        self.previous_evaluation_prompt = normalized_prompt.clone();
        let cache_key = request
            .personalization
            .is_none()
            .then(|| EvaluationCacheKey {
                prompt: prompt.clone(),
                candidate_text_for_evaluation: candidate_text.clone(),
                original_candidate_text: request.candidate.text.clone(),
                prefix_constraint: request.prefix_constraint.clone(),
                request_rich_candidates: request.request_rich_candidates,
                reuses_addressed_prefix,
            });
        if let Some(cached) = cache_key.as_ref().and_then(|key| self.evaluations.get(key)) {
            return Ok(cached);
        }
        let prompt_tokens = if let Some(tokens) = self.prompt_tokens.get(&prompt) {
            tokens
        } else {
            let tokens = tokenize_for_model(model, &prompt, true)?;
            self.prompt_tokens.insert(prompt.clone(), tokens.clone());
            tokens
        };
        let addressed_token_count = if reuses_addressed_prefix {
            let mut prefix = String::new();
            for character in UnicodeSegmentation::graphemes(request.candidate.text.as_str(), true) {
                let next = format!("{prefix}{character}");
                if request.prefix_constraint.bytes.starts_with(next.as_bytes()) {
                    prefix = next;
                } else {
                    break;
                }
            }
            if prefix.is_empty() {
                0
            } else {
                tokenize_for_model(model, &prefix, false)?.len()
            }
        } else {
            0
        };
        let result = evaluate_candidate_tokens(
            model,
            &request,
            prompt_tokens,
            &candidate_text,
            addressed_token_count,
        )?;
        if let Some(cache_key) = cache_key {
            self.evaluations.insert(cache_key, result.clone());
        }
        Ok(result)
    }
}

fn drop_graphemes(value: &str, count: usize) -> Option<String> {
    let graphemes: Vec<_> = UnicodeSegmentation::graphemes(value, true).collect();
    (count <= graphemes.len()).then(|| graphemes[count..].concat())
}

fn dropping_fixed_entries(
    entries: &[crate::DictionaryEntry],
    byte_count: usize,
) -> Vec<crate::DictionaryEntry> {
    let mut covered = 0;
    let mut first_remaining = 0;
    for entry in entries {
        let next = covered + entry.word.len();
        if next > byte_count {
            break;
        }
        covered = next;
        first_remaining += 1;
    }
    entries[first_remaining..].to_vec()
}

fn appending_left_context(version: &ZenzVersionConfig, fixed: &str) -> ZenzVersionConfig {
    let append = |context: &Option<String>| {
        let mut value = context.clone().unwrap_or_default();
        value.push_str(fixed);
        value
    };
    match version {
        ZenzVersionConfig::V2(config) => ZenzVersionConfig::V2(ZenzV2Config {
            left_context: Some(append(&config.left_context)),
            ..config.clone()
        }),
        ZenzVersionConfig::V3(config) => ZenzVersionConfig::V3(ZenzV3Config {
            left_context: Some(append(&config.left_context)),
            ..config.clone()
        }),
    }
}

fn prepend_fixed(
    fixed: &[u8],
    fixed_text: &str,
    result: CandidateEvaluation,
) -> CandidateEvaluation {
    match result {
        CandidateEvaluation::Pass {
            score,
            alternatives,
        } => CandidateEvaluation::Pass {
            score,
            alternatives: alternatives
                .into_iter()
                .map(|alternative| {
                    let mut bytes = fixed.to_vec();
                    bytes.extend(alternative.bytes);
                    AlternativeConstraint {
                        probability_ratio: alternative.probability_ratio,
                        bytes,
                    }
                })
                .collect(),
        },
        CandidateEvaluation::FixRequired(mut bytes) => {
            let mut prefix = fixed.to_vec();
            prefix.append(&mut bytes);
            CandidateEvaluation::FixRequired(prefix)
        }
        CandidateEvaluation::WholeResult(result) => {
            CandidateEvaluation::WholeResult(format!("{fixed_text}{result}"))
        }
    }
}

pub fn evaluate_candidate(
    model: &mut dyn ZenzLanguageModel,
    request: ZenzEvaluationRequest<'_>,
) -> Result<CandidateEvaluation, ZenzInferenceError> {
    ZenzEvaluator::default().evaluate(model, request)
}

fn evaluate_candidate_tokens(
    model: &mut dyn ZenzLanguageModel,
    request: &ZenzEvaluationRequest<'_>,
    prompt_tokens: Vec<i32>,
    candidate_text: &str,
    addressed_token_count: usize,
) -> Result<CandidateEvaluation, ZenzInferenceError> {
    let candidate_tokens = tokenize_for_model(model, candidate_text, false)?;
    let vocabulary_size = model.vocabulary_size();
    let addressed_token_count = addressed_token_count.min(candidate_tokens.len());
    let mut candidate_prefix = Vec::new();
    for &token in &candidate_tokens[..addressed_token_count] {
        candidate_prefix.extend(model.token_to_piece(token)?);
    }
    let mut score = 0.0;
    let mut alternatives = Vec::new();

    if candidate_tokens.is_empty() {
        return Ok(CandidateEvaluation::Pass {
            score,
            alternatives,
        });
    }
    if prompt_tokens.is_empty() {
        return Err(ZenzInferenceError::Message(
            "candidate evaluation prompt produced no tokens".into(),
        ));
    }
    if addressed_token_count == candidate_tokens.len() {
        return Ok(CandidateEvaluation::Pass {
            score,
            alternatives,
        });
    }
    let prompt_token_count = prompt_tokens.len();
    let mut evaluation_tokens = prompt_tokens;
    evaluation_tokens.extend_from_slice(&candidate_tokens[..candidate_tokens.len() - 1]);
    let logits = model.logits_for_suffix(
        &evaluation_tokens,
        prompt_token_count - 1 + addressed_token_count,
        ZenzInferenceSequence::Evaluation,
    )?;
    let expected_logits = candidate_tokens
        .len()
        .saturating_sub(addressed_token_count)
        .checked_mul(vocabulary_size)
        .ok_or_else(|| ZenzInferenceError::Message("candidate logits size overflow".into()))?;
    if logits.len() != expected_logits || vocabulary_size == 0 {
        return Err(ZenzInferenceError::Message(format!(
            "model returned {} logits for {} candidate tokens and a vocabulary of {vocabulary_size}",
            logits.len(),
            candidate_tokens.len()
        )));
    }

    for (candidate_index, &candidate_token) in candidate_tokens
        .iter()
        .enumerate()
        .skip(addressed_token_count)
    {
        let row_start = (candidate_index - addressed_token_count) * vocabulary_size;
        let row = &logits[row_start..row_start + vocabulary_size];
        let personalized = personalized_logits(
            row,
            &candidate_tokens[..candidate_index],
            request.personalization.as_ref(),
        );
        let evaluated = personalized.as_deref().unwrap_or(row);
        let maximum_token = strict_argmax(evaluated);
        let maximum_logit = evaluated[maximum_token];
        let candidate_token_index = usize::try_from(candidate_token)
            .ok()
            .filter(|&token| token < vocabulary_size)
            .ok_or_else(|| {
                ZenzInferenceError::Message(format!("invalid candidate token {candidate_token}"))
            })?;

        let candidate_was_maximum = maximum_token == candidate_token_index;
        if !candidate_was_maximum {
            if i32::try_from(maximum_token).ok() == Some(model.eos_token()) {
                return Ok(CandidateEvaluation::WholeResult(
                    String::from_utf8(candidate_prefix).unwrap_or_default(),
                ));
            }
            candidate_prefix.extend(
                model.token_to_piece(
                    i32::try_from(maximum_token).map_err(|_| {
                        ZenzInferenceError::Message("vocabulary exceeds i32".into())
                    })?,
                )?,
            );
            return Ok(CandidateEvaluation::FixRequired(candidate_prefix));
        }

        if request.request_rich_candidates && candidate_was_maximum {
            let mut top_tokens: Vec<_> = evaluated.iter().copied().enumerate().collect();
            top_tokens.sort_by(|left, right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| left.0.cmp(&right.0))
            });
            for (token, logit) in top_tokens.into_iter().take(3) {
                if token == maximum_token {
                    continue;
                }
                let mut bytes = candidate_prefix.clone();
                bytes.extend(model.token_to_piece(i32::try_from(token).map_err(|_| {
                    ZenzInferenceError::Message("vocabulary exceeds i32".into())
                })?)?);
                alternatives.push(AlternativeConstraint {
                    probability_ratio: (logit - maximum_logit).exp(),
                    bytes,
                });
            }
        }
        score += maximum_logit;
        candidate_prefix.extend(model.token_to_piece(candidate_token)?);
    }

    alternatives.sort_by(|left, right| right.probability_ratio.total_cmp(&left.probability_ratio));
    alternatives.truncate(5);
    Ok(CandidateEvaluation::Pass {
        score,
        alternatives,
    })
}

pub struct ZenzInputGenerationRequest<'a> {
    pub left_context: &'a str,
    pub composing_text: &'a str,
    pub count: usize,
    pub min_length: usize,
    pub max_entropy: Option<f32>,
    pub version: &'a ZenzVersionConfig,
    pub possible_nexts: &'a [String],
}

pub fn generate_next_input(
    model: &mut dyn ZenzLanguageModel,
    request: ZenzInputGenerationRequest<'_>,
) -> Result<String, ZenzInferenceError> {
    if request.count == 0 {
        return Ok(String::new());
    }
    let Some(prompt) = ZenzPromptBuilder::input_prediction(
        request.left_context,
        request.composing_text,
        request.version,
    ) else {
        return Ok(String::new());
    };
    let allowed_prefixes: Vec<_> = request
        .possible_nexts
        .iter()
        .filter(|prefix| !prefix.is_empty())
        .collect();
    let is_allowed = |candidate: &str| {
        allowed_prefixes.is_empty()
            || allowed_prefixes
                .iter()
                .any(|prefix| prefix.starts_with(&to_katakana(candidate)))
    };
    let mut prompt_tokens = tokenize_for_model(model, &prompt, true)?;
    let min_length = request.min_length.clamp(1, request.count);
    let vocabulary_size = model.vocabulary_size();
    let mut predicted = String::new();
    let mut predicted_count = 0;

    for _ in 0..request.count {
        let logits = model.next_logits(&prompt_tokens)?;
        if logits.len() != vocabulary_size || logits.is_empty() {
            return Err(ZenzInferenceError::Message(format!(
                "model returned {} logits for a vocabulary of {vocabulary_size}",
                logits.len()
            )));
        }
        let mut token_penalties = HashMap::<i32, f32>::new();
        for (index, token) in prompt_tokens.iter().copied().enumerate() {
            *token_penalties.entry(token).or_default() +=
                2.0 / (prompt_tokens.len() - index) as f32;
        }
        let mut sum_exp = 0.0_f32;
        let mut sum_exp_value = 0.0_f32;
        let mut best_value = f32::NEG_INFINITY;
        let mut best_character = None;
        let mut best_next_text = String::new();

        for (token, logit) in logits.into_iter().enumerate() {
            let token = i32::try_from(token)
                .map_err(|_| ZenzInferenceError::Message("vocabulary exceeds i32".into()))?;
            let value = logit / (1.0 + token_penalties.get(&token).copied().unwrap_or_default());
            let exponential = value.exp();
            sum_exp += exponential;
            sum_exp_value += exponential * value;
            if value <= best_value {
                continue;
            }
            let piece = model.token_to_piece(token)?;
            let Ok(piece) = std::str::from_utf8(&piece) else {
                continue;
            };
            let Some(character) = UnicodeSegmentation::graphemes(piece, true).next() else {
                continue;
            };
            let next_text = format!("{predicted}{character}");
            if !is_allowed(&next_text) {
                continue;
            }
            best_value = value;
            best_character = Some(character.to_owned());
            best_next_text = next_text;
        }

        if let Some(max_entropy) = request.max_entropy
            && predicted_count >= min_length
            && sum_exp > 0.0
        {
            let entropy = sum_exp.ln() - sum_exp_value / sum_exp;
            if entropy >= max_entropy {
                break;
            }
        }
        let Some(character) = best_character else {
            break;
        };
        if matches!(character.as_str(), "、" | "。" | "！" | "？") && predicted_count >= min_length
        {
            break;
        }
        predicted = best_next_text;
        predicted_count += 1;
        let appended = tokenize_for_model(model, &character, false)?;
        if appended.is_empty() {
            break;
        }
        prompt_tokens.extend(appended);
    }

    Ok(predicted)
}

fn personalized_logits(
    logits: &[f32],
    candidate_prefix: &[i32],
    personalization: Option<&ZenzPersonalization<'_>>,
) -> Option<Vec<f32>> {
    let personalization = personalization.filter(|mode| mode.alpha > 0.0)?;
    if candidate_prefix.is_empty() {
        return None;
    }
    let base = personalization
        .base
        .probabilities(candidate_prefix, logits.len())?;
    let personal = personalization
        .personal
        .probabilities(candidate_prefix, logits.len())?;
    if base.len() != logits.len() || personal.len() != logits.len() {
        return None;
    }
    Some(
        logits
            .iter()
            .copied()
            .zip(base.into_iter().zip(personal))
            .map(|(logit, (base, personal))| {
                logit + personalization.alpha * ((personal + 1e-7).ln() - (base + 1e-7).ln())
            })
            .collect(),
    )
}

fn chosen_token_log_probability(row: &[f32], token: i32) -> Result<f64, ZenzInferenceError> {
    let index = usize::try_from(token)
        .ok()
        .filter(|&index| index < row.len())
        .ok_or_else(|| ZenzInferenceError::Message(format!("invalid comparison token {token}")))?;
    let maximum = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !maximum.is_finite() || row.iter().any(|value| value.is_nan()) {
        return Err(ZenzInferenceError::Message(
            "comparison logits do not define a probability distribution".into(),
        ));
    }
    let maximum = f64::from(maximum);
    let sum_exp = row
        .iter()
        .map(|&value| (f64::from(value) - maximum).exp())
        .sum::<f64>();
    Ok((f64::from(row[index]) - maximum) - sum_exp.ln())
}

fn strict_argmax(logits: &[f32]) -> usize {
    let mut maximum = 0;
    for index in 1..logits.len() {
        if logits[index] > logits[maximum] {
            maximum = index;
        }
    }
    maximum
}

pub(crate) fn normalize_for_model(value: &str) -> String {
    value.replace(' ', "\u{3000}").replace('\n', "")
}

pub(crate) fn tokenize_for_model(
    model: &mut dyn ZenzLanguageModel,
    value: &str,
    add_special: bool,
) -> Result<Vec<i32>, ZenzInferenceError> {
    model.tokenize(&normalize_for_model(value), add_special)
}

fn v3_conditions(config: &ZenzV3Config) -> Vec<String> {
    [
        (PROFILE_TAG, config.profile.as_deref()),
        (TOPIC_TAG, config.topic.as_deref()),
        (STYLE_TAG, config.style.as_deref()),
        (PREFERENCE_TAG, config.preference.as_deref()),
    ]
    .into_iter()
    .filter_map(|(tag, value)| {
        value
            .filter(|value| !value.is_empty())
            .map(|value| format!("{tag}{}", suffix(value, 25)))
    })
    .collect()
}

fn aligned_input(input: &str, cursor_position: Option<usize>, enabled: bool) -> String {
    if !enabled || !should_insert_alignment_separator(input, cursor_position) {
        return input.to_owned();
    }
    let cursor_position = cursor_position.expect("validated cursor position");
    let mut graphemes = UnicodeSegmentation::graphemes(input, true);
    let left = graphemes.by_ref().take(cursor_position).collect::<String>();
    format!(
        "{left}{ALIGNMENT_SEPARATOR}{}",
        graphemes.collect::<String>()
    )
}

fn should_insert_alignment_separator(input: &str, cursor_position: Option<usize>) -> bool {
    cursor_position
        .is_some_and(|position| position < UnicodeSegmentation::graphemes(input, true).count())
}

fn prefix(value: &str, count: usize) -> String {
    UnicodeSegmentation::graphemes(value, true)
        .take(count)
        .collect()
}

fn suffix(value: &str, count: usize) -> String {
    let graphemes = UnicodeSegmentation::graphemes(value, true).collect::<Vec<_>>();
    graphemes[graphemes.len().saturating_sub(count)..].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComposingCount, DictionaryEntry};

    enum Script {
        Pass,
        Fix,
        Eos,
    }

    struct ScriptedModel {
        script: Script,
    }

    struct InputPredictionModel {
        evaluations: usize,
    }

    struct BatchedModel {
        suffix_evaluations: usize,
        prompt_tokenizations: usize,
        start_indices: Vec<usize>,
    }

    struct FixedPrefixModel {
        tokenized: Vec<String>,
    }

    impl ZenzLanguageModel for InputPredictionModel {
        fn vocabulary_size(&self) -> usize {
            5
        }

        fn eos_token(&self) -> i32 {
            2
        }

        fn tokenize(
            &mut self,
            _text: &str,
            add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            Ok(if add_special { vec![1] } else { vec![3] })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(match token {
                3 => "カ".as_bytes().to_vec(),
                4 => "。".as_bytes().to_vec(),
                _ => Vec::new(),
            })
        }

        fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            self.evaluations += 1;
            Ok(if tokens.contains(&3) {
                vec![-10.0, -10.0, -10.0, 0.0, 10.0]
            } else {
                vec![-10.0, -10.0, -10.0, 10.0, 0.0]
            })
        }
    }

    impl ZenzLanguageModel for ScriptedModel {
        fn vocabulary_size(&self) -> usize {
            6
        }

        fn eos_token(&self) -> i32 {
            2
        }

        fn tokenize(
            &mut self,
            text: &str,
            _add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            Ok(if text == "ab" { vec![3, 4] } else { vec![1] })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(match token {
                3 => b"a".to_vec(),
                4 => b"b".to_vec(),
                5 => b"x".to_vec(),
                _ => Vec::new(),
            })
        }

        fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            let desired = if tokens == [1] {
                match self.script {
                    Script::Pass => 3,
                    Script::Fix => 5,
                    Script::Eos => 2,
                }
            } else {
                4
            };
            let mut logits = vec![-10.0; 6];
            logits[desired] = 2.0;
            Ok(logits)
        }
    }

    impl ZenzLanguageModel for BatchedModel {
        fn vocabulary_size(&self) -> usize {
            6
        }

        fn eos_token(&self) -> i32 {
            2
        }

        fn tokenize(
            &mut self,
            text: &str,
            add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            if add_special {
                self.prompt_tokenizations += 1;
                Ok(vec![1])
            } else if text == "ab" {
                Ok(vec![3, 4])
            } else {
                Ok(vec![1])
            }
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(match token {
                3 => b"a".to_vec(),
                4 => b"b".to_vec(),
                _ => Vec::new(),
            })
        }

        fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            panic!("candidate evaluation should use the batched suffix API")
        }

        fn logits_for_suffix(
            &mut self,
            tokens: &[i32],
            start_index: usize,
            sequence: ZenzInferenceSequence,
        ) -> Result<Vec<f32>, ZenzInferenceError> {
            self.suffix_evaluations += 1;
            assert_eq!(tokens, [1, 3]);
            self.start_indices.push(start_index);
            assert_eq!(sequence, ZenzInferenceSequence::Evaluation);
            let logits = vec![
                -10.0, -10.0, -10.0, 2.0, -10.0, -10.0, -10.0, -10.0, -10.0, -10.0, 2.0, -10.0,
            ];
            Ok(if start_index == 0 {
                logits
            } else {
                logits[6..].to_vec()
            })
        }
    }

    impl ZenzLanguageModel for FixedPrefixModel {
        fn vocabulary_size(&self) -> usize {
            6
        }

        fn eos_token(&self) -> i32 {
            2
        }

        fn tokenize(
            &mut self,
            text: &str,
            _add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            self.tokenized.push(text.to_owned());
            Ok(if text == "ab" { vec![3, 4] } else { vec![1] })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(match token {
                3 => b"a".to_vec(),
                4 => b"b".to_vec(),
                5 => b"x".to_vec(),
                _ => Vec::new(),
            })
        }

        fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            let desired = if tokens == [1] { 5 } else { 4 };
            let mut logits = vec![-10.0; 6];
            logits[desired] = 2.0;
            Ok(logits)
        }
    }

    fn evaluation_candidate() -> Candidate {
        Candidate::single(
            "ab".into(),
            -1.0,
            ComposingCount::Surface(1),
            500,
            vec![DictionaryEntry {
                word: "ab".into(),
                ruby: "エービー".into(),
                left_id: 0,
                right_id: 0,
                meaning_id: 500,
                base_value: -1.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            }],
        )
    }

    fn evaluate_script(script: Script) -> CandidateEvaluation {
        let mut model = ScriptedModel { script };
        let candidate = evaluation_candidate();
        evaluate_candidate(
            &mut model,
            ZenzEvaluationRequest {
                input: "エービー",
                input_cursor_position: None,
                candidate: &candidate,
                request_rich_candidates: false,
                prefix_constraint: &PrefixConstraint::default(),
                personalization: None,
                version: &ZenzVersionConfig::default(),
            },
        )
        .unwrap()
    }

    #[test]
    fn caches_incremental_evaluations_after_the_prompt_state_is_reused() {
        let mut model = BatchedModel {
            suffix_evaluations: 0,
            prompt_tokenizations: 0,
            start_indices: Vec::new(),
        };
        let candidate = evaluation_candidate();
        let mut evaluator = ZenzEvaluator::default();
        let evaluate = |evaluator: &mut ZenzEvaluator, model: &mut BatchedModel| {
            evaluator
                .evaluate(
                    model,
                    ZenzEvaluationRequest {
                        input: "エービー",
                        input_cursor_position: None,
                        candidate: &candidate,
                        request_rich_candidates: false,
                        prefix_constraint: &PrefixConstraint::default(),
                        personalization: None,
                        version: &ZenzVersionConfig::default(),
                    },
                )
                .unwrap()
        };

        let first = evaluate(&mut evaluator, &mut model);
        let second = evaluate(&mut evaluator, &mut model);
        let third = evaluate(&mut evaluator, &mut model);

        assert_eq!(first, second);
        assert_eq!(second, third);
        assert!(matches!(first, CandidateEvaluation::Pass { .. }));
        assert_eq!(model.suffix_evaluations, 2);
        assert_eq!(model.prompt_tokenizations, 1);
        assert_eq!(model.start_indices, [0, 0]);
    }

    #[test]
    fn fixed_prefix_evaluation_moves_text_to_context_and_restores_result() {
        let mut model = FixedPrefixModel {
            tokenized: Vec::new(),
        };
        let candidate = Candidate::single(
            "xab".into(),
            -1.0,
            ComposingCount::Surface(2),
            500,
            vec![
                DictionaryEntry {
                    word: "x".into(),
                    ruby: "エ".into(),
                    left_id: 0,
                    right_id: 0,
                    meaning_id: 500,
                    base_value: -1.0,
                    adjustment: 0.0,
                    metadata: DictionaryMetadata::default(),
                },
                DictionaryEntry {
                    word: "ab".into(),
                    ruby: "ービー".into(),
                    left_id: 0,
                    right_id: 0,
                    meaning_id: 500,
                    base_value: -1.0,
                    adjustment: 0.0,
                    metadata: DictionaryMetadata::default(),
                },
            ],
        );
        let constraint = PrefixConstraint {
            bytes: b"xab".to_vec(),
            has_eos: false,
            ignore_memory_and_user_dictionary: false,
            fixed_prefix: Some(FixedPrefix {
                byte_count: 1,
                ruby_count: 1,
            }),
        };
        let version = ZenzVersionConfig::V3(ZenzV3Config {
            left_context: Some("ctx".into()),
            ..Default::default()
        });

        let result = evaluate_candidate(
            &mut model,
            ZenzEvaluationRequest {
                input: "エービー",
                input_cursor_position: None,
                candidate: &candidate,
                request_rich_candidates: false,
                prefix_constraint: &constraint,
                personalization: None,
                version: &version,
            },
        )
        .unwrap();

        assert_eq!(result, CandidateEvaluation::FixRequired(b"xx".to_vec()));
        assert!(model.tokenized.iter().any(|value| value.contains("ctxx")));
        assert!(model.tokenized.iter().any(|value| value == "ab"));
    }

    #[test]
    fn fixed_prefix_keeps_an_entry_crossing_the_boundary() {
        let entries = vec![
            DictionaryEntry {
                word: "xy".into(),
                ruby: "エ".into(),
                left_id: 0,
                right_id: 0,
                meaning_id: 500,
                base_value: -1.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            },
            DictionaryEntry {
                word: "z".into(),
                ruby: "ゼ".into(),
                left_id: 0,
                right_id: 0,
                meaning_id: 500,
                base_value: -1.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            },
        ];

        let remaining = dropping_fixed_entries(&entries, 1);

        assert_eq!(remaining, entries);
    }

    #[test]
    fn fixed_prefix_is_restored_to_every_evaluation_result() {
        let pass = prepend_fixed(
            b"x",
            "x",
            CandidateEvaluation::Pass {
                score: 1.0,
                alternatives: vec![AlternativeConstraint {
                    probability_ratio: 0.5,
                    bytes: b"ab".to_vec(),
                }],
            },
        );
        assert_eq!(
            pass,
            CandidateEvaluation::Pass {
                score: 1.0,
                alternatives: vec![AlternativeConstraint {
                    probability_ratio: 0.5,
                    bytes: b"xab".to_vec(),
                }],
            }
        );
        assert_eq!(
            prepend_fixed(b"x", "x", CandidateEvaluation::FixRequired(b"y".to_vec())),
            CandidateEvaluation::FixRequired(b"xy".to_vec())
        );
        assert_eq!(
            prepend_fixed(b"x", "x", CandidateEvaluation::WholeResult("y".into())),
            CandidateEvaluation::WholeResult("xy".into())
        );
    }

    #[test]
    fn skips_the_model_prefix_already_addressed_by_the_constraint() {
        let mut model = BatchedModel {
            suffix_evaluations: 0,
            prompt_tokenizations: 0,
            start_indices: Vec::new(),
        };
        let candidate = evaluation_candidate();
        let mut evaluator = ZenzEvaluator::default();
        let version = ZenzVersionConfig::default();

        for constraint in [
            PrefixConstraint::default(),
            PrefixConstraint::new(b"a".to_vec()),
        ] {
            let result = evaluator
                .evaluate(
                    &mut model,
                    ZenzEvaluationRequest {
                        input: "エービー",
                        input_cursor_position: None,
                        candidate: &candidate,
                        request_rich_candidates: false,
                        prefix_constraint: &constraint,
                        personalization: None,
                        version: &version,
                    },
                )
                .unwrap();
            assert!(matches!(result, CandidateEvaluation::Pass { .. }));
        }

        assert_eq!(model.start_indices, [0, 1]);
    }

    #[test]
    fn normalizes_every_model_input_like_the_upstream_context() {
        assert_eq!(normalize_for_model("a b\nc\r"), "a\u{3000}bc\r");
    }

    #[test]
    fn builds_v3_input_prediction_prompt() {
        let config = ZenzVersionConfig::V3(ZenzV3Config {
            profile: Some("profile".into()),
            topic: Some("topic".into()),
            style: Some("style".into()),
            preference: Some("preference".into()),
            right_context: Some("uvwxyz".into()),
            max_left_context_length: Some(2),
            max_right_context_length: Some(3),
            ..Default::default()
        });
        assert_eq!(
            ZenzPromptBuilder::input_prediction("abcdef", "かんじ", &config).unwrap(),
            "\u{EE03}profile\u{EE04}topic\u{EE05}style\u{EE06}preference\u{EE02}ef\u{EE07}uvw\u{EE00}カンジ"
        );
    }

    #[test]
    fn builds_v2_candidate_evaluation_prompt() {
        let config = ZenzVersionConfig::V2(ZenzV2Config {
            profile: Some("profile".into()),
            left_context: Some("abcdef".into()),
            max_left_context_length: Some(3),
        });
        assert_eq!(
            ZenzPromptBuilder::candidate_evaluation("ヘンカン", None, "単語(たんご)", &config),
            "\u{EE00}ヘンカン\u{EE02}辞書:単語(たんご)・プロフィール:profile・発言:def\u{EE01}"
        );
    }

    #[test]
    fn inserts_alignment_separator_only_for_an_internal_cursor() {
        let config = ZenzVersionConfig::V3(ZenzV3Config {
            enable_alignment_separator: true,
            ..Default::default()
        });
        assert_eq!(
            ZenzPromptBuilder::candidate_evaluation("ハシ", Some(1), "", &config),
            "\u{EE00}ハ\u{EE08}シ\u{EE01}"
        );
        assert_eq!(
            ZenzPromptBuilder::candidate_text_for_evaluation("葉", "ハシ", Some(1), &config),
            "葉\u{EE08}"
        );
        assert_eq!(
            ZenzPromptBuilder::candidate_evaluation("ハシ", Some(2), "", &config),
            "\u{EE00}ハシ\u{EE01}"
        );
    }

    #[test]
    fn generates_v3_input_until_punctuation_after_the_minimum_length() {
        let mut model = InputPredictionModel { evaluations: 0 };
        let prediction = generate_next_input(
            &mut model,
            ZenzInputGenerationRequest {
                left_context: "今日は",
                composing_text: "",
                count: 10,
                min_length: 1,
                max_entropy: Some(3.0),
                version: &ZenzVersionConfig::default(),
                possible_nexts: &[],
            },
        )
        .unwrap();
        assert_eq!(prediction, "カ");
        assert_eq!(model.evaluations, 2);
    }

    #[test]
    fn constrains_generated_input_to_pending_roman_prefixes() {
        let mut model = InputPredictionModel { evaluations: 0 };
        let prediction = generate_next_input(
            &mut model,
            ZenzInputGenerationRequest {
                left_context: "",
                composing_text: "",
                count: 10,
                min_length: 1,
                max_entropy: None,
                version: &ZenzVersionConfig::default(),
                possible_nexts: &["キ".into()],
            },
        )
        .unwrap();
        assert!(prediction.is_empty());
    }

    #[test]
    fn builds_context_only_prompts() {
        assert_eq!(ZenzPromptBuilder::typo_correction_prefix(""), "\u{EE00}");
        assert_eq!(
            ZenzPromptBuilder::candidate_evaluation(
                "ハシ",
                None,
                "",
                &ZenzVersionConfig::V3(ZenzV3Config {
                    right_context: Some("abcdef".into()),
                    max_right_context_length: Some(2),
                    ..Default::default()
                })
            ),
            "\u{EE07}ab\u{EE00}ハシ\u{EE01}"
        );
    }

    #[test]
    fn alignment_separator_turns_a_prefix_into_an_eos_constraint() {
        let constraint =
            PrefixConstraint::normalized("橋\u{EE08}ignored".as_bytes().to_vec(), false, true);
        assert_eq!(constraint.bytes, "橋".as_bytes());
        assert!(constraint.has_eos);
        assert!(constraint.ignore_memory_and_user_dictionary);
    }

    #[test]
    fn evaluates_candidate_tokens_until_they_pass() {
        assert_eq!(
            evaluate_script(Script::Pass),
            CandidateEvaluation::Pass {
                score: 4.0,
                alternatives: Vec::new(),
            }
        );
    }

    #[test]
    fn returns_the_model_selected_utf8_prefix() {
        assert_eq!(
            evaluate_script(Script::Fix),
            CandidateEvaluation::FixRequired(b"x".to_vec())
        );
    }

    #[test]
    fn turns_eos_into_a_whole_result() {
        assert_eq!(
            evaluate_script(Script::Eos),
            CandidateEvaluation::WholeResult(String::new())
        );
    }
}

#[cfg(test)]
#[path = "zenz_comparison_tests.rs"]
mod comparison_tests;
