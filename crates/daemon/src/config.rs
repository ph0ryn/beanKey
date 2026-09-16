use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

const EXPECTED_CONTEXT_SIZE: usize = 512;
const EXPECTED_BATCH_SIZE: usize = 512;
const EXPECTED_MICRO_BATCH_SIZE: usize = 64;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    pub dictionary: PathBuf,
    pub model: PathBuf,
    pub emoji_dictionary: PathBuf,
    pub llama_backend_directory: PathBuf,
    pub runtime_socket: PathBuf,
    pub hunspell: HunspellConfig,
    pub conversion: ConversionConfig,
    #[serde(default)]
    pub learning: LearningConfig,
    pub zenz: ZenzConfig,
    #[serde(default)]
    pub lm_typo: LmTypoCorrectionConfig,
    pub inference: InferenceConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConversionConfig {
    pub input_style: InputStyleConfig,
    pub custom_input_table: Option<String>,
    pub keyboard_language: KeyboardLanguageConfig,
    pub n_best: usize,
    pub japanese_prediction: PredictionConfig,
    pub foreign_prediction: PredictionConfig,
    pub full_width_roman: bool,
    pub half_width_kana: bool,
    pub typography: bool,
    pub typo_correction: TypoCorrectionConfig,
    pub live_conversion: bool,
    pub type_backslash: bool,
    pub type_half_space: bool,
    pub option_direct_full_width_input: bool,
    pub punctuation_style: PunctuationStyleConfig,
    pub user_dictionary: Option<PathBuf>,
    pub user_dictionary_directory: Option<PathBuf>,
    pub custom_input_tables: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    pub mode: LearningModeConfig,
    pub max_count: usize,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            mode: LearningModeConfig::InputAndOutput,
            max_count: 65_536,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LearningModeConfig {
    #[default]
    InputAndOutput,
    OnlyOutput,
    Nothing,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            input_style: InputStyleConfig::RomanToKana,
            custom_input_table: None,
            keyboard_language: KeyboardLanguageConfig::Japanese,
            n_best: 10,
            japanese_prediction: PredictionConfig::Disabled,
            foreign_prediction: PredictionConfig::Disabled,
            full_width_roman: true,
            half_width_kana: false,
            typography: false,
            typo_correction: TypoCorrectionConfig::Automatic,
            live_conversion: true,
            type_backslash: false,
            type_half_space: false,
            option_direct_full_width_input: false,
            punctuation_style: PunctuationStyleConfig::KutenAndToten,
            user_dictionary: None,
            user_dictionary_directory: None,
            custom_input_tables: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardLanguageConfig {
    None,
    #[default]
    Japanese,
    EnglishUs,
    Greek,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InputStyleConfig {
    Direct,
    #[default]
    RomanToKana,
    Azik,
    KanaJis,
    KanaUs,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PredictionConfig {
    #[default]
    Automatic,
    Manual,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TypoCorrectionConfig {
    Enabled,
    #[default]
    Automatic,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PunctuationStyleConfig {
    #[default]
    KutenAndToten,
    KutenAndComma,
    PeriodAndToten,
    PeriodAndComma,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ZenzConfig {
    pub inference_limit: usize,
    pub rich_candidates: bool,
    pub predictive_input: bool,
    pub profile: Option<String>,
    pub topic: Option<String>,
    pub style: Option<String>,
    pub preference: Option<String>,
    pub enable_alignment_separator: bool,
    pub personalization: Option<PersonalizationConfig>,
}

impl Default for ZenzConfig {
    fn default() -> Self {
        Self {
            inference_limit: 5,
            rich_candidates: false,
            predictive_input: false,
            profile: None,
            topic: None,
            style: None,
            preference: None,
            enable_alignment_separator: true,
            personalization: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PersonalizationConfig {
    pub base_ngram: PathBuf,
    pub personal_ngram: PathBuf,
    pub alpha: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LmTypoCorrectionConfig {
    pub enabled: bool,
    pub language_model: LmTypoLanguageModel,
    pub ngram: Option<TypoNGramConfig>,
    pub beam_size: usize,
    pub top_k: usize,
    pub n_best: usize,
    pub max_steps: Option<usize>,
    pub substitution_cost: f32,
    pub deletion_cost: f32,
    pub transposition_cost: f32,
}

impl Default for LmTypoCorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            language_model: LmTypoLanguageModel::Zenz,
            ngram: None,
            beam_size: 32,
            top_k: 64,
            n_best: 5,
            max_steps: None,
            substitution_cost: 2.0,
            deletion_cost: 3.0,
            transposition_cost: 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LmTypoLanguageModel {
    #[default]
    Zenz,
    Ngram,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TypoNGramConfig {
    pub prefix: PathBuf,
    pub tokenizer: PathBuf,
    pub n: usize,
    pub discount: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HunspellConfig {
    pub english_dictionary: PathBuf,
    pub greek_dictionary: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InferenceConfig {
    pub context_size: usize,
    pub batch_size: usize,
    pub micro_batch_size: usize,
    pub flash_attention: bool,
}

#[derive(Debug)]
pub enum DaemonConfigError {
    Read(std::io::Error),
    Parse(toml::de::Error),
    InvalidRuntimeSocket,
    InvalidConversionCandidateCount,
    InvalidDefaultInputTable,
    InvalidPersonalizationAlpha,
    InvalidLmTypoConfiguration,
    UnsupportedInferenceProfile(InferenceConfig),
}

impl fmt::Display for DaemonConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "could not read daemon configuration: {error}"),
            Self::Parse(error) => write!(formatter, "invalid daemon configuration: {error}"),
            Self::InvalidRuntimeSocket => {
                write!(formatter, "runtime_socket must be beankey/daemon.sock")
            }
            Self::InvalidConversionCandidateCount => {
                write!(formatter, "conversion.n_best must be greater than zero")
            }
            Self::InvalidDefaultInputTable => {
                write!(formatter, "conversion.custom_input_table is not registered")
            }
            Self::InvalidPersonalizationAlpha => {
                write!(
                    formatter,
                    "zenz.personalization.alpha must be finite and nonnegative"
                )
            }
            Self::InvalidLmTypoConfiguration => {
                write!(formatter, "lm_typo configuration is invalid or incomplete")
            }
            Self::UnsupportedInferenceProfile(profile) => write!(
                formatter,
                "unsupported inference profile: context={}, batch={}, microbatch={}, flash_attention={}",
                profile.context_size,
                profile.batch_size,
                profile.micro_batch_size,
                profile.flash_attention
            ),
        }
    }
}

impl Error for DaemonConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::InvalidRuntimeSocket
            | Self::InvalidConversionCandidateCount
            | Self::InvalidDefaultInputTable
            | Self::InvalidPersonalizationAlpha
            | Self::InvalidLmTypoConfiguration
            | Self::UnsupportedInferenceProfile(_) => None,
        }
    }
}

impl DaemonConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DaemonConfigError> {
        let source = fs::read_to_string(path).map_err(DaemonConfigError::Read)?;
        Self::parse(&source)
    }

    pub fn parse(source: &str) -> Result<Self, DaemonConfigError> {
        let config: Self = toml::from_str(source).map_err(DaemonConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), DaemonConfigError> {
        if self.runtime_socket != Path::new("beankey/daemon.sock")
            || self.runtime_socket.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            })
        {
            return Err(DaemonConfigError::InvalidRuntimeSocket);
        }
        if self.inference.context_size != EXPECTED_CONTEXT_SIZE
            || self.inference.batch_size != EXPECTED_BATCH_SIZE
            || self.inference.micro_batch_size != EXPECTED_MICRO_BATCH_SIZE
            || !self.inference.flash_attention
        {
            return Err(DaemonConfigError::UnsupportedInferenceProfile(
                self.inference.clone(),
            ));
        }
        if self.conversion.n_best == 0 {
            return Err(DaemonConfigError::InvalidConversionCandidateCount);
        }
        if self.conversion.input_style == InputStyleConfig::Custom
            && self
                .conversion
                .custom_input_table
                .as_ref()
                .is_none_or(|name| !self.conversion.custom_input_tables.contains_key(name))
        {
            return Err(DaemonConfigError::InvalidDefaultInputTable);
        }
        if self
            .zenz
            .personalization
            .as_ref()
            .is_some_and(|config| !config.alpha.is_finite() || config.alpha < 0.0)
        {
            return Err(DaemonConfigError::InvalidPersonalizationAlpha);
        }
        let typo = &self.lm_typo;
        let invalid_typo = typo.enabled
            && (typo.beam_size == 0
                || typo.top_k == 0
                || typo.n_best == 0
                || [
                    typo.substitution_cost,
                    typo.deletion_cost,
                    typo.transposition_cost,
                ]
                .into_iter()
                .any(|cost| !cost.is_finite() || cost < 0.0)
                || match (typo.language_model, typo.ngram.as_ref()) {
                    (LmTypoLanguageModel::Zenz, _) => false,
                    (LmTypoLanguageModel::Ngram, Some(ngram)) => {
                        ngram.n < 2 || !ngram.discount.is_finite()
                    }
                    (LmTypoLanguageModel::Ngram, None) => true,
                });
        if invalid_typo {
            return Err(DaemonConfigError::InvalidLmTypoConfiguration);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
dictionary = "/nix/store/dictionary"
model = "/nix/store/model.gguf"
emoji_dictionary = "/nix/store/emoji_all_E17.0.txt"
llama_backend_directory = "/nix/store/llama/bin"
runtime_socket = "beankey/daemon.sock"

[hunspell]
english_dictionary = "/nix/store/en_US"
greek_dictionary = "/nix/store/el_GR"

[conversion]
input_style = "roman_to_kana"
keyboard_language = "japanese"
n_best = 10
japanese_prediction = "automatic"
foreign_prediction = "automatic"
full_width_roman = false
half_width_kana = false
typography = false
typo_correction = "automatic"
live_conversion = false
type_backslash = true
type_half_space = true
option_direct_full_width_input = true
punctuation_style = "period_and_comma"
custom_input_tables = {}

[learning]
mode = "input_and_output"
max_count = 65536

[zenz]
inference_limit = 10
rich_candidates = false
predictive_input = false
enable_alignment_separator = false

[lm_typo]
enabled = false
language_model = "zenz"
beam_size = 32
top_k = 64
n_best = 5
substitution_cost = 2.0
deletion_cost = 3.0
transposition_cost = 2.0

[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#;

    #[test]
    fn parses_the_internal_nixos_configuration() {
        let config = DaemonConfig::parse(CONFIG).unwrap();
        assert_eq!(config.runtime_socket, Path::new("beankey/daemon.sock"));
        assert_eq!(config.inference.context_size, 512);
        assert_eq!(config.zenz.inference_limit, 10);
        assert_eq!(
            config.conversion.keyboard_language,
            KeyboardLanguageConfig::Japanese
        );
        assert_eq!(config.learning, LearningConfig::default());
        assert!(config.conversion.type_backslash);
        assert!(config.conversion.type_half_space);
        assert!(config.conversion.option_direct_full_width_input);
        assert_eq!(
            config.conversion.punctuation_style,
            PunctuationStyleConfig::PeriodAndComma
        );
    }

    #[test]
    fn rejects_path_traversal_and_noncanonical_inference_profiles() {
        let traversal = CONFIG.replace(
            "runtime_socket = \"beankey/daemon.sock\"",
            "runtime_socket = \"../daemon.sock\"",
        );
        assert!(matches!(
            DaemonConfig::parse(&traversal),
            Err(DaemonConfigError::InvalidRuntimeSocket)
        ));

        let profile = CONFIG.replace("context_size = 512", "context_size = 1024");
        assert!(matches!(
            DaemonConfig::parse(&profile),
            Err(DaemonConfigError::UnsupportedInferenceProfile(_))
        ));
    }

    #[test]
    fn rejects_unknown_internal_settings() {
        let unknown = format!("{CONFIG}\nunexpected = true\n");
        assert!(matches!(
            DaemonConfig::parse(&unknown),
            Err(DaemonConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_an_empty_candidate_search() {
        let config = CONFIG.replace("n_best = 10", "n_best = 0");
        assert!(matches!(
            DaemonConfig::parse(&config),
            Err(DaemonConfigError::InvalidConversionCandidateCount)
        ));
    }

    #[test]
    fn rejects_an_invalid_personalization_strength() {
        let config = format!(
            "{CONFIG}\n[zenz.personalization]\nbase_ngram = \"/base\"\npersonal_ngram = \"/personal\"\nalpha = -1.0\n"
        );
        assert!(matches!(
            DaemonConfig::parse(&config),
            Err(DaemonConfigError::InvalidPersonalizationAlpha)
        ));
    }

    #[test]
    fn requires_assets_for_enabled_ngram_typo_correction() {
        let config = CONFIG
            .replace("enabled = false", "enabled = true")
            .replace("language_model = \"zenz\"", "language_model = \"ngram\"");
        assert!(matches!(
            DaemonConfig::parse(&config),
            Err(DaemonConfigError::InvalidLmTypoConfiguration)
        ));
    }

    #[test]
    fn requires_a_registered_default_custom_input_table() {
        let config = CONFIG.replace(
            "input_style = \"roman_to_kana\"",
            "input_style = \"custom\"\ncustom_input_table = \"missing\"",
        );
        assert!(matches!(
            DaemonConfig::parse(&config),
            Err(DaemonConfigError::InvalidDefaultInputTable)
        ));
    }
}
