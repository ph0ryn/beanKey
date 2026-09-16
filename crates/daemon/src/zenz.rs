use std::error::Error;
use std::fmt;
use std::path::Path;

use beankey_converter::{
    Candidate, CandidateEvaluation, ConversionSession, DictionaryError, DictionaryMetadata,
    EfficientNGram, InputTableRegistry, NGramError, NormalConverter, PrefixConstraint,
    ZenzEvaluationRequest, ZenzEvaluator, ZenzInferenceError, ZenzInferenceSequence,
    ZenzLanguageModel, ZenzPersonalization, ZenzVersionConfig, to_katakana,
};
use beankey_llama::{LlamaContext, LlamaError, LlamaSequence};

pub const DEFAULT_INFERENCE_LIMIT: usize = 10;
const PERSONALIZATION_N: usize = 5;
const PERSONALIZATION_DISCOUNT: f64 = 0.75;

pub(crate) struct ZenzPersonalizationModels {
    alpha: f32,
    base: EfficientNGram,
    personal: EfficientNGram,
}

pub(crate) struct ZenzConversionOptions<'a> {
    pub(crate) version: &'a ZenzVersionConfig,
    pub(crate) request_rich_candidates: bool,
    pub(crate) inference_limit: usize,
    pub(crate) personalization: Option<&'a ZenzPersonalizationModels>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ZenzConversionCache {
    input: String,
    constraint: PrefixConstraint,
    satisfying_candidate: Option<Candidate>,
    evaluated_satisfying_candidate: Option<Candidate>,
}

impl ZenzConversionCache {
    fn constraint_for(&self, input: &str) -> PrefixConstraint {
        if let Some(candidate) = &self.satisfying_candidate {
            let mut remaining = input;
            let mut bytes = Vec::new();
            for entry in &candidate.entries {
                let Some(next) = remaining.strip_prefix(&entry.ruby) else {
                    break;
                };
                bytes.extend_from_slice(entry.word.as_bytes());
                remaining = next;
            }
            return PrefixConstraint::new(bytes);
        }
        if input.starts_with(&self.input) {
            return PrefixConstraint {
                bytes: self.constraint.bytes.clone(),
                has_eos: false,
                ignore_memory_and_user_dictionary: self
                    .constraint
                    .ignore_memory_and_user_dictionary,
            };
        }
        PrefixConstraint::default()
    }

    fn update(
        &mut self,
        input: String,
        constraint: PrefixConstraint,
        satisfying_candidate: Option<Candidate>,
        evaluated_satisfying_candidate: Option<Candidate>,
    ) {
        self.input = input;
        self.constraint = constraint;
        self.satisfying_candidate = satisfying_candidate;
        self.evaluated_satisfying_candidate = evaluated_satisfying_candidate;
    }
}

impl ZenzPersonalizationModels {
    pub(crate) fn load(
        base: impl AsRef<Path>,
        personal: impl AsRef<Path>,
        alpha: f32,
    ) -> Result<Self, NGramError> {
        Ok(Self {
            alpha,
            base: EfficientNGram::open(base, PERSONALIZATION_N, PERSONALIZATION_DISCOUNT)?,
            personal: EfficientNGram::open(personal, PERSONALIZATION_N, PERSONALIZATION_DISCOUNT)?,
        })
    }

    fn request(&self) -> ZenzPersonalization<'_> {
        ZenzPersonalization {
            alpha: self.alpha,
            base: &self.base,
            personal: &self.personal,
        }
    }
}

pub struct LlamaModel {
    context: LlamaContext,
}

impl LlamaModel {
    pub fn load(
        model_path: impl AsRef<Path>,
        backend_directory: impl AsRef<Path>,
    ) -> Result<Self, LlamaError> {
        Ok(Self {
            context: LlamaContext::load(model_path, backend_directory)?,
        })
    }
}

impl ZenzLanguageModel for LlamaModel {
    fn vocabulary_size(&self) -> usize {
        self.context.vocabulary_size()
    }

    fn eos_token(&self) -> i32 {
        self.context.eos_token()
    }

    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        self.context
            .tokenize(text, add_special)
            .map_err(inference_error)
    }

    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        self.context.token_to_piece(token).map_err(inference_error)
    }

    fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        self.context.next_logits(tokens).map_err(inference_error)
    }

    fn logits_for_suffix(
        &mut self,
        tokens: &[i32],
        start_index: usize,
        sequence: ZenzInferenceSequence,
    ) -> Result<Vec<f32>, ZenzInferenceError> {
        let sequence = match sequence {
            ZenzInferenceSequence::Evaluation => LlamaSequence::Evaluation,
            ZenzInferenceSequence::InputPrediction => LlamaSequence::InputPrediction,
        };
        self.context
            .logits(tokens, start_index, sequence)
            .map_err(inference_error)
    }
}

fn inference_error(error: LlamaError) -> ZenzInferenceError {
    ZenzInferenceError(error.to_string())
}

#[derive(Debug)]
pub enum ZenzConversionError {
    Dictionary(DictionaryError),
    Inference(ZenzInferenceError),
}

impl fmt::Display for ZenzConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dictionary(error) => error.fmt(formatter),
            Self::Inference(error) => error.fmt(formatter),
        }
    }
}

impl Error for ZenzConversionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dictionary(error) => Some(error),
            Self::Inference(error) => Some(error),
        }
    }
}

impl From<DictionaryError> for ZenzConversionError {
    fn from(value: DictionaryError) -> Self {
        Self::Dictionary(value)
    }
}

impl From<ZenzInferenceError> for ZenzConversionError {
    fn from(value: ZenzInferenceError) -> Self {
        Self::Inference(value)
    }
}

pub fn convert(
    session: &mut ConversionSession,
    converter: &NormalConverter<'_>,
    tables: &InputTableRegistry,
    model: &mut dyn ZenzLanguageModel,
    evaluator: &mut ZenzEvaluator,
    cache: &mut ZenzConversionCache,
    options: ZenzConversionOptions<'_>,
) -> Result<(), ZenzConversionError> {
    let lattice_input = to_katakana(&session.composing().surface());
    let model_input = to_katakana(&session.zenz_model_composing().surface());
    let input_cursor_position = Some(session.zenz_model_composing().cursor());
    let defers_evaluation_for_pending_input = !options.request_rich_candidates
        && options.personalization.is_none()
        && session.pending_zenz_suffix_count(tables) > 0;
    let mut constraint = cache.constraint_for(&lattice_input);
    let mut constructed_candidates = Vec::<Candidate>::new();
    let mut inserted_candidates = Vec::<Candidate>::new();
    let mut remaining_inferences = options.inference_limit;

    loop {
        let draft = session
            .request_zenz_draft(
                converter,
                tables,
                if constraint.is_empty() {
                    2
                } else if options.request_rich_candidates {
                    3
                } else {
                    1
                },
                &constraint,
            )?
            .to_vec();
        constructed_candidates.extend(draft.iter().cloned());
        let Some((mut candidate_index, mut candidate)) = best_candidate(&draft) else {
            session.set_zenz_candidates(inserted_candidates);
            cache.update(lattice_input, PrefixConstraint::default(), None, None);
            return Ok(());
        };
        'review: loop {
            inserted_candidates.insert(0, candidate.clone());
            if remaining_inferences == 0 {
                cache.update(lattice_input, constraint, Some(candidate), None);
                session.set_zenz_candidates(inserted_candidates);
                return Ok(());
            }
            if defers_evaluation_for_pending_input {
                let evaluated = (!constraint.is_empty())
                    .then(|| cache.evaluated_satisfying_candidate.clone())
                    .flatten();
                cache.update(lattice_input, constraint, evaluated.clone(), evaluated);
                session.set_zenz_candidates(inserted_candidates);
                return Ok(());
            }

            let evaluation = evaluator.evaluate(
                model,
                ZenzEvaluationRequest {
                    input: &model_input,
                    input_cursor_position,
                    candidate: &candidate,
                    request_rich_candidates: options.request_rich_candidates,
                    prefix_constraint: &constraint,
                    personalization: options
                        .personalization
                        .map(ZenzPersonalizationModels::request),
                    version: options.version,
                },
            )?;
            remaining_inferences -= 1;
            match evaluation {
                CandidateEvaluation::Pass { alternatives, .. } => {
                    if options.request_rich_candidates {
                        insert_rich_candidates(
                            session,
                            converter,
                            tables,
                            &mut inserted_candidates,
                            &constructed_candidates,
                            alternatives,
                        )?;
                    }
                    cache.update(
                        lattice_input,
                        constraint,
                        Some(candidate.clone()),
                        Some(candidate),
                    );
                    session.set_zenz_candidates(inserted_candidates);
                    return Ok(());
                }
                CandidateEvaluation::FixRequired(bytes) => {
                    let next = PrefixConstraint::normalized(
                        bytes,
                        false,
                        constraint.ignore_memory_and_user_dictionary,
                    );
                    match review_rejection(
                        &mut constraint,
                        next,
                        &draft,
                        candidate_index,
                        &candidate,
                    ) {
                        ReviewAction::Fail => {
                            cache.update(lattice_input, PrefixConstraint::default(), None, None);
                            session.set_zenz_candidates(inserted_candidates);
                            return Ok(());
                        }
                        ReviewAction::Retry(index) => {
                            candidate_index = index;
                            candidate = draft[index].clone();
                            continue 'review;
                        }
                        ReviewAction::Research => break 'review,
                    }
                }
                CandidateEvaluation::WholeResult(result) => {
                    let next = PrefixConstraint::normalized(
                        result.into_bytes(),
                        true,
                        constraint.ignore_memory_and_user_dictionary,
                    );
                    match review_rejection(
                        &mut constraint,
                        next,
                        &draft,
                        candidate_index,
                        &candidate,
                    ) {
                        ReviewAction::Fail => {
                            cache.update(lattice_input, PrefixConstraint::default(), None, None);
                            session.set_zenz_candidates(inserted_candidates);
                            return Ok(());
                        }
                        ReviewAction::Retry(index) => {
                            candidate_index = index;
                            candidate = draft[index].clone();
                            continue 'review;
                        }
                        ReviewAction::Research => break 'review,
                    }
                }
            }
        }
    }
}

fn best_candidate(candidates: &[Candidate]) -> Option<(usize, Candidate)> {
    candidates
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.value.total_cmp(&right.value))
        .map(|(index, candidate)| (index, candidate.clone()))
}

enum ReviewAction {
    Fail,
    Retry(usize),
    Research,
}

fn review_rejection(
    constraint: &mut PrefixConstraint,
    next: PrefixConstraint,
    candidates: &[Candidate],
    candidate_index: usize,
    candidate: &Candidate,
) -> ReviewAction {
    if *constraint == next {
        if !constraint.ignore_memory_and_user_dictionary
            && candidate_uses_personal_dictionary(candidate)
        {
            constraint.ignore_memory_and_user_dictionary = true;
            return retry_candidate(candidates, candidate_index, &next)
                .map_or(ReviewAction::Research, ReviewAction::Retry);
        }
        return ReviewAction::Fail;
    }

    let incremental = next.bytes.starts_with(&constraint.bytes);
    *constraint = next;
    if incremental && let Some(index) = retry_candidate(candidates, candidate_index, constraint) {
        return ReviewAction::Retry(index);
    }
    ReviewAction::Research
}

fn retry_candidate(
    candidates: &[Candidate],
    current_index: usize,
    constraint: &PrefixConstraint,
) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .find_map(|(index, candidate)| {
            (index != current_index
                && candidate_satisfies(candidate, constraint)
                && heuristic_retry_validation(candidate))
            .then_some(index)
        })
}

fn heuristic_retry_validation(candidate: &Candidate) -> bool {
    !candidate.text.contains(['\u{3099}', '\u{309a}'])
}

fn insert_rich_candidates(
    session: &mut ConversionSession,
    converter: &NormalConverter<'_>,
    tables: &InputTableRegistry,
    inserted: &mut Vec<Candidate>,
    constructed: &[Candidate],
    alternatives: Vec<beankey_converter::AlternativeConstraint>,
) -> Result<(), DictionaryError> {
    for alternative in alternatives
        .into_iter()
        .rev()
        .filter(|alternative| alternative.probability_ratio > 0.25)
    {
        let constraint = PrefixConstraint::normalized(alternative.bytes, false, false);
        let existing = constructed
            .iter()
            .filter(|candidate| candidate_satisfies(candidate, &constraint))
            .max_by(|left, right| left.value.total_cmp(&right.value))
            .cloned();
        let candidate = if existing.is_some() {
            existing
        } else if alternative.probability_ratio > 0.5 {
            best_candidate(session.request_zenz_draft(converter, tables, 3, &constraint)?)
                .map(|(_, candidate)| candidate)
        } else {
            None
        };
        if let Some(candidate) = candidate {
            inserted.insert(1.min(inserted.len()), candidate);
        }
    }
    Ok(())
}

fn candidate_satisfies(candidate: &Candidate, constraint: &PrefixConstraint) -> bool {
    if constraint.has_eos {
        candidate.text.as_bytes() == constraint.bytes
    } else {
        candidate.text.as_bytes().starts_with(&constraint.bytes)
    }
}

fn candidate_uses_personal_dictionary(candidate: &Candidate) -> bool {
    candidate.entries.iter().any(|entry| {
        entry.metadata.contains(DictionaryMetadata::LEARNED)
            || entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY)
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use beankey_converter::{
        ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
        ZenzEvaluator, ZenzInferenceError, ZenzLanguageModel, ZenzV3Config, ZenzVersionConfig,
    };

    use super::{
        ReviewAction, ZenzConversionCache, ZenzConversionOptions, convert, review_rejection,
    };

    struct PrefixModel {
        evaluations: usize,
        prompts: Vec<String>,
    }

    struct PassModel {
        evaluations: usize,
    }

    impl ZenzLanguageModel for PassModel {
        fn vocabulary_size(&self) -> usize {
            4
        }

        fn eos_token(&self) -> i32 {
            2
        }

        fn tokenize(
            &mut self,
            _text: &str,
            add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            Ok(vec![if add_special { 1 } else { 3 }])
        }

        fn token_to_piece(&mut self, _token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(b"x".to_vec())
        }

        fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            self.evaluations += 1;
            Ok(vec![0.0, 0.0, 0.0, 10.0])
        }
    }

    impl ZenzLanguageModel for PrefixModel {
        fn vocabulary_size(&self) -> usize {
            5
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
                self.prompts.push(text.to_owned());
            }
            Ok(if add_special {
                vec![1]
            } else if text == "箸" {
                vec![4]
            } else {
                vec![3]
            })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(match token {
                4 => "箸".as_bytes().to_vec(),
                _ => b"x".to_vec(),
            })
        }

        fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            self.evaluations += 1;
            Ok(vec![0.0, 0.0, 0.0, 1.0, 10.0])
        }
    }

    fn dictionary_root() -> PathBuf {
        PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        )
    }

    fn session() -> (DictionaryStore, InputTableRegistry, ConversionSession) {
        let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("はし", InputStyle::Direct, &tables);
        (dictionary, tables, session)
    }

    #[test]
    fn constrains_a_retried_draft_to_the_model_prefix() {
        let (dictionary, tables, mut session) = session();
        let converter = NormalConverter::new(&dictionary);
        let mut model = PrefixModel {
            evaluations: 0,
            prompts: Vec::new(),
        };
        let mut evaluator = ZenzEvaluator::default();
        let mut cache = ZenzConversionCache::default();

        convert(
            &mut session,
            &converter,
            &tables,
            &mut model,
            &mut evaluator,
            &mut cache,
            ZenzConversionOptions {
                version: &ZenzVersionConfig::default(),
                request_rich_candidates: false,
                inference_limit: 10,
                personalization: None,
            },
        )
        .unwrap();

        assert_eq!(session.candidates()[0].text, "箸");
        assert_eq!(model.evaluations, 1);
        assert_eq!(cache.constraint_for("ハシデ").bytes, "箸".as_bytes());
    }

    #[test]
    fn returns_the_initial_draft_when_the_inference_limit_is_zero() {
        let (dictionary, tables, mut session) = session();
        let converter = NormalConverter::new(&dictionary);
        let mut model = PrefixModel {
            evaluations: 0,
            prompts: Vec::new(),
        };
        let mut evaluator = ZenzEvaluator::default();
        let mut cache = ZenzConversionCache::default();

        convert(
            &mut session,
            &converter,
            &tables,
            &mut model,
            &mut evaluator,
            &mut cache,
            ZenzConversionOptions {
                version: &ZenzVersionConfig::default(),
                request_rich_candidates: false,
                inference_limit: 0,
                personalization: None,
            },
        )
        .unwrap();

        assert!(!session.candidates().is_empty());
        assert_eq!(model.evaluations, 0);
    }

    #[test]
    fn includes_the_internal_composition_cursor_in_the_zenz_prompt() {
        let (dictionary, tables, mut session) = session();
        session.move_cursor(-1);
        session.begin_segment_request(&tables);
        let converter = NormalConverter::new(&dictionary);
        let mut model = PrefixModel {
            evaluations: 0,
            prompts: Vec::new(),
        };
        let mut evaluator = ZenzEvaluator::default();
        let mut cache = ZenzConversionCache::default();

        convert(
            &mut session,
            &converter,
            &tables,
            &mut model,
            &mut evaluator,
            &mut cache,
            ZenzConversionOptions {
                version: &ZenzVersionConfig::V3(ZenzV3Config {
                    enable_alignment_separator: true,
                    ..Default::default()
                }),
                request_rich_candidates: false,
                inference_limit: 1,
                personalization: None,
            },
        )
        .unwrap();
        session.end_segment_request();

        assert!(
            model
                .prompts
                .iter()
                .any(|prompt| prompt.contains("\u{ee00}ハ\u{ee08}シ\u{ee01}"))
        );
    }

    #[test]
    fn repeated_model_constraint_is_not_cached_as_satisfied() {
        let (dictionary, tables, mut session) = session();
        let converter = NormalConverter::new(&dictionary);
        let candidates = session
            .request_zenz_draft(
                &converter,
                &tables,
                2,
                &beankey_converter::PrefixConstraint::default(),
            )
            .unwrap()
            .to_vec();
        let candidate = candidates[0].clone();
        let mut constraint = beankey_converter::PrefixConstraint::new(b"x".to_vec());

        let action = review_rejection(
            &mut constraint,
            beankey_converter::PrefixConstraint::new(b"x".to_vec()),
            &candidates,
            0,
            &candidate,
        );

        assert!(matches!(action, ReviewAction::Fail));
    }

    #[test]
    fn pending_roman_suffix_does_not_replace_the_last_evaluated_candidate() {
        let (dictionary, tables, mut session) = session();
        let converter = NormalConverter::new(&dictionary);
        let mut model = PassModel { evaluations: 0 };
        let mut evaluator = ZenzEvaluator::default();
        let mut cache = ZenzConversionCache::default();
        let version = ZenzVersionConfig::default();
        let options = || ZenzConversionOptions {
            version: &version,
            request_rich_candidates: false,
            inference_limit: 5,
            personalization: None,
        };

        convert(
            &mut session,
            &converter,
            &tables,
            &mut model,
            &mut evaluator,
            &mut cache,
            options(),
        )
        .unwrap();
        let evaluated = cache.evaluated_satisfying_candidate.clone().unwrap();
        let evaluations = model.evaluations;

        session.insert_str("n", InputStyle::RomanToKana, &tables);
        assert_eq!(session.pending_zenz_suffix_count(&tables), 1);
        convert(
            &mut session,
            &converter,
            &tables,
            &mut model,
            &mut evaluator,
            &mut cache,
            options(),
        )
        .unwrap();

        assert_eq!(model.evaluations, evaluations);
        assert_eq!(
            cache
                .evaluated_satisfying_candidate
                .as_ref()
                .map(|candidate| &candidate.text),
            Some(&evaluated.text)
        );
    }
}
