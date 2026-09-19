use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beankey_converter::{
    Candidate as ConverterCandidate, ComposingCount as ConverterComposingCount, ConversionResult,
    ConversionSession, DictionaryEntry, DictionaryError, DictionaryMetadata, DictionaryStore,
    ForeignCompletionProvider, FormatReport, HunspellCompleter, HunspellError, InputModifier,
    InputStyle as ConverterInputStyle, InputTable, InputTableId, InputTableRegistry,
    KeyboardLanguage, LearningError, LearningMemory, LearningMode, LmTypoConfig, NGramError,
    NGramLanguageModel, NormalConverter, PredictionMode, RequestOptions, SelectionError,
    TextReplacer, TextReplacerError, TypoCorrectionMode, ZenzEvaluator, ZenzLanguageModel,
    ZenzV3Config, ZenzVersionConfig, experimental_typo_correction, to_full_width, to_hiragana,
    to_katakana,
};
use beankey_llama::LlamaError;
use serde::Deserialize;

use crate::config::{
    ConversionConfig, DaemonConfig, InputStyleConfig, KeyboardLanguageConfig, LearningConfig,
    LearningModeConfig, LmTypoLanguageModel, PredictionConfig, PunctuationStyleConfig,
    TypoCorrectionConfig,
};
use crate::protocol::composing_count::Count;
use crate::protocol::envelope::Payload;
use crate::protocol::protocol_error::Code;
use crate::zenz;
use crate::{LlamaModel, PROTOCOL_VERSION, protocol};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InputMode {
    #[default]
    None,
    Composing,
    Previewing,
    Selecting,
    Unicode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InputLanguage {
    #[default]
    Japanese,
    English,
}

#[derive(Clone, Debug)]
struct InputPrediction {
    display_text: String,
    append_text: String,
    delete_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct InputBehavior {
    type_backslash: bool,
    type_half_space: bool,
    option_direct_full_width_input: bool,
    punctuation_style: PunctuationStyleConfig,
}

struct SessionState {
    conversion: ConversionSession,
    input_style: ConverterInputStyle,
    last_request_id: u64,
    selected_candidate: usize,
    input_mode: InputMode,
    display_candidates: Vec<ConverterCandidate>,
    candidate_remainders: Vec<String>,
    candidate_annotations: Vec<String>,
    preview_candidate_index: Option<usize>,
    additional_candidate_count: usize,
    segment_surface_count: Option<usize>,
    last_was_backspace: bool,
    prediction: Option<InputPrediction>,
    live_conversion_enabled: bool,
    surrounding: SurroundingContext,
    request_options: RequestOptions,
    live_candidate: Option<beankey_converter::Candidate>,
    typo_corrections: Vec<beankey_converter::LmTypoCandidate>,
    lm_typo_available: bool,
    learning_available: bool,
    learning_writable: bool,
    zenz_cache: zenz::ZenzConversionCache,
    unicode_input: String,
    input_language: InputLanguage,
}

impl SessionState {
    fn clear_presentation(&mut self) {
        self.selected_candidate = 0;
        self.display_candidates.clear();
        self.candidate_remainders.clear();
        self.candidate_annotations.clear();
        self.preview_candidate_index = None;
        self.additional_candidate_count = 0;
        self.prediction = None;
        self.live_candidate = None;
    }

    fn reset(&mut self) {
        self.conversion.reset();
        self.input_mode = InputMode::None;
        self.segment_surface_count = None;
        self.last_was_backspace = false;
        self.clear_presentation();
        self.typo_corrections.clear();
        self.zenz_cache = zenz::ZenzConversionCache::default();
        self.unicode_input.clear();
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SurroundingContext {
    left: Option<String>,
    right: Option<String>,
}

type SessionRequestResult<T> = Result<T, (Code, String)>;

const DESKTOP_PROPER_NOUN_CID: u16 = 1288;
const DESKTOP_GENERAL_MID: u16 = 501;
const DESKTOP_USER_DICTIONARY_VALUE: f32 = -5.0;

pub struct Engine {
    dictionary: DictionaryStore,
    tables: InputTableRegistry,
    sessions: HashMap<String, SessionState>,
    zenz_model: Option<Box<dyn ZenzLanguageModel>>,
    zenz_evaluator: ZenzEvaluator,
    zenz_version: ZenzVersionConfig,
    zenz_rich_candidates: bool,
    zenz_inference_limit: usize,
    zenz_predictive_input: bool,
    zenz_personalization: Option<zenz::ZenzPersonalizationModels>,
    lm_typo_enabled: bool,
    lm_typo_language_model: LmTypoLanguageModel,
    lm_typo_config: LmTypoConfig,
    lm_typo_ngram: Option<NGramLanguageModel>,
    foreign_completion_provider: Option<Arc<dyn ForeignCompletionProvider>>,
    learning_memory: Option<LearningMemory>,
    learning_available: bool,
    learning_writable: bool,
    text_replacer: Option<TextReplacer>,
    user_dictionary: Vec<DictionaryEntry>,
    user_dictionary_directory: Option<PathBuf>,
    request_options: RequestOptions,
    live_conversion: bool,
    default_input_style: ConverterInputStyle,
    default_keyboard_language: KeyboardLanguage,
    input_behavior: InputBehavior,
}

#[derive(Debug)]
pub enum EngineOpenError {
    Dictionary(DictionaryError),
    Llama(LlamaError),
    Hunspell(HunspellError),
    Learning(LearningError),
    TextReplacer(TextReplacerError),
    ConversionResource(ConversionResourceError),
    NGram(NGramError),
}

impl fmt::Display for EngineOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dictionary(error) => error.fmt(formatter),
            Self::Llama(error) => error.fmt(formatter),
            Self::Hunspell(error) => error.fmt(formatter),
            Self::Learning(error) => error.fmt(formatter),
            Self::TextReplacer(error) => error.fmt(formatter),
            Self::ConversionResource(error) => error.fmt(formatter),
            Self::NGram(error) => error.fmt(formatter),
        }
    }
}

impl Error for EngineOpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dictionary(error) => Some(error),
            Self::Llama(error) => Some(error),
            Self::Hunspell(error) => Some(error),
            Self::Learning(error) => Some(error),
            Self::TextReplacer(error) => Some(error),
            Self::ConversionResource(error) => Some(error),
            Self::NGram(error) => Some(error),
        }
    }
}

impl From<DictionaryError> for EngineOpenError {
    fn from(value: DictionaryError) -> Self {
        Self::Dictionary(value)
    }
}

impl From<LlamaError> for EngineOpenError {
    fn from(value: LlamaError) -> Self {
        Self::Llama(value)
    }
}

impl From<HunspellError> for EngineOpenError {
    fn from(value: HunspellError) -> Self {
        Self::Hunspell(value)
    }
}

impl From<LearningError> for EngineOpenError {
    fn from(value: LearningError) -> Self {
        Self::Learning(value)
    }
}

impl From<TextReplacerError> for EngineOpenError {
    fn from(value: TextReplacerError) -> Self {
        Self::TextReplacer(value)
    }
}

impl From<ConversionResourceError> for EngineOpenError {
    fn from(value: ConversionResourceError) -> Self {
        Self::ConversionResource(value)
    }
}

impl From<NGramError> for EngineOpenError {
    fn from(value: NGramError) -> Self {
        Self::NGram(value)
    }
}

#[derive(Debug)]
pub enum ConversionResourceError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    UserDictionary(serde_json::Error),
    InvalidInputTable {
        name: String,
    },
    Dictionary(DictionaryError),
}

impl fmt::Display for ConversionResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::UserDictionary(error) => write!(formatter, "invalid user dictionary: {error}"),
            Self::InvalidInputTable { name } => write!(formatter, "invalid input table {name}"),
            Self::Dictionary(error) => error.fmt(formatter),
        }
    }
}

impl Error for ConversionResourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::UserDictionary(error) => Some(error),
            Self::Dictionary(error) => Some(error),
            Self::InvalidInputTable { .. } => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserDictionaryItem {
    word: String,
    reading: String,
    #[serde(default)]
    #[serde(rename = "hint")]
    _hint: Option<String>,
}

impl Engine {
    pub fn open(dictionary_path: impl AsRef<Path>) -> Result<Self, DictionaryError> {
        Ok(Self {
            dictionary: DictionaryStore::open(dictionary_path.as_ref().to_path_buf())?,
            tables: InputTableRegistry::new(),
            sessions: HashMap::new(),
            zenz_model: None,
            zenz_evaluator: ZenzEvaluator::default(),
            zenz_version: ZenzVersionConfig::default(),
            zenz_rich_candidates: false,
            zenz_inference_limit: zenz::DEFAULT_INFERENCE_LIMIT,
            zenz_predictive_input: false,
            zenz_personalization: None,
            lm_typo_enabled: false,
            lm_typo_language_model: LmTypoLanguageModel::Zenz,
            lm_typo_config: LmTypoConfig::default(),
            lm_typo_ngram: None,
            foreign_completion_provider: None,
            learning_memory: None,
            learning_available: false,
            learning_writable: false,
            text_replacer: None,
            user_dictionary: default_user_dictionary(),
            user_dictionary_directory: None,
            request_options: default_request_options(),
            live_conversion: false,
            default_input_style: ConverterInputStyle::RomanToKana,
            default_keyboard_language: KeyboardLanguage::Japanese,
            input_behavior: InputBehavior::default(),
        })
    }

    pub fn open_with_zenz_model(
        dictionary_path: impl AsRef<Path>,
        model: Box<dyn ZenzLanguageModel>,
    ) -> Result<Self, DictionaryError> {
        let mut engine = Self::open(dictionary_path)?;
        engine.zenz_model = Some(model);
        Ok(engine)
    }

    pub fn open_with_llama(
        dictionary_path: impl AsRef<Path>,
        model_path: impl AsRef<Path>,
        backend_directory: impl AsRef<Path>,
    ) -> Result<Self, EngineOpenError> {
        Ok(Self::open_with_zenz_model(
            dictionary_path,
            Box::new(LlamaModel::load(model_path, backend_directory)?),
        )?)
    }

    pub fn open_with_config(
        config: &DaemonConfig,
        learning_directory: impl AsRef<Path>,
    ) -> Result<Self, EngineOpenError> {
        let mut engine = Self::open_with_llama(
            &config.dictionary,
            &config.model,
            &config.llama_backend_directory,
        )?;
        engine.load_hunspell(
            &config.hunspell.english_dictionary,
            &config.hunspell.greek_dictionary,
        )?;
        engine.load_learning(learning_directory, &config.learning)?;
        engine.load_text_replacer(&config.emoji_dictionary)?;
        engine.load_conversion_resources(&config.conversion)?;
        engine.apply_conversion_options(&config.conversion)?;
        engine.apply_zenz_options(&config.zenz);
        engine.load_zenz_personalization(&config.zenz)?;
        engine.load_lm_typo(&config.lm_typo)?;
        Ok(engine)
    }

    pub fn open_with_hunspell(
        dictionary_path: impl AsRef<Path>,
        english_dictionary: impl AsRef<Path>,
        greek_dictionary: impl AsRef<Path>,
    ) -> Result<Self, EngineOpenError> {
        let mut engine = Self::open(dictionary_path)?;
        engine.load_hunspell(english_dictionary, greek_dictionary)?;
        engine.request_options.foreign_prediction = PredictionMode::Automatic;
        Ok(engine)
    }

    pub fn open_with_learning(
        dictionary_path: impl AsRef<Path>,
        learning_directory: impl AsRef<Path>,
    ) -> Result<Self, EngineOpenError> {
        let mut engine = Self::open(dictionary_path)?;
        engine.load_learning(learning_directory, &LearningConfig::default())?;
        Ok(engine)
    }

    pub fn open_with_emoji(
        dictionary_path: impl AsRef<Path>,
        emoji_dictionary: impl AsRef<Path>,
    ) -> Result<Self, EngineOpenError> {
        let mut engine = Self::open(dictionary_path)?;
        engine.load_text_replacer(emoji_dictionary)?;
        Ok(engine)
    }

    pub fn open_with_conversion_resources(
        dictionary_path: impl AsRef<Path>,
        conversion: &ConversionConfig,
    ) -> Result<Self, EngineOpenError> {
        let mut engine = Self::open(dictionary_path)?;
        engine.load_conversion_resources(conversion)?;
        engine.apply_conversion_options(conversion)?;
        Ok(engine)
    }

    fn load_hunspell(
        &mut self,
        english_dictionary: impl AsRef<Path>,
        greek_dictionary: impl AsRef<Path>,
    ) -> Result<(), HunspellError> {
        self.foreign_completion_provider = Some(Arc::new(HunspellCompleter::open(
            english_dictionary,
            greek_dictionary,
        )?));
        Ok(())
    }

    fn load_learning(
        &mut self,
        learning_directory: impl AsRef<Path>,
        config: &LearningConfig,
    ) -> Result<(), LearningError> {
        let mode = match config.mode {
            LearningModeConfig::InputAndOutput => LearningMode::InputAndOutput,
            LearningModeConfig::OnlyOutput => LearningMode::OnlyOutput,
            LearningModeConfig::Nothing => LearningMode::Nothing,
        };
        self.learning_memory = Some(LearningMemory::open(
            learning_directory.as_ref().to_path_buf(),
            mode,
            config.max_count,
            self.dictionary.character_ids().clone(),
        )?);
        self.learning_available = config.mode != LearningModeConfig::Nothing;
        self.learning_writable = config.mode == LearningModeConfig::InputAndOutput;
        Ok(())
    }

    fn load_text_replacer(
        &mut self,
        emoji_dictionary: impl AsRef<Path>,
    ) -> Result<(), TextReplacerError> {
        self.text_replacer = Some(TextReplacer::open(emoji_dictionary)?);
        Ok(())
    }

    fn load_conversion_resources(
        &mut self,
        conversion: &ConversionConfig,
    ) -> Result<(), ConversionResourceError> {
        if let Some(path) = &conversion.user_dictionary {
            let source =
                fs::read_to_string(path).map_err(|source| ConversionResourceError::Read {
                    path: path.clone(),
                    source,
                })?;
            let items: Vec<UserDictionaryItem> =
                serde_json::from_str(&source).map_err(ConversionResourceError::UserDictionary)?;
            self.user_dictionary.extend(
                items.into_iter().map(|item| {
                    desktop_user_dictionary_entry(item.word, to_katakana(&item.reading))
                }),
            );
        }
        if let Some(path) = &conversion.user_dictionary_directory {
            beankey_converter::UserDictionary::open(path.clone())
                .map_err(ConversionResourceError::Dictionary)?;
            self.user_dictionary_directory = Some(path.clone());
        }
        for (name, path) in &conversion.custom_input_tables {
            let source =
                fs::read_to_string(path).map_err(|source| ConversionResourceError::Read {
                    path: path.clone(),
                    source,
                })?;
            if name.is_empty()
                || !matches!(
                    InputTable::check_custom_tsv(&source),
                    FormatReport::FullyValid
                )
            {
                return Err(ConversionResourceError::InvalidInputTable { name: name.clone() });
            }
            self.tables
                .register(name.clone(), InputTable::from_custom_tsv(&source));
        }
        Ok(())
    }

    fn apply_conversion_options(
        &mut self,
        conversion: &ConversionConfig,
    ) -> Result<(), ConversionResourceError> {
        self.request_options = RequestOptions {
            n_best: conversion.n_best,
            japanese_prediction: prediction_mode(conversion.japanese_prediction),
            full_width_roman: conversion.full_width_roman,
            half_width_kana: conversion.half_width_kana,
            typo_correction: typo_correction_mode(conversion.typo_correction),
            foreign_prediction: prediction_mode(conversion.foreign_prediction),
            typography: conversion.typography,
            ..default_request_options()
        };
        self.live_conversion = conversion.live_conversion;
        self.default_input_style = match conversion.input_style {
            InputStyleConfig::Direct => ConverterInputStyle::Direct,
            InputStyleConfig::RomanToKana => ConverterInputStyle::RomanToKana,
            InputStyleConfig::Azik => ConverterInputStyle::Mapped(InputTableId::DefaultAzik),
            InputStyleConfig::KanaJis => ConverterInputStyle::Mapped(InputTableId::DefaultKanaJis),
            InputStyleConfig::KanaUs => ConverterInputStyle::Mapped(InputTableId::DefaultKanaUs),
            InputStyleConfig::Custom => {
                let name = conversion
                    .custom_input_table
                    .as_ref()
                    .filter(|name| self.tables.contains(name))
                    .ok_or_else(|| ConversionResourceError::InvalidInputTable {
                        name: conversion.custom_input_table.clone().unwrap_or_default(),
                    })?;
                ConverterInputStyle::Mapped(InputTableId::Named(name.clone()))
            }
        };
        self.default_keyboard_language = match conversion.keyboard_language {
            KeyboardLanguageConfig::None => KeyboardLanguage::None,
            KeyboardLanguageConfig::Japanese => KeyboardLanguage::Japanese,
            KeyboardLanguageConfig::EnglishUs => KeyboardLanguage::EnglishUs,
            KeyboardLanguageConfig::Greek => KeyboardLanguage::Greek,
        };
        self.input_behavior = InputBehavior {
            type_backslash: conversion.type_backslash,
            type_half_space: conversion.type_half_space,
            option_direct_full_width_input: conversion.option_direct_full_width_input,
            punctuation_style: conversion.punctuation_style,
        };
        Ok(())
    }

    fn apply_zenz_options(&mut self, config: &crate::config::ZenzConfig) {
        self.zenz_inference_limit = config.inference_limit;
        self.zenz_rich_candidates = config.rich_candidates;
        self.zenz_predictive_input = config.predictive_input;
        self.zenz_version = ZenzVersionConfig::V3(ZenzV3Config {
            profile: config.profile.clone(),
            topic: config.topic.clone(),
            style: config.style.clone(),
            preference: config.preference.clone(),
            enable_alignment_separator: config.enable_alignment_separator,
            ..ZenzV3Config::default()
        });
    }

    fn load_zenz_personalization(
        &mut self,
        config: &crate::config::ZenzConfig,
    ) -> Result<(), NGramError> {
        self.zenz_personalization = config
            .personalization
            .as_ref()
            .map(|personalization| {
                zenz::ZenzPersonalizationModels::load(
                    &personalization.base_ngram,
                    &personalization.personal_ngram,
                    personalization.alpha,
                )
            })
            .transpose()?;
        Ok(())
    }

    fn load_lm_typo(
        &mut self,
        config: &crate::config::LmTypoCorrectionConfig,
    ) -> Result<(), NGramError> {
        self.lm_typo_enabled = config.enabled;
        self.lm_typo_language_model = config.language_model;
        self.lm_typo_config = LmTypoConfig {
            beam_size: config.beam_size,
            top_k: config.top_k,
            n_best: config.n_best,
            max_steps: config.max_steps,
            substitution_cost: config.substitution_cost,
            deletion_cost: config.deletion_cost,
            transposition_cost: config.transposition_cost,
        };
        self.lm_typo_ngram =
            if config.enabled && config.language_model == LmTypoLanguageModel::Ngram {
                config
                    .ngram
                    .as_ref()
                    .map(|ngram| {
                        NGramLanguageModel::open(
                            &ngram.prefix,
                            &ngram.tokenizer,
                            ngram.n,
                            ngram.discount,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
        Ok(())
    }

    pub fn handle(&mut self, envelope: protocol::Envelope) -> protocol::Envelope {
        let request_id = envelope.request_id;
        let session_id = envelope.session_id.clone();
        let payload = match envelope.payload {
            Some(payload) => payload,
            None => {
                return error_envelope(
                    request_id,
                    session_id,
                    Code::InvalidPayload,
                    "missing payload",
                );
            }
        };
        match payload {
            Payload::StartSession(start) => {
                if self
                    .sessions
                    .get(&session_id)
                    .is_some_and(|session| request_id <= session.last_request_id)
                {
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::OutOfOrderRequest,
                        "request ID is not greater than the previous request",
                    );
                }
                let input_style = if protocol::InputStyle::try_from(start.input_style)
                    == Ok(protocol::InputStyle::Unspecified)
                {
                    Some(self.default_input_style.clone())
                } else {
                    input_style(start.input_style, &start.custom_input_table, &self.tables)
                };
                let Some(input_style) = input_style else {
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::InvalidPayload,
                        "invalid input style",
                    );
                };
                let surrounding = match surrounding_context(start.surrounding_text.as_ref()) {
                    Ok(surrounding) => surrounding,
                    Err(message) => {
                        return error_envelope(
                            request_id,
                            session_id,
                            Code::InvalidPayload,
                            message,
                        );
                    }
                };
                let keyboard_language =
                    if protocol::KeyboardLanguage::try_from(start.keyboard_language)
                        == Ok(protocol::KeyboardLanguage::Unspecified)
                    {
                        Some(self.default_keyboard_language)
                    } else {
                        keyboard_language(start.keyboard_language)
                    };
                let Some(keyboard_language) = keyboard_language else {
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::InvalidPayload,
                        "invalid keyboard language",
                    );
                };
                let mut conversion = ConversionSession::new();
                conversion.import_dynamic_user_dictionary(
                    self.user_dictionary.clone(),
                    desktop_dynamic_shortcuts(),
                );
                if let Some(path) = &self.user_dictionary_directory
                    && let Err(error) = conversion.update_user_dictionary_path(path.clone())
                {
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::Internal,
                        format!("user dictionary initialization failed: {error}"),
                    );
                }
                if let Some(provider) = &self.foreign_completion_provider {
                    conversion.set_foreign_completion_provider(provider.clone());
                }
                if let Some(memory) = &self.learning_memory
                    && let Err(error) = conversion.set_learning_memory(memory.clone())
                {
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::Internal,
                        format!("learning memory initialization failed: {error}"),
                    );
                }
                self.sessions.insert(
                    session_id.clone(),
                    SessionState {
                        conversion,
                        input_style,
                        last_request_id: request_id,
                        selected_candidate: 0,
                        input_mode: InputMode::None,
                        display_candidates: Vec::new(),
                        candidate_remainders: Vec::new(),
                        candidate_annotations: Vec::new(),
                        preview_candidate_index: None,
                        additional_candidate_count: 0,
                        segment_surface_count: None,
                        last_was_backspace: false,
                        prediction: None,
                        live_conversion_enabled: self.live_conversion,
                        surrounding,
                        request_options: RequestOptions {
                            keyboard_language,
                            ..self.request_options.clone()
                        },
                        live_candidate: None,
                        typo_corrections: Vec::new(),
                        lm_typo_available: self.lm_typo_enabled,
                        learning_available: self.learning_available,
                        learning_writable: self.learning_writable,
                        zenz_cache: zenz::ZenzConversionCache::default(),
                        unicode_input: String::new(),
                        input_language: InputLanguage::Japanese,
                    },
                );
                state_envelope(
                    request_id,
                    session_id,
                    protocol::StateResponse {
                        reset: true,
                        input_state: protocol::InputState::None as i32,
                        candidate_window: protocol::CandidateWindow::Hidden as i32,
                        lm_typo_available: self.lm_typo_enabled,
                        learning_available: self.learning_available,
                        learning_writable: self.learning_writable,
                        ..Default::default()
                    },
                )
            }
            Payload::StateResponse(_)
            | Payload::TypoCorrectionResponse(_)
            | Payload::ProtocolError(_) => error_envelope(
                request_id,
                session_id,
                Code::InvalidPayload,
                "response payload sent as a request",
            ),
            request => self.handle_session_request(request_id, session_id, request),
        }
    }

    pub fn remove_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    fn handle_session_request(
        &mut self,
        request_id: u64,
        session_id: String,
        request: Payload,
    ) -> protocol::Envelope {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return error_envelope(
                request_id,
                session_id,
                Code::UnknownSession,
                "unknown session",
            );
        };
        if request_id <= session.last_request_id {
            self.sessions.insert(session_id.clone(), session);
            return error_envelope(
                request_id,
                session_id,
                Code::OutOfOrderRequest,
                "request ID is not greater than the previous request",
            );
        }
        session.last_request_id = request_id;
        if matches!(request, Payload::EndSession(_)) {
            if let Err(error) = session.conversion.commit_learning() {
                return error_envelope(
                    request_id,
                    session_id,
                    Code::Internal,
                    format!("learning memory commit failed: {error}"),
                );
            }
            return state_envelope(
                request_id,
                session_id,
                protocol::StateResponse {
                    reset: true,
                    input_state: protocol::InputState::None as i32,
                    candidate_window: protocol::CandidateWindow::Hidden as i32,
                    lm_typo_available: session.lm_typo_available,
                    learning_available: session.learning_available,
                    learning_writable: session.learning_writable,
                    ..Default::default()
                },
            );
        }
        if matches!(request, Payload::RequestTypoCorrections(_)) {
            let result = self.request_typo_corrections(&session);
            match result {
                Ok(candidates) => {
                    session.typo_corrections.clone_from(&candidates);
                    self.sessions.insert(session_id.clone(), session);
                    return typo_correction_envelope(request_id, session_id, candidates);
                }
                Err((code, message)) => {
                    session.reset();
                    self.sessions.insert(session_id.clone(), session);
                    return error_envelope(request_id, session_id, code, message);
                }
            }
        }

        let response = match request {
            Payload::ResetSession(_) => {
                session.reset();
                Ok(protocol::StateResponse {
                    reset: true,
                    input_state: protocol::InputState::None as i32,
                    candidate_window: protocol::CandidateWindow::Hidden as i32,
                    lm_typo_available: session.lm_typo_available,
                    learning_available: session.learning_available,
                    learning_writable: session.learning_writable,
                    ..Default::default()
                })
            }
            Payload::KeyEvent(event) => {
                let updated = event
                    .surrounding_text
                    .as_ref()
                    .map(|surrounding| surrounding_context(Some(surrounding)))
                    .transpose();
                match updated {
                    Ok(Some(surrounding)) => {
                        session.surrounding = surrounding;
                        self.handle_key(&mut session, event)
                    }
                    Ok(None) => self.handle_key(&mut session, event),
                    Err(message) => Err((Code::InvalidPayload, message)),
                }
            }
            Payload::SelectCandidate(selection) => {
                self.select_candidate(&mut session, selection.index as usize)
            }
            Payload::SelectTypoCorrection(selection) => {
                self.select_typo_correction(&mut session, selection.index as usize)
            }
            Payload::ForgetCandidate(forget) => {
                self.forget_candidate(&mut session, forget.index as usize)
            }
            Payload::ResetLearning(_) => self.reset_learning(&mut session),
            Payload::PageCandidates(page) => {
                if !page_candidates(&mut session, page) {
                    self.sessions.insert(session_id.clone(), session);
                    return error_envelope(
                        request_id,
                        session_id,
                        Code::InvalidPayload,
                        "invalid candidate page request",
                    );
                }
                session.live_candidate = None;
                Ok(make_state(&session, true, String::new(), false))
            }
            Payload::CommitComposition(_) => {
                let commit = current_marked_text(&session, &self.tables);
                let consumed = !commit.is_empty();
                session.reset();
                Ok(protocol::StateResponse {
                    consumed,
                    commit,
                    reset: consumed,
                    input_state: protocol::InputState::None as i32,
                    candidate_window: protocol::CandidateWindow::Hidden as i32,
                    lm_typo_available: session.lm_typo_available,
                    learning_available: session.learning_available,
                    learning_writable: session.learning_writable,
                    ..Default::default()
                })
            }
            Payload::StartSession(_)
            | Payload::EndSession(_)
            | Payload::RequestTypoCorrections(_)
            | Payload::StateResponse(_)
            | Payload::TypoCorrectionResponse(_)
            | Payload::ProtocolError(_) => {
                Err((Code::InvalidPayload, "invalid session request".into()))
            }
        };
        self.sessions.insert(session_id.clone(), session);
        match response {
            Ok(response) => state_envelope(request_id, session_id, response),
            Err((code, message)) => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.reset();
                }
                error_envelope(request_id, session_id, code, message)
            }
        }
    }

    fn request_typo_corrections(
        &mut self,
        session: &SessionState,
    ) -> SessionRequestResult<Vec<beankey_converter::LmTypoCandidate>> {
        if !self.lm_typo_enabled {
            return Err((
                Code::InvalidPayload,
                "LM typo correction is disabled".into(),
            ));
        }
        let model: &mut dyn ZenzLanguageModel = match self.lm_typo_language_model {
            LmTypoLanguageModel::Zenz => self.zenz_model.as_deref_mut().ok_or_else(|| {
                (
                    Code::Internal,
                    "Zenz model is unavailable for LM typo correction".into(),
                )
            })?,
            LmTypoLanguageModel::Ngram => self.lm_typo_ngram.as_mut().ok_or_else(|| {
                (
                    Code::Internal,
                    "N-gram model is unavailable for LM typo correction".into(),
                )
            })?,
        };
        experimental_typo_correction(
            model,
            session.surrounding.left.as_deref().unwrap_or(""),
            session.conversion.composing(),
            &session.input_style,
            &self.tables,
            &self.lm_typo_config,
        )
        .map_err(|error| {
            (
                Code::Internal,
                format!("LM typo correction failed: {error}"),
            )
        })
    }

    fn handle_key(
        &mut self,
        session: &mut SessionState,
        mut event: protocol::KeyEvent,
    ) -> SessionRequestResult<protocol::StateResponse> {
        session.typo_corrections.clear();
        let action = protocol::UserAction::try_from(event.action)
            .unwrap_or(protocol::UserAction::Unspecified);

        if action == protocol::UserAction::Input
            && session.input_mode == InputMode::None
            && self.input_behavior.option_direct_full_width_input
            && event.option
            && let Some(text) = option_direct_input_text(&event, self.input_behavior.type_backslash)
        {
            return Ok(make_state(session, true, text, false));
        }

        normalize_input_event(&mut event, self.input_behavior);

        if session.input_mode == InputMode::Unicode {
            return self.handle_unicode_input(session, action, &event);
        }

        if action == protocol::UserAction::StartUnicodeInput {
            return self.enter_unicode_input(session);
        }

        if action == protocol::UserAction::Eisu {
            if session.input_mode == InputMode::Selecting {
                let mut response = self.select_candidate(session, session.selected_candidate)?;
                session.input_language = InputLanguage::English;
                response.input_language = protocol::InputLanguage::English as i32;
                return Ok(response);
            }
            session.input_language = InputLanguage::English;
            return Ok(make_state(session, true, String::new(), false));
        }

        if action == protocol::UserAction::Kana {
            session.input_language = InputLanguage::Japanese;
            return Ok(make_state(session, true, String::new(), false));
        }

        if action == protocol::UserAction::Space
            && session.input_mode == InputMode::Composing
            && session.input_language == InputLanguage::English
        {
            session
                .conversion
                .insert_str(" ", ConverterInputStyle::Direct, &self.tables);
            session.segment_surface_count = None;
            session.last_was_backspace = false;
            session.clear_presentation();
            return Ok(make_state(session, true, String::new(), false));
        }

        if action == protocol::UserAction::Input
            && session.input_mode == InputMode::None
            && session.input_language == InputLanguage::English
        {
            let input = if event.input.is_empty() {
                event.text.clone()
            } else {
                event.input.clone()
            };
            if !input.is_empty() && !input.chars().any(char::is_control) {
                return Ok(make_state(session, true, input, false));
            }
        }

        if action == protocol::UserAction::Input
            && session.input_mode == InputMode::Selecting
            && let Some(number) = selection_number(&event.text)
        {
            if number == 0 {
                let commit = current_marked_text(session, &self.tables);
                session.reset();
                insert_event(session, &event, &self.tables);
                session.input_mode = InputMode::Composing;
                self.request_candidates(session)?;
                return Ok(make_state(session, true, commit, false));
            }
            let page_start = session.selected_candidate / 9 * 9;
            let index = page_start + number - 1;
            return self.select_candidate(session, index);
        }

        match action {
            protocol::UserAction::Backspace if !session.conversion.composing().is_empty() => {
                session.conversion.delete_backward(1, &self.tables);
                session.segment_surface_count = None;
                session.last_was_backspace = true;
                if session.conversion.composing().is_empty() {
                    session.reset();
                    return Ok(make_state(session, true, String::new(), true));
                }
                session.input_mode = InputMode::Composing;
                session.clear_presentation();
                self.request_candidates(session)?;
                // azooKey-Desktop deliberately shows the raw reading immediately after deletion.
                session.live_candidate = None;
                return Ok(make_state(session, true, String::new(), false));
            }
            protocol::UserAction::Escape => match session.input_mode {
                InputMode::None => return Ok(make_state(session, false, String::new(), false)),
                InputMode::Composing => {
                    session.reset();
                    return Ok(make_state(session, true, String::new(), true));
                }
                InputMode::Previewing => {
                    session.input_mode = InputMode::Composing;
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Selecting => {
                    session.input_mode = if self.live_conversion {
                        InputMode::Composing
                    } else {
                        InputMode::Previewing
                    };
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Unicode => {
                    unreachable!("Unicode input is handled before the main state machine")
                }
            },
            protocol::UserAction::Enter if session.input_mode != InputMode::None => {
                if session.input_mode == InputMode::Selecting {
                    return self.select_candidate(session, session.selected_candidate);
                }
                let commit = current_marked_text(session, &self.tables);
                self.learn_current_marked_candidate(session, &commit)?;
                session.reset();
                return Ok(make_state(session, true, commit, true));
            }
            protocol::UserAction::Space => match session.input_mode {
                InputMode::None => {
                    let full_width = session.input_language == InputLanguage::Japanese
                        && event.shift == self.input_behavior.type_half_space;
                    let commit = if full_width { "　" } else { " " }.to_owned();
                    return Ok(make_state(session, true, commit, false));
                }
                InputMode::Composing => {
                    session
                        .conversion
                        .insert_composition_separator(session.input_style.clone(), &self.tables);
                    if self.live_conversion {
                        session.input_mode = InputMode::Selecting;
                        self.request_rich_candidates(session)?;
                        session.selected_candidate = 0;
                    } else {
                        session.input_mode = InputMode::Previewing;
                        self.request_candidates(session)?;
                        session.selected_candidate =
                            session.preview_candidate_index.unwrap_or_default();
                    }
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Previewing => {
                    self.request_rich_candidates(session)?;
                    session.input_mode = InputMode::Selecting;
                    session.selected_candidate = 0;
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Selecting => {
                    let count = session.display_candidates.len();
                    if count > 0 {
                        session.selected_candidate = if event.shift {
                            session.selected_candidate.saturating_sub(1)
                        } else {
                            (session.selected_candidate + 1).min(count - 1)
                        };
                    }
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Unicode => {
                    unreachable!("Unicode input is handled before the main state machine")
                }
            },
            protocol::UserAction::Down => match session.input_mode {
                InputMode::Composing | InputMode::Previewing => {
                    session
                        .conversion
                        .insert_composition_separator(session.input_style.clone(), &self.tables);
                    self.request_rich_candidates(session)?;
                    session.input_mode = InputMode::Selecting;
                    session.selected_candidate = 0;
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::Selecting => {
                    let count = session.display_candidates.len();
                    if count > 0 {
                        session.selected_candidate =
                            (session.selected_candidate + 1).min(count - 1);
                    }
                    return Ok(make_state(session, true, String::new(), false));
                }
                InputMode::None => return Ok(make_state(session, false, String::new(), false)),
                InputMode::Unicode => {
                    unreachable!("Unicode input is handled before the main state machine")
                }
            },
            protocol::UserAction::Up => {
                if session.input_mode == InputMode::Selecting {
                    if session.selected_candidate == 0 && session.additional_candidate_count < 5 {
                        self.reveal_additional_candidate(session);
                    } else {
                        session.selected_candidate = session.selected_candidate.saturating_sub(1);
                    }
                }
                let consumed = session.input_mode != InputMode::None;
                return Ok(make_state(session, consumed, String::new(), false));
            }
            protocol::UserAction::Left | protocol::UserAction::Right
                if event.shift && session.input_mode != InputMode::None =>
            {
                if let Some(remaining) = session
                    .candidate_remainders
                    .get(session.selected_candidate)
                    .filter(|_| session.input_mode == InputMode::Selecting)
                {
                    let prefix = session
                        .display_candidates
                        .get(session.selected_candidate)
                        .map(|candidate| {
                            session
                                .conversion
                                .consumed_surface_count(candidate, &self.tables)
                        })
                        .unwrap_or_else(|| {
                            session
                                .conversion
                                .composing()
                                .surface_graphemes()
                                .len()
                                .saturating_sub(remaining.chars().count())
                        });
                    let cursor = session.conversion.composing().cursor();
                    session
                        .conversion
                        .move_cursor(prefix as isize - cursor as isize);
                }
                let total = session.conversion.composing().surface_graphemes().len();
                let current = session.conversion.composing().cursor();
                let target = if action == protocol::UserAction::Right {
                    if current >= total { 1 } else { current + 1 }
                } else {
                    current.saturating_sub(1).max(1)
                };
                session
                    .conversion
                    .move_cursor(target as isize - current as isize);
                session.segment_surface_count = Some(target);
                session.input_mode = InputMode::Selecting;
                session.clear_presentation();
                self.request_candidates(session)?;
                session.live_candidate = None;
                return Ok(make_state(session, true, String::new(), false));
            }
            protocol::UserAction::Right if session.input_mode == InputMode::Selecting => {
                return self.select_candidate_with_remainder_mode(
                    session,
                    session.selected_candidate,
                    InputMode::Selecting,
                );
            }
            protocol::UserAction::Left | protocol::UserAction::Right
                if session.input_mode != InputMode::None =>
            {
                return Ok(make_state(session, true, String::new(), false));
            }
            protocol::UserAction::Tab if session.input_mode != InputMode::None => {
                if session.input_mode == InputMode::Composing
                    && let Some(prediction) = session.prediction.clone()
                {
                    session.last_was_backspace = false;
                    if prediction.delete_count > 0 {
                        session
                            .conversion
                            .delete_backward(prediction.delete_count, &self.tables);
                    }
                    session.conversion.insert_str(
                        &prediction.append_text,
                        session.input_style.clone(),
                        &self.tables,
                    );
                    self.request_candidates(session)?;
                }
                return Ok(make_state(session, true, String::new(), false));
            }
            protocol::UserAction::Hiragana
            | protocol::UserAction::Katakana
            | protocol::UserAction::HalfWidthKatakana
            | protocol::UserAction::FullWidthRoman
            | protocol::UserAction::HalfWidthRoman
                if session.input_mode != InputMode::None =>
            {
                let representation_index = match action {
                    protocol::UserAction::HalfWidthRoman => 0,
                    protocol::UserAction::FullWidthRoman => 1,
                    protocol::UserAction::HalfWidthKatakana => 2,
                    protocol::UserAction::Katakana => 3,
                    protocol::UserAction::Hiragana => 4,
                    _ => unreachable!(),
                };
                let selected = (session.input_mode == InputMode::Selecting)
                    .then(|| {
                        session
                            .display_candidates
                            .get(session.selected_candidate)
                            .cloned()
                    })
                    .flatten();
                let transformed = session
                    .conversion
                    .desktop_additional_candidates(selected.as_ref(), &self.tables)
                    .into_iter()
                    .nth(representation_index)
                    .expect("desktop representations have a fixed shape")
                    .0;
                return self.commit_candidate(session, transformed, InputMode::Selecting);
            }
            protocol::UserAction::Forget if session.input_mode == InputMode::Selecting => {
                return self.forget_candidate(session, session.selected_candidate);
            }
            protocol::UserAction::Consume => {
                let consumed = session.input_mode != InputMode::None;
                return Ok(make_state(session, consumed, String::new(), false));
            }
            protocol::UserAction::Input if valid_event_text(&event) => {
                let commit = matches!(
                    session.input_mode,
                    InputMode::Previewing | InputMode::Selecting
                )
                .then(|| current_marked_text(session, &self.tables))
                .unwrap_or_default();
                if !commit.is_empty() {
                    session.reset();
                } else {
                    session.clear_presentation();
                }
                session.segment_surface_count = None;
                session.last_was_backspace = false;
                insert_event(session, &event, &self.tables);
                session.input_mode = InputMode::Composing;
                self.request_candidates(session)?;
                return Ok(make_state(session, true, commit, false));
            }
            _ => {}
        }
        Ok(make_state(session, false, String::new(), false))
    }

    fn enter_unicode_input(
        &mut self,
        session: &mut SessionState,
    ) -> SessionRequestResult<protocol::StateResponse> {
        let mut commit = String::new();
        match session.input_mode {
            InputMode::None => {}
            InputMode::Composing | InputMode::Previewing => {
                commit = current_marked_text(session, &self.tables);
                self.learn_current_marked_candidate(session, &commit)?;
                session.reset();
            }
            InputMode::Selecting => {
                let candidate = session
                    .display_candidates
                    .get(session.selected_candidate)
                    .cloned()
                    .ok_or_else(|| {
                        (
                            Code::InvalidPayload,
                            "selected candidate is outside the current candidates".to_owned(),
                        )
                    })?;
                commit = self.select_and_stage_learning(session, candidate)?;
                if !session.conversion.composing().is_empty() {
                    commit.push_str(&session.conversion.composing().surface());
                }
                session.reset();
            }
            InputMode::Unicode => {}
        }
        session.input_mode = InputMode::Unicode;
        session.unicode_input.clear();
        Ok(make_state(session, true, commit, false))
    }

    fn handle_unicode_input(
        &mut self,
        session: &mut SessionState,
        action: protocol::UserAction,
        event: &protocol::KeyEvent,
    ) -> SessionRequestResult<protocol::StateResponse> {
        match action {
            protocol::UserAction::Input => {
                session.unicode_input.extend(
                    event
                        .input
                        .to_ascii_lowercase()
                        .chars()
                        .filter(char::is_ascii_hexdigit),
                );
                Ok(make_state(session, true, String::new(), false))
            }
            protocol::UserAction::Backspace => {
                if session.unicode_input.pop().is_none() {
                    session.reset();
                    return Ok(make_state(session, true, String::new(), true));
                }
                Ok(make_state(session, true, String::new(), false))
            }
            protocol::UserAction::Enter | protocol::UserAction::Space => {
                let commit = u32::from_str_radix(&session.unicode_input, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                session.reset();
                Ok(make_state(session, true, commit, true))
            }
            protocol::UserAction::Escape => {
                session.reset();
                Ok(make_state(session, true, String::new(), true))
            }
            _ => Ok(make_state(session, true, String::new(), false)),
        }
    }

    fn reveal_additional_candidate(&self, session: &mut SessionState) {
        let base_start = session.additional_candidate_count;
        let selected = session.display_candidates.get(base_start);
        let additional = session
            .conversion
            .desktop_additional_candidates(selected, &self.tables);
        let shown = (session.additional_candidate_count + 1).min(additional.len());
        let base_candidates = session.display_candidates[base_start..].to_vec();
        let mut candidates = additional[additional.len() - shown..]
            .iter()
            .map(|(candidate, _)| candidate.clone())
            .collect::<Vec<_>>();
        let mut annotations = additional[additional.len() - shown..]
            .iter()
            .map(|(_, annotation)| (*annotation).to_owned())
            .collect::<Vec<_>>();
        candidates.extend(base_candidates);
        annotations.extend(std::iter::repeat_n(
            String::new(),
            candidates.len() - annotations.len(),
        ));
        session.display_candidates = candidates;
        session.candidate_annotations = annotations;
        session.candidate_remainders = session
            .display_candidates
            .iter()
            .map(|candidate| {
                session
                    .conversion
                    .remaining_after_candidate(candidate, &self.tables)
            })
            .collect();
        session.additional_candidate_count = shown;
        session.selected_candidate = 0;
    }

    fn select_candidate(
        &mut self,
        session: &mut SessionState,
        index: usize,
    ) -> SessionRequestResult<protocol::StateResponse> {
        self.select_candidate_with_remainder_mode(session, index, InputMode::Previewing)
    }

    fn select_candidate_with_remainder_mode(
        &mut self,
        session: &mut SessionState,
        index: usize,
        remainder_mode: InputMode,
    ) -> SessionRequestResult<protocol::StateResponse> {
        session.typo_corrections.clear();
        let selected = session
            .display_candidates
            .get(index)
            .cloned()
            .ok_or_else(|| {
                (
                    Code::InvalidPayload,
                    format!("candidate index {index} is outside the current candidates"),
                )
            })?;
        self.commit_candidate(session, selected, remainder_mode)
    }

    fn commit_candidate(
        &mut self,
        session: &mut SessionState,
        selected: ConverterCandidate,
        remainder_mode: InputMode,
    ) -> SessionRequestResult<protocol::StateResponse> {
        let commit = self.select_and_stage_learning(session, selected)?;
        let mut left_context = session.surrounding.left.clone().unwrap_or_default();
        left_context.push_str(&commit);
        session.clear_presentation();
        session.last_was_backspace = false;
        if !session.conversion.composing().is_empty() {
            session.segment_surface_count = None;
            session.input_mode = remainder_mode;
            self.request_candidates_with_context(session, true, Some(&left_context))?;
        } else {
            session.input_mode = InputMode::None;
        }
        let reset = session.conversion.composing().is_empty();
        Ok(make_state(session, true, commit, reset))
    }

    fn select_and_stage_learning(
        &mut self,
        session: &mut SessionState,
        candidate: ConverterCandidate,
    ) -> SessionRequestResult<String> {
        session
            .conversion
            .select_candidate_value(candidate, &self.tables)
            .map_err(|error| match error {
                SelectionError::CandidateOutOfRange { .. } => (
                    Code::InvalidPayload,
                    format!("candidate selection failed: {error}"),
                ),
                SelectionError::Learning(_) => (
                    Code::Internal,
                    format!("candidate selection failed: {error}"),
                ),
            })
    }

    fn learn_current_marked_candidate(
        &mut self,
        session: &mut SessionState,
        marked_text: &str,
    ) -> SessionRequestResult<()> {
        let candidate = session
            .live_candidate
            .as_ref()
            .filter(|candidate| candidate.text == marked_text)
            .or_else(|| {
                session
                    .display_candidates
                    .iter()
                    .zip(&session.candidate_remainders)
                    .find_map(|(candidate, remaining)| {
                        (remaining.is_empty() && candidate.text == marked_text).then_some(candidate)
                    })
            })
            .cloned();
        let Some(candidate) = candidate else {
            return Ok(());
        };
        self.select_and_stage_learning(session, candidate)?;
        Ok(())
    }

    fn select_typo_correction(
        &mut self,
        session: &mut SessionState,
        index: usize,
    ) -> SessionRequestResult<protocol::StateResponse> {
        let correction = session
            .typo_corrections
            .get(index)
            .cloned()
            .ok_or_else(|| {
                (
                    Code::InvalidPayload,
                    format!("typo correction index {index} is outside the current candidates"),
                )
            })?;
        session.reset();
        Ok(protocol::StateResponse {
            consumed: true,
            commit: correction.converted_text,
            reset: true,
            input_state: protocol::InputState::None as i32,
            candidate_window: protocol::CandidateWindow::Hidden as i32,
            lm_typo_available: session.lm_typo_available,
            learning_available: session.learning_available,
            learning_writable: session.learning_writable,
            ..Default::default()
        })
    }

    fn forget_candidate(
        &mut self,
        session: &mut SessionState,
        index: usize,
    ) -> SessionRequestResult<protocol::StateResponse> {
        session.typo_corrections.clear();
        let candidate = session
            .display_candidates
            .get(index)
            .cloned()
            .ok_or_else(|| {
                (
                    Code::InvalidPayload,
                    format!("candidate index {index} is outside the current candidates"),
                )
            })?;
        session
            .conversion
            .forget_learning(&candidate)
            .map_err(|error| {
                (
                    Code::Internal,
                    format!("learning memory forget failed: {error}"),
                )
            })?;
        session.selected_candidate = 0;
        session.live_candidate = None;
        self.request_candidates(session)?;
        Ok(make_state(session, true, String::new(), false))
    }

    fn reset_learning(
        &mut self,
        session: &mut SessionState,
    ) -> SessionRequestResult<protocol::StateResponse> {
        session.typo_corrections.clear();
        session.conversion.reset_learning().map_err(|error| {
            (
                Code::Internal,
                format!("learning memory reset failed: {error}"),
            )
        })?;
        session.selected_candidate = 0;
        session.live_candidate = None;
        if !session.conversion.composing().is_empty() {
            self.request_candidates(session)?;
        }
        Ok(make_state(session, true, String::new(), false))
    }

    fn request_candidates(&mut self, session: &mut SessionState) -> SessionRequestResult<()> {
        self.request_candidates_with_context(session, false, None)
    }

    fn request_rich_candidates(&mut self, session: &mut SessionState) -> SessionRequestResult<()> {
        self.request_candidates_with_context(session, true, None)
    }

    fn request_candidates_with_context(
        &mut self,
        session: &mut SessionState,
        request_rich_candidates: bool,
        left_context: Option<&str>,
    ) -> SessionRequestResult<()> {
        let segment_request = session.segment_surface_count.is_some();
        if segment_request {
            session.conversion.begin_segment_request(&self.tables);
        }
        let result = self.request_candidates_for_active_target(
            session,
            request_rich_candidates,
            left_context,
        );
        if segment_request {
            session.conversion.end_segment_request();
            if result.is_ok() {
                session.candidate_remainders = session
                    .display_candidates
                    .iter()
                    .map(|candidate| {
                        session
                            .conversion
                            .remaining_after_candidate(candidate, &self.tables)
                    })
                    .collect();
            }
        }
        result
    }

    fn request_candidates_for_active_target(
        &mut self,
        session: &mut SessionState,
        request_rich_candidates: bool,
        left_context: Option<&str>,
    ) -> SessionRequestResult<()> {
        session.conversion.refresh_learning().map_err(|error| {
            (
                Code::Internal,
                format!("learning memory refresh failed: {error}"),
            )
        })?;
        let converter = NormalConverter::new(&self.dictionary);
        let result = if let Some(model) = self.zenz_model.as_deref_mut() {
            let surrounding = surrounding_with_left_context(&session.surrounding, left_context);
            let version = version_with_context(&self.zenz_version, &surrounding);
            zenz::convert(
                &mut session.conversion,
                &converter,
                &self.tables,
                model,
                &mut self.zenz_evaluator,
                &mut session.zenz_cache,
                zenz::ZenzConversionOptions {
                    version: &version,
                    request_rich_candidates: self.zenz_rich_candidates || request_rich_candidates,
                    inference_limit: self.zenz_inference_limit,
                    personalization: self.zenz_personalization.as_ref(),
                },
            )
            .map_err(|error| {
                (
                    Code::Internal,
                    format!("Zenz candidate generation failed: {error}"),
                )
            })?;
            let prediction_override = if self.zenz_predictive_input
                && session.request_options.japanese_prediction != PredictionMode::Disabled
            {
                Some(
                    session
                        .conversion
                        .request_zenz_prediction(
                            &converter,
                            &self.tables,
                            model,
                            &version,
                            surrounding.left.as_deref().unwrap_or(""),
                            |prediction_session, model| {
                                let mut prediction_cache = zenz::ZenzConversionCache::default();
                                zenz::convert(
                                    prediction_session,
                                    &converter,
                                    &self.tables,
                                    model,
                                    &mut self.zenz_evaluator,
                                    &mut prediction_cache,
                                    zenz::ZenzConversionOptions {
                                        version: &version,
                                        request_rich_candidates: false,
                                        inference_limit: self.zenz_inference_limit,
                                        personalization: self.zenz_personalization.as_ref(),
                                    },
                                )
                                .map_err(|error| match error {
                                    zenz::ZenzConversionError::Dictionary(error) => {
                                        beankey_converter::ZenzPredictionError::Dictionary(error)
                                    }
                                    zenz::ZenzConversionError::Inference(error) => {
                                        beankey_converter::ZenzPredictionError::Inference(error)
                                    }
                                })
                            },
                        )
                        .map_err(|error| {
                            (
                                Code::Internal,
                                format!("Zenz input prediction failed: {error}"),
                            )
                        })?,
                )
            } else {
                None
            };
            session
                .conversion
                .finalize_zenz_request_with_prediction_override(
                    &converter,
                    &self.tables,
                    session.request_options.clone(),
                    prediction_override,
                )
                .map_err(|error| {
                    (
                        Code::Internal,
                        format!("candidate assembly failed: {error}"),
                    )
                })?
        } else {
            session
                .conversion
                .request(&converter, &self.tables, session.request_options.clone())
                .map_err(|error| {
                    (
                        Code::Internal,
                        format!("candidate generation failed: {error}"),
                    )
                })?
        };
        let live_candidate = if self.live_conversion
            && session.input_mode == InputMode::Composing
            && session.input_language == InputLanguage::Japanese
            && session.segment_surface_count.is_none()
            && session.conversion.composing().is_at_end()
            && !session.last_was_backspace
            && session.conversion.composing().surface_graphemes().len() > 1
        {
            result.main_results.first().cloned()
        } else {
            None
        };
        session.display_candidates = if session.segment_surface_count.is_some() {
            result.main_results.clone()
        } else {
            session.conversion.desktop_candidates(&result, &self.tables)
        };
        session.candidate_remainders = session
            .display_candidates
            .iter()
            .map(|candidate| {
                session
                    .conversion
                    .remaining_after_candidate(candidate, &self.tables)
            })
            .collect();
        session.candidate_annotations = vec![String::new(); session.display_candidates.len()];
        session.preview_candidate_index = result
            .main_results
            .first()
            .filter(|preview| {
                session
                    .conversion
                    .remaining_after_candidate(preview, &self.tables)
                    .is_empty()
            })
            .and_then(|preview| {
                session.display_candidates.iter().position(|candidate| {
                    candidate.text == preview.text
                        && candidate.composing_count == preview.composing_count
                })
            });
        session.additional_candidate_count = 0;
        session.prediction = input_prediction(session, &result);
        if let Some(live_candidate) = &live_candidate
            && let Some(index) = session.display_candidates.iter().position(|candidate| {
                candidate.text == live_candidate.text
                    && candidate.composing_count == live_candidate.composing_count
            })
        {
            session.selected_candidate = index;
        }
        session.live_candidate = live_candidate;
        Ok(())
    }
}

fn insert_event(
    session: &mut SessionState,
    event: &protocol::KeyEvent,
    tables: &InputTableRegistry,
) {
    if session.input_language == InputLanguage::English {
        let input = if event.input.is_empty() {
            event.text.as_str()
        } else {
            event.input.as_str()
        };
        session
            .conversion
            .insert_str(input, ConverterInputStyle::Direct, tables);
        return;
    }
    if matches!(session.input_style, ConverterInputStyle::Mapped(_))
        && (!event.input.is_empty() || !event.intention.is_empty())
    {
        let input = if event.input.is_empty() {
            event.text.clone()
        } else {
            event.input.clone()
        };
        session.conversion.insert_key(
            (!event.intention.is_empty()).then(|| event.intention.clone()),
            input,
            event.shift.then_some(InputModifier::Shift),
            session.input_style.clone(),
            tables,
        );
    } else {
        session
            .conversion
            .insert_str(&event.text, session.input_style.clone(), tables);
    }
}

fn selection_number(text: &str) -> Option<usize> {
    (text.chars().count() == 1)
        .then(|| text.chars().next())
        .flatten()
        .and_then(|character| character.to_digit(10))
        .map(|number| number as usize)
}

fn current_marked_text(session: &SessionState, _tables: &InputTableRegistry) -> String {
    if session.input_mode == InputMode::Previewing {
        return session
            .preview_candidate_index
            .and_then(|index| session.display_candidates.get(index))
            .map_or_else(
                || session.conversion.composing().surface(),
                |candidate| candidate.text.clone(),
            );
    }
    if session.input_mode == InputMode::Selecting
        && let Some(candidate) = session.display_candidates.get(session.selected_candidate)
    {
        let remaining = session
            .candidate_remainders
            .get(session.selected_candidate)
            .map(String::as_str)
            .unwrap_or("");
        return format!("{}{remaining}", candidate.text);
    }
    session.live_candidate.as_ref().map_or_else(
        || session.conversion.composing().surface(),
        |candidate| candidate.text.clone(),
    )
}

fn input_prediction(session: &SessionState, result: &ConversionResult) -> Option<InputPrediction> {
    if session.last_was_backspace
        && let Some(candidate) = result
            .main_results
            .iter()
            .find(|candidate| candidate.is_typo_correction)
    {
        let corrected_reading = to_hiragana(
            &candidate
                .entries
                .iter()
                .map(|entry| entry.ruby.as_str())
                .collect::<String>(),
        );
        if corrected_reading != session.conversion.composing().surface() {
            return Some(InputPrediction {
                display_text: candidate.text.clone(),
                append_text: corrected_reading,
                delete_count: session.conversion.composing().surface_graphemes().len(),
            });
        }
    }
    if session.request_options.japanese_prediction != PredictionMode::Manual {
        return None;
    }
    let surface = session.conversion.composing().surface();
    let mut target = surface.as_str();
    let mut delete_count = 0;
    if surface
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_alphabetic())
    {
        let last = surface.char_indices().next_back()?.0;
        target = &surface[..last];
        delete_count = 1;
    }
    if target.chars().count() < 2 {
        return None;
    }
    let target = to_hiragana(target);
    result.prediction_results.iter().find_map(|candidate| {
        let reading = candidate
            .entries
            .iter()
            .map(|entry| entry.ruby.as_str())
            .collect::<String>();
        let reading = to_hiragana(&reading);
        let append_text = reading.strip_prefix(&target)?;
        (!append_text.is_empty()).then(|| InputPrediction {
            display_text: candidate.text.clone(),
            append_text: append_text.to_owned(),
            delete_count,
        })
    })
}

fn valid_event_text(event: &protocol::KeyEvent) -> bool {
    !event.text.is_empty()
        && !event.text.chars().any(char::is_control)
        && !event.input.chars().any(char::is_control)
        && !event.intention.chars().any(char::is_control)
}

fn option_direct_input_text(event: &protocol::KeyEvent, type_backslash: bool) -> Option<String> {
    let input = if event.input.is_empty() {
        event.text.as_str()
    } else {
        event.input.as_str()
    };
    if input.is_empty() || input.chars().any(char::is_control) {
        return None;
    }
    let normalized = match input {
        "¥" | "\\" if type_backslash => "\\",
        "¥" | "\\" => "¥",
        _ => input,
    };
    Some(to_full_width(normalized))
}

fn normalize_input_event(event: &mut protocol::KeyEvent, behavior: InputBehavior) {
    if protocol::UserAction::try_from(event.action) != Ok(protocol::UserAction::Input) {
        return;
    }
    let input = if event.input.is_empty() {
        event.text.as_str()
    } else {
        event.input.as_str()
    };
    let normalized = match input {
        "¥" | "\\" if event.shift => Some("｜"),
        "¥" | "\\" if behavior.type_backslash != event.option => Some("＼"),
        "¥" | "\\" => Some("￥"),
        "," if !event.shift => Some(behavior.punctuation_style.comma(event.option)),
        "." if !event.shift => Some(behavior.punctuation_style.period(event.option)),
        "/" | "?" if event.option && event.shift => Some("…"),
        "/" if event.option => Some("／"),
        "[" | "{" if event.option && event.shift => Some("｛"),
        "[" if event.option => Some("［"),
        "]" | "}" if event.option && event.shift => Some("｝"),
        "]" if event.option => Some("］"),
        _ => japanese_symbol_intention(input),
    };
    if let Some(normalized) = normalized {
        event.text = normalized.to_owned();
        event.intention = normalized.to_owned();
    } else if event.option
        && !input.is_empty()
        && input.chars().all(|character| !character.is_control())
        && !input.is_ascii()
    {
        let generated = input.to_owned();
        event.text.clone_from(&generated);
        event.intention = generated;
    }
}

fn japanese_symbol_intention(input: &str) -> Option<&'static str> {
    match input {
        "!" => Some("！"),
        "\"" => Some("”"),
        "#" => Some("＃"),
        "$" => Some("＄"),
        "%" => Some("％"),
        "&" => Some("＆"),
        "'" => Some("’"),
        "(" => Some("（"),
        ")" => Some("）"),
        "=" => Some("＝"),
        "~" => Some("〜"),
        "|" => Some("｜"),
        "`" => Some("｀"),
        "{" => Some("『"),
        "+" => Some("＋"),
        "*" => Some("＊"),
        "}" => Some("』"),
        "<" => Some("＜"),
        ">" => Some("＞"),
        "?" => Some("？"),
        "_" => Some("＿"),
        "-" => Some("ー"),
        "^" => Some("＾"),
        "\\" => Some("＼"),
        "¥" => Some("￥"),
        "@" => Some("＠"),
        "[" => Some("「"),
        ";" => Some("；"),
        ":" => Some("："),
        "]" => Some("」"),
        "," => Some("、"),
        "." => Some("。"),
        "/" => Some("・"),
        _ => None,
    }
}

impl PunctuationStyleConfig {
    fn comma(self, inverted: bool) -> &'static str {
        let comma = if matches!(self, Self::KutenAndComma | Self::PeriodAndComma) {
            "，"
        } else {
            "、"
        };
        if inverted {
            if comma == "，" { "、" } else { "，" }
        } else {
            comma
        }
    }

    fn period(self, inverted: bool) -> &'static str {
        let period = if matches!(self, Self::PeriodAndToten | Self::PeriodAndComma) {
            "．"
        } else {
            "。"
        };
        if inverted {
            if period == "．" { "。" } else { "．" }
        } else {
            period
        }
    }
}

fn surrounding_context(
    surrounding: Option<&protocol::SurroundingText>,
) -> Result<SurroundingContext, String> {
    let Some(surrounding) = surrounding.filter(|surrounding| surrounding.available) else {
        return Ok(SurroundingContext::default());
    };
    let character_count = surrounding.text.chars().count();
    let cursor = surrounding.cursor as usize;
    let anchor = surrounding.anchor as usize;
    if cursor > character_count || anchor > character_count {
        return Err(format!(
            "surrounding text cursor {cursor} and anchor {anchor} exceed {character_count} characters"
        ));
    }
    let selection_start = cursor.min(anchor);
    let selection_end = cursor.max(anchor);
    let left: String = surrounding.text.chars().take(selection_start).collect();
    let left_line = left.rsplit('\n').next().unwrap_or("").trim_start();
    let left = left_line
        .chars()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let right: String = surrounding.text.chars().skip(selection_end).collect();
    let right = right
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end()
        .chars()
        .take(30)
        .collect();
    Ok(SurroundingContext {
        left: Some(left),
        right: Some(right),
    })
}

fn version_with_context(
    version: &ZenzVersionConfig,
    surrounding: &SurroundingContext,
) -> ZenzVersionConfig {
    match version.clone() {
        ZenzVersionConfig::V2(mut config) => {
            config.left_context.clone_from(&surrounding.left);
            ZenzVersionConfig::V2(config)
        }
        ZenzVersionConfig::V3(mut config) => {
            config.left_context.clone_from(&surrounding.left);
            config.right_context.clone_from(&surrounding.right);
            ZenzVersionConfig::V3(config)
        }
    }
}

fn surrounding_with_left_context(
    surrounding: &SurroundingContext,
    left_context: Option<&str>,
) -> SurroundingContext {
    let mut surrounding = surrounding.clone();
    if let Some(left_context) = left_context {
        surrounding.left = Some(left_context.to_owned());
    }
    surrounding
}

fn input_style(
    value: i32,
    custom_table: &str,
    tables: &InputTableRegistry,
) -> Option<ConverterInputStyle> {
    match protocol::InputStyle::try_from(value).ok()? {
        protocol::InputStyle::Unspecified => None,
        protocol::InputStyle::Direct => Some(ConverterInputStyle::Direct),
        protocol::InputStyle::RomanToKana => Some(ConverterInputStyle::RomanToKana),
        protocol::InputStyle::Azik => Some(ConverterInputStyle::Mapped(InputTableId::DefaultAzik)),
        protocol::InputStyle::KanaJis => {
            Some(ConverterInputStyle::Mapped(InputTableId::DefaultKanaJis))
        }
        protocol::InputStyle::KanaUs => {
            Some(ConverterInputStyle::Mapped(InputTableId::DefaultKanaUs))
        }
        protocol::InputStyle::Custom => tables
            .contains(custom_table)
            .then(|| ConverterInputStyle::Mapped(InputTableId::Named(custom_table.into()))),
    }
}

fn keyboard_language(value: i32) -> Option<KeyboardLanguage> {
    match protocol::KeyboardLanguage::try_from(value).ok()? {
        protocol::KeyboardLanguage::Unspecified => None,
        protocol::KeyboardLanguage::Japanese => Some(KeyboardLanguage::Japanese),
        protocol::KeyboardLanguage::EnglishUs => Some(KeyboardLanguage::EnglishUs),
        protocol::KeyboardLanguage::Greek => Some(KeyboardLanguage::Greek),
    }
}

fn prediction_mode(value: PredictionConfig) -> PredictionMode {
    match value {
        PredictionConfig::Automatic => PredictionMode::Automatic,
        PredictionConfig::Manual => PredictionMode::Manual,
        PredictionConfig::Disabled => PredictionMode::Disabled,
    }
}

fn default_request_options() -> RequestOptions {
    RequestOptions {
        version_string: Some(format!("beanKey {}", env!("CARGO_PKG_VERSION"))),
        ..RequestOptions::default()
    }
}

fn desktop_dynamic_shortcuts() -> Vec<DictionaryEntry> {
    const DATE_FORMATS: [(&str, f32, &str); 7] = [
        ("M/d", -18.0, "western"),
        ("yyyy/MM/dd", -18.1, "western"),
        ("yyyy-MM-dd", -18.2, "western"),
        ("M月d日（E）", -18.3, "western"),
        ("yyyy年M月d日", -18.4, "western"),
        ("Gyyyy年M月d日", -18.5, "japanese"),
        ("E曜日", -18.6, "western"),
    ];
    const RELATIVE_DATES: [(&str, i64, i64); 5] = [
        ("オトトイ", -2, 86_400),
        ("キノウ", -1, 86_400),
        ("キョウ", 0, 1),
        ("アシタ", 1, 86_400),
        ("アサッテ", 2, 86_400),
    ];

    let mut shortcuts = Vec::with_capacity(41);
    for (format, value, calendar) in DATE_FORMATS {
        shortcuts.extend(RELATIVE_DATES.map(|(ruby, delta, delta_unit)| {
            date_shortcut(format, calendar, ruby, delta, delta_unit, value)
        }));
    }
    shortcuts.extend([
        date_shortcut("MM月", "western", "コンゲツ", 0, 1, -18.0),
        date_shortcut("yyyy年", "western", "コトシ", 0, 1, -18.0),
        date_shortcut("Gyyyy年", "japanese", "コトシ", 0, 1, -18.1),
        date_shortcut("HH:mm", "western", "イマ", 0, 1, -18.0),
        date_shortcut("HH時mm分", "western", "イマ", 0, 1, -18.1),
        date_shortcut("aK時mm分", "western", "イマ", 0, 1, -18.2),
    ]);
    shortcuts
}

fn date_shortcut(
    format: &str,
    calendar: &str,
    ruby: &str,
    delta: i64,
    delta_unit: i64,
    value: f32,
) -> DictionaryEntry {
    DictionaryEntry {
        word: format!(
            r#"<date format="{format}" type="{calendar}" language="ja_JP" delta="{delta}" deltaunit="{delta_unit}">"#
        ),
        ruby: ruby.to_owned(),
        left_id: DESKTOP_PROPER_NOUN_CID,
        right_id: DESKTOP_PROPER_NOUN_CID,
        meaning_id: DESKTOP_GENERAL_MID,
        base_value: value,
        adjustment: 0.0,
        metadata: DictionaryMetadata::default(),
    }
}

fn default_user_dictionary() -> Vec<DictionaryEntry> {
    [("azooKey", "アズーキー"), ("beanKey", "ビーンキー")]
        .into_iter()
        .map(|(word, ruby)| desktop_user_dictionary_entry(word.into(), ruby.into()))
        .collect()
}

fn desktop_user_dictionary_entry(word: String, ruby: String) -> DictionaryEntry {
    DictionaryEntry {
        word,
        ruby,
        left_id: DESKTOP_PROPER_NOUN_CID,
        right_id: DESKTOP_PROPER_NOUN_CID,
        meaning_id: DESKTOP_GENERAL_MID,
        base_value: DESKTOP_USER_DICTIONARY_VALUE,
        adjustment: 0.0,
        metadata: DictionaryMetadata::USER_DICTIONARY,
    }
}

fn typo_correction_mode(value: TypoCorrectionConfig) -> TypoCorrectionMode {
    match value {
        TypoCorrectionConfig::Enabled => TypoCorrectionMode::Enabled,
        TypoCorrectionConfig::Automatic => TypoCorrectionMode::Automatic,
        TypoCorrectionConfig::Disabled => TypoCorrectionMode::Disabled,
    }
}

fn page_candidates(session: &mut SessionState, page: protocol::PageCandidates) -> bool {
    let count = active_candidate_count(session);
    if page.page_size == 0 {
        return false;
    }
    if count == 0 {
        return true;
    }
    let page_size = page.page_size as usize;
    session.selected_candidate =
        match protocol::page_candidates::Direction::try_from(page.direction) {
            Ok(protocol::page_candidates::Direction::Previous) => {
                session.selected_candidate.saturating_sub(page_size)
            }
            Ok(protocol::page_candidates::Direction::Next) => {
                (session.selected_candidate + page_size).min(count - 1)
            }
            _ => return false,
        };
    true
}

fn active_candidate_count(session: &SessionState) -> usize {
    session.display_candidates.len()
}

fn make_state(
    session: &SessionState,
    consumed: bool,
    commit: String,
    reset: bool,
) -> protocol::StateResponse {
    let (preedit, highlighted_preedit_length) = match session.input_mode {
        InputMode::Previewing => session
            .preview_candidate_index
            .and_then(|index| session.display_candidates.get(index))
            .map_or_else(
                || (session.conversion.composing().surface(), 0),
                |candidate| (candidate.text.clone(), 0),
            ),
        InputMode::Selecting => session
            .display_candidates
            .get(session.selected_candidate)
            .map_or_else(
                || (session.conversion.composing().surface(), 0),
                |candidate| {
                    let remaining = session
                        .candidate_remainders
                        .get(session.selected_candidate)
                        .map(String::as_str)
                        .unwrap_or("");
                    let highlighted = candidate.text.chars().count().min(u32::MAX as usize) as u32;
                    (format!("{}{remaining}", candidate.text), highlighted)
                },
            ),
        InputMode::Composing => session.live_candidate.as_ref().map_or_else(
            || (session.conversion.composing().surface(), 0),
            |candidate| (candidate.text.clone(), 0),
        ),
        InputMode::None => (String::new(), 0),
        InputMode::Unicode => (format!("U+{}", session.unicode_input), 0),
    };
    let preedit_cursor = preedit.chars().count().min(u32::MAX as usize) as u32;
    let candidate_window = match session.input_mode {
        InputMode::Selecting => protocol::CandidateWindow::Selecting,
        InputMode::Composing
            if session.input_language == InputLanguage::Japanese
                && !session.live_conversion_enabled =>
        {
            protocol::CandidateWindow::Preview
        }
        _ => protocol::CandidateWindow::Hidden,
    };
    let candidate_indices: Vec<usize> = match candidate_window {
        protocol::CandidateWindow::Selecting => (0..session.display_candidates.len()).collect(),
        protocol::CandidateWindow::Preview => session
            .preview_candidate_index
            .into_iter()
            .chain(
                (0..session.display_candidates.len())
                    .filter(|index| Some(*index) != session.preview_candidate_index),
            )
            .collect(),
        _ => (0..session.display_candidates.len()).collect(),
    };
    let candidates = candidate_indices
        .into_iter()
        .filter_map(|index| {
            session.display_candidates.get(index).map(|candidate| {
                candidate_to_protocol(
                    index,
                    candidate,
                    session
                        .candidate_annotations
                        .get(index)
                        .map(String::as_str)
                        .unwrap_or(""),
                )
            })
        })
        .collect();
    protocol::StateResponse {
        consumed,
        preedit,
        preedit_cursor,
        candidates,
        selected_candidate: if session.input_mode == InputMode::Selecting {
            session.selected_candidate as i32
        } else {
            -1
        },
        commit,
        reset,
        lm_typo_available: session.lm_typo_available,
        learning_available: session.learning_available,
        learning_writable: session.learning_writable,
        input_state: match session.input_mode {
            InputMode::None => protocol::InputState::None,
            InputMode::Composing => protocol::InputState::Composing,
            InputMode::Previewing => protocol::InputState::Previewing,
            InputMode::Selecting => protocol::InputState::Selecting,
            InputMode::Unicode => protocol::InputState::Unicode,
        } as i32,
        candidate_window: candidate_window as i32,
        highlighted_preedit_length,
        prediction: (session.input_mode == InputMode::Composing)
            .then_some(session.prediction.as_ref())
            .flatten()
            .map(|prediction| protocol::Prediction {
                display_text: prediction.display_text.clone(),
                append_text: prediction.append_text.clone(),
                delete_count: prediction.delete_count.min(u32::MAX as usize) as u32,
            }),
        input_language: match session.input_language {
            InputLanguage::Japanese => protocol::InputLanguage::Japanese,
            InputLanguage::English => protocol::InputLanguage::English,
        } as i32,
    }
}

fn candidate_to_protocol(
    index: usize,
    candidate: &beankey_converter::Candidate,
    annotation: &str,
) -> protocol::Candidate {
    protocol::Candidate {
        text: candidate.text.clone(),
        value: candidate.value,
        composing_count: Some(composing_count_to_protocol(&candidate.composing_count)),
        annotation: annotation.to_owned(),
        index: index.min(u32::MAX as usize) as u32,
    }
}

fn composing_count_to_protocol(count: &ConverterComposingCount) -> protocol::ComposingCount {
    let count = match count {
        ConverterComposingCount::Input(count) => {
            Count::Input((*count).min(u32::MAX as usize) as u32)
        }
        ConverterComposingCount::Surface(count) => {
            Count::Surface((*count).min(u32::MAX as usize) as u32)
        }
        ConverterComposingCount::Composite(left, right) => {
            Count::Composite(Box::new(protocol::CompositeComposingCount {
                left: Some(Box::new(composing_count_to_protocol(left))),
                right: Some(Box::new(composing_count_to_protocol(right))),
            }))
        }
    };
    protocol::ComposingCount { count: Some(count) }
}

fn state_envelope(
    request_id: u64,
    session_id: String,
    response: protocol::StateResponse,
) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id,
        payload: Some(Payload::StateResponse(response)),
        trace: Vec::new(),
    }
}

fn typo_correction_envelope(
    request_id: u64,
    session_id: String,
    candidates: Vec<beankey_converter::LmTypoCandidate>,
) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id,
        payload: Some(Payload::TypoCorrectionResponse(
            protocol::TypoCorrectionResponse {
                candidates: candidates
                    .into_iter()
                    .map(|candidate| protocol::TypoCorrectionCandidate {
                        corrected_input: candidate.corrected_input,
                        converted_text: candidate.converted_text,
                        score: candidate.score,
                        language_model_score: candidate.lm_score,
                        channel_cost: candidate.channel_cost,
                        prominence: candidate.prominence,
                    })
                    .collect(),
            },
        )),
        trace: Vec::new(),
    }
}

fn error_envelope(
    request_id: u64,
    session_id: String,
    code: Code,
    message: impl Into<String>,
) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id,
        payload: Some(Payload::ProtocolError(protocol::ProtocolError {
            code: code as i32,
            message: message.into(),
        })),
        trace: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use beankey_converter::{ForeignLanguage, ZenzInferenceError, ZenzV3Config, ZenzVersionConfig};

    use super::*;

    struct GreekCompleter;

    struct LivePrefixModel;

    impl ZenzLanguageModel for LivePrefixModel {
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
            Ok(if add_special {
                vec![1]
            } else if text == "箸" {
                vec![4]
            } else {
                vec![3]
            })
        }

        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(if token == 4 {
                "箸".as_bytes().to_vec()
            } else {
                b"x".to_vec()
            })
        }

        fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            Ok(vec![0.0, 0.0, 0.0, 1.0, 10.0])
        }
    }

    impl ForeignCompletionProvider for GreekCompleter {
        fn completions(&self, language: ForeignLanguage, input: &str) -> Vec<String> {
            if language == ForeignLanguage::Greek && input == "καλ" {
                vec!["καλά".into()]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn mirrors_the_complete_desktop_dynamic_shortcut_set() {
        let shortcuts = desktop_dynamic_shortcuts();

        assert_eq!(shortcuts.len(), 41);
        assert_eq!(
            shortcuts
                .iter()
                .filter(|entry| entry.ruby == "キョウ")
                .count(),
            7
        );
        assert_eq!(
            shortcuts
                .iter()
                .filter(|entry| entry.ruby == "イマ")
                .count(),
            3
        );
        assert!(shortcuts.iter().all(|entry| {
            entry.left_id == DESKTOP_PROPER_NOUN_CID
                && entry.right_id == DESKTOP_PROPER_NOUN_CID
                && entry.meaning_id == DESKTOP_GENERAL_MID
        }));
    }

    #[test]
    fn uses_the_desktop_score_for_built_in_product_names() {
        let entries = default_user_dictionary();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| {
            entry.base_value == DESKTOP_USER_DICTIONARY_VALUE
                && entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY)
        }));
    }

    #[test]
    fn uses_the_finalized_zenz_result_for_live_conversion() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let mut engine =
            Engine::open_with_zenz_model(dictionary, Box::new(LivePrefixModel)).unwrap();
        engine.live_conversion = true;
        let envelope = |request_id, payload| protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: "live-zenz".into(),
            payload: Some(payload),
            trace: Vec::new(),
        };
        engine.handle(envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Direct as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                custom_input_table: String::new(),
            }),
        ));

        let response = engine.handle(envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "はし".into(),
                ..Default::default()
            }),
        ));
        let Payload::StateResponse(state) = response.payload.unwrap() else {
            panic!("conversion did not return state");
        };

        assert_eq!(state.preedit, "箸");
        assert_eq!(state.highlighted_preedit_length, 0);
    }

    #[test]
    fn learns_a_live_candidate_committed_with_enter() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let learning_directory = tempfile::tempdir().unwrap();
        let mut engine =
            Engine::open_with_zenz_model(dictionary, Box::new(LivePrefixModel)).unwrap();
        engine
            .load_learning(learning_directory.path(), &LearningConfig::default())
            .unwrap();
        engine.live_conversion = true;
        let envelope = |request_id, payload| protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: "live-enter-learning".into(),
            payload: Some(payload),
            trace: Vec::new(),
        };
        engine.handle(envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Direct as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                custom_input_table: String::new(),
            }),
        ));
        engine.handle(envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "はし".into(),
                ..Default::default()
            }),
        ));

        let response = engine.handle(envelope(
            3,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Enter as i32,
                ..Default::default()
            }),
        ));
        let Payload::StateResponse(state) = response.payload.unwrap() else {
            panic!("conversion did not return state");
        };

        assert_eq!(state.commit, "箸");
        assert!(!learning_directory.path().join("memory.louds").exists());
        engine.handle(envelope(4, Payload::EndSession(protocol::EndSession {})));
        assert!(learning_directory.path().join("memory.louds").exists());
    }

    #[test]
    fn distinguishes_read_only_learning_management_from_candidate_forgetting() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let state = tempfile::tempdir().unwrap();
        let mut engine = Engine::open(dictionary).unwrap();
        engine
            .load_learning(
                state.path(),
                &LearningConfig {
                    mode: LearningModeConfig::OnlyOutput,
                    ..Default::default()
                },
            )
            .unwrap();

        let response = engine.handle(protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: 1,
            session_id: "read-only-learning".into(),
            payload: Some(Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::RomanToKana as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                custom_input_table: String::new(),
            })),
            trace: Vec::new(),
        });
        let Some(Payload::StateResponse(response)) = response.payload else {
            panic!("session start did not return state");
        };
        assert!(response.learning_available);
        assert!(!response.learning_writable);
    }

    #[test]
    fn uses_the_configured_keyboard_language_for_unspecified_sessions() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let mut engine = Engine::open(dictionary).unwrap();
        engine.foreign_completion_provider = Some(Arc::new(GreekCompleter));
        engine
            .apply_conversion_options(&ConversionConfig {
                input_style: InputStyleConfig::Direct,
                keyboard_language: KeyboardLanguageConfig::Greek,
                foreign_prediction: PredictionConfig::Automatic,
                ..Default::default()
            })
            .unwrap();
        let envelope = |request_id, payload| protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: "greek".into(),
            payload: Some(payload),
            trace: Vec::new(),
        };
        engine.handle(envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Unspecified as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Unspecified as i32,
                custom_input_table: String::new(),
            }),
        ));

        let response = engine.handle(envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "καλ".into(),
                ..Default::default()
            }),
        ));
        let Payload::StateResponse(state) = response.payload.unwrap() else {
            panic!("conversion did not return state");
        };

        assert!(
            state
                .candidates
                .iter()
                .any(|candidate| candidate.text == "καλά")
        );
    }

    #[test]
    fn splits_fcitx_surrounding_text_around_the_selection() {
        let context = surrounding_context(Some(&protocol::SurroundingText {
            available: true,
            text: "甲😀選択乙".into(),
            cursor: 4,
            anchor: 2,
        }))
        .unwrap();

        assert_eq!(context.left.as_deref(), Some("甲😀"));
        assert_eq!(context.right.as_deref(), Some("乙"));
    }

    #[test]
    fn limits_dynamic_context_to_thirty_trimmed_characters_on_the_current_line() {
        let left = format!("ignored line\n   {}", "あ".repeat(35));
        let text = format!("{left}選択{}   \nignored", "い".repeat(35));
        let left_count = left.chars().count();
        let context = surrounding_context(Some(&protocol::SurroundingText {
            available: true,
            text,
            cursor: (left_count + 2) as u32,
            anchor: left_count as u32,
        }))
        .unwrap();

        assert_eq!(
            context.left.as_deref(),
            Some("ああああああああああああああああああああああああああああああ")
        );
        assert_eq!(
            context.right.as_deref(),
            Some("いいいいいいいいいいいいいいいいいいいいいいいいいいいいいい")
        );
    }

    #[test]
    fn rejects_out_of_range_surrounding_text_positions() {
        let error = surrounding_context(Some(&protocol::SurroundingText {
            available: true,
            text: "甲😀".into(),
            cursor: 3,
            anchor: 2,
        }))
        .unwrap_err();

        assert!(error.contains("exceed 2 characters"));
    }

    #[test]
    fn applies_dynamic_context_without_replacing_zenz_conditions() {
        let version = ZenzVersionConfig::V3(ZenzV3Config {
            profile: Some("profile".into()),
            left_context: Some("stale left".into()),
            right_context: Some("stale right".into()),
            ..Default::default()
        });
        let updated = version_with_context(
            &version,
            &SurroundingContext {
                left: Some("left".into()),
                right: Some("right".into()),
            },
        );

        assert_eq!(
            updated,
            ZenzVersionConfig::V3(ZenzV3Config {
                profile: Some("profile".into()),
                left_context: Some("left".into()),
                right_context: Some("right".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn applies_static_zenz_configuration() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let mut engine = Engine::open(dictionary).unwrap();
        engine.apply_zenz_options(&crate::ZenzConfig {
            inference_limit: 7,
            rich_candidates: true,
            predictive_input: true,
            profile: Some("profile".into()),
            topic: Some("topic".into()),
            style: Some("style".into()),
            preference: Some("preference".into()),
            enable_alignment_separator: true,
            personalization: None,
        });

        assert_eq!(engine.zenz_inference_limit, 7);
        assert!(engine.zenz_rich_candidates);
        assert!(engine.zenz_predictive_input);
        assert_eq!(
            engine.zenz_version,
            ZenzVersionConfig::V3(ZenzV3Config {
                profile: Some("profile".into()),
                topic: Some("topic".into()),
                style: Some("style".into()),
                preference: Some("preference".into()),
                enable_alignment_separator: true,
                ..Default::default()
            })
        );
    }

    #[test]
    fn loads_fixed_format_personalization_models() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let ngram = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../converter/tests/data/ngram");
        let mut engine = Engine::open(dictionary).unwrap();
        let config = crate::ZenzConfig {
            personalization: Some(crate::PersonalizationConfig {
                base_ngram: ngram.join("lm"),
                personal_ngram: ngram.join("personal"),
                alpha: 0.5,
            }),
            ..Default::default()
        };

        engine.load_zenz_personalization(&config).unwrap();

        assert!(engine.zenz_personalization.is_some());
    }

    #[test]
    fn returns_lm_typo_candidates_without_mutating_composition() {
        let dictionary = PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        );
        let ngram =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../converter/tests/data/ngram/lm");
        let tokenizer = std::env::var_os("BEANKEY_TEST_ZENZ_TOKENIZER")
            .expect("BEANKEY_TEST_ZENZ_TOKENIZER must point to tokenizer.json");
        let mut engine = Engine::open(dictionary).unwrap();
        engine
            .load_lm_typo(&crate::LmTypoCorrectionConfig {
                enabled: true,
                language_model: crate::LmTypoLanguageModel::Ngram,
                ngram: Some(crate::TypoNGramConfig {
                    prefix: ngram,
                    tokenizer: tokenizer.into(),
                    n: 2,
                    discount: 0.75,
                }),
                max_steps: Some(0),
                ..Default::default()
            })
            .unwrap();
        let envelope = |request_id, payload| protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: "typo".into(),
            payload: Some(payload),
            trace: Vec::new(),
        };
        engine.handle(envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::RomanToKana as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                custom_input_table: String::new(),
            }),
        ));
        engine.handle(envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "kana".into(),
                ..Default::default()
            }),
        ));

        let correction = engine.handle(envelope(
            3,
            Payload::RequestTypoCorrections(protocol::RequestTypoCorrections {}),
        ));
        let Payload::TypoCorrectionResponse(correction) = correction.payload.unwrap() else {
            panic!("LM typo correction did not return its dedicated response");
        };
        assert_eq!(correction.candidates[0].corrected_input, "kana");
        let expected_commit = correction.candidates[0].converted_text.clone();

        let invalid = engine.handle(envelope(
            4,
            Payload::SelectTypoCorrection(protocol::SelectTypoCorrection {
                index: correction.candidates.len() as u32,
            }),
        ));
        assert!(matches!(invalid.payload, Some(Payload::ProtocolError(_))));
        let stale = engine.handle(envelope(
            5,
            Payload::SelectTypoCorrection(protocol::SelectTypoCorrection { index: 0 }),
        ));
        assert!(matches!(stale.payload, Some(Payload::ProtocolError(_))));

        engine.handle(envelope(
            6,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "kana".into(),
                ..Default::default()
            }),
        ));
        engine.handle(envelope(
            7,
            Payload::RequestTypoCorrections(protocol::RequestTypoCorrections {}),
        ));
        let selected = engine.handle(envelope(
            8,
            Payload::SelectTypoCorrection(protocol::SelectTypoCorrection { index: 0 }),
        ));
        let Payload::StateResponse(selected) = selected.payload.unwrap() else {
            panic!("LM typo correction selection did not return state");
        };
        assert_eq!(selected.commit, expected_commit);
        assert!(selected.consumed);
        assert!(selected.reset);
        assert!(selected.preedit.is_empty());
    }
}
