mod dictionary;
mod foreign;
mod input;
mod kana;
mod lattice;
mod learning;
mod lm_typo;
mod ngram;
mod post_prediction;
mod session;
mod special;
mod template;
mod text_replacer;
mod typo;
mod zenz;

pub use dictionary::{
    CharacterIdMap, DictionaryBinaryError, DictionaryEntry, DictionaryError, DictionaryMatch,
    DictionaryMetadata, DictionaryStore, Louds, LoudsError, MeaningMatrix, UserDictionary,
    escaped_identifier, parse_connection_cost_line, parse_entry_block, parse_entry_shard,
};
pub use foreign::{
    ForeignCompletionProvider, ForeignLanguage, HunspellCompleter, HunspellError, KeyboardLanguage,
};
pub use input::{
    ComposingCount, ComposingText, DifferenceSuffix, FormatError, FormatErrorKind, FormatReport,
    FormatSide, InputElement, InputModifier, InputPiece, InputStyle, InputTable,
    InputTableExportError, InputTableId, InputTableRegistry, KeyElement, ValueElement,
};
pub use kana::{to_hiragana, to_katakana};
pub use lattice::{Candidate, ConversionContext, LatticeRange, NormalConverter};
pub use learning::{LearningError, LearningMemory, LearningMode};
pub use lm_typo::{LmTypoCandidate, LmTypoConfig, experimental_typo_correction};
pub use ngram::{EfficientNGram, NGramError, NGramLanguageModel, ZenzTokenizer};
pub use post_prediction::{
    PostCompositionPrediction, PostCompositionPredictor, PostPredictionKind,
};
pub use session::{
    ConversionResult, ConversionSession, PredictionMode, RequestOptions, SelectionError,
    TextTransform, TypoCorrectionMode, ZenzPredictionError,
};
pub use special::{special_candidates, typographical_candidates};
pub use template::expand_templates;
pub use text_replacer::{ReplacementCandidate, TextReplacer, TextReplacerError, TextSearchResult};
pub use zenz::{
    ALIGNMENT_SEPARATOR, AlternativeConstraint, CandidateEvaluation, PrefixConstraint,
    TokenProbabilityModel, ZenzEvaluationRequest, ZenzEvaluator, ZenzInferenceError,
    ZenzInferenceSequence, ZenzInputGenerationRequest, ZenzLanguageModel, ZenzPersonalization,
    ZenzPromptBuilder, ZenzV2Config, ZenzV3Config, ZenzVersionConfig, evaluate_candidate,
    generate_next_input,
};

pub fn to_full_width(value: &str) -> String {
    lattice::to_full_width(value)
}
