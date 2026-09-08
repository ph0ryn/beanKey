use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::foreign::{ForeignCompletionProvider, foreign_predictions};
use crate::kana::{to_hiragana, to_katakana};
use crate::lattice::{to_full_width, to_half_width};
use crate::{
    Candidate, ComposingCount, ComposingText, ConversionContext, DictionaryEntry, DictionaryError,
    DictionaryMetadata, InputElement, InputModifier, InputPiece, InputStyle, InputTableRegistry,
    LearningError, LearningMemory, NormalConverter, PostCompositionPrediction,
    PostCompositionPredictor, PrefixConstraint, UserDictionary, ZenzInferenceError,
    ZenzInputGenerationRequest, ZenzLanguageModel, ZenzVersionConfig, generate_next_input,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PredictionMode {
    Automatic,
    Manual,
    #[default]
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextTransform {
    Hiragana,
    Katakana,
    HalfWidthKatakana,
    FullWidthRoman,
    HalfWidthRoman,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TypoCorrectionMode {
    Enabled,
    #[default]
    Automatic,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestOptions {
    pub n_best: usize,
    pub japanese_prediction: PredictionMode,
    pub full_width_roman: bool,
    pub half_width_kana: bool,
    pub version_string: Option<String>,
    pub typo_correction: TypoCorrectionMode,
    pub foreign_prediction: PredictionMode,
    pub keyboard_language: crate::KeyboardLanguage,
    pub typography: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            n_best: 10,
            japanese_prediction: PredictionMode::Disabled,
            full_width_roman: true,
            half_width_kana: false,
            version_string: None,
            typo_correction: TypoCorrectionMode::Automatic,
            foreign_prediction: PredictionMode::Disabled,
            keyboard_language: crate::KeyboardLanguage::None,
            typography: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConversionResult {
    pub main_results: Vec<Candidate>,
    pub prediction_results: Vec<Candidate>,
    pub english_prediction_results: Vec<Candidate>,
    pub first_clause_results: Vec<Candidate>,
}

impl ConversionResult {
    fn merged_desktop_candidates(&self) -> Vec<Candidate> {
        let mut seen = std::collections::HashSet::new();
        self.first_clause_results
            .iter()
            .chain(&self.main_results)
            .filter(|candidate| seen.insert(candidate.text.clone()))
            .cloned()
            .collect()
    }
}

#[derive(Debug)]
pub enum SelectionError {
    CandidateOutOfRange {
        index: usize,
        candidate_count: usize,
    },
    Learning(LearningError),
}

impl PartialEq for SelectionError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::CandidateOutOfRange {
                    index: left_index,
                    candidate_count: left_count,
                },
                Self::CandidateOutOfRange {
                    index: right_index,
                    candidate_count: right_count,
                },
            ) => left_index == right_index && left_count == right_count,
            (Self::Learning(left), Self::Learning(right)) => left.to_string() == right.to_string(),
            _ => false,
        }
    }
}

impl Eq for SelectionError {}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateOutOfRange {
                index,
                candidate_count,
            } => write!(
                formatter,
                "candidate index {index} is outside the current {candidate_count} candidates"
            ),
            Self::Learning(error) => error.fmt(formatter),
        }
    }
}

impl Error for SelectionError {}

impl From<LearningError> for SelectionError {
    fn from(value: LearningError) -> Self {
        Self::Learning(value)
    }
}

#[derive(Clone, Default)]
pub struct ConversionSession {
    composing: ComposingText,
    full_composing: Option<ComposingText>,
    candidates: Vec<Candidate>,
    context: ConversionContext,
    last_committed: Option<Candidate>,
    user_dictionary: Vec<DictionaryEntry>,
    user_shortcuts: Vec<DictionaryEntry>,
    file_user_dictionary: UserDictionary,
    learning_memory: Option<LearningMemory>,
    stable_prediction_cache: Option<StablePredictionCache>,
    zenz_prediction_cache: Option<ZenzPredictionCache>,
    foreign_completion_provider: Option<Arc<dyn ForeignCompletionProvider>>,
}

#[derive(Clone)]
struct StablePredictionCache {
    original_surface: String,
    suffix_count: usize,
    candidates: Vec<Candidate>,
}

struct PredictiveInputSource {
    base_surface: String,
    possible_nexts: Vec<String>,
    dropped_suffix_count: usize,
}

#[derive(Clone)]
struct ZenzPredictionCache {
    left_context: String,
    version: ZenzVersionConfig,
    input_style: InputStyle,
    original_surface: String,
    suffix_count: usize,
    predicted_text: String,
}

impl ConversionSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn composing(&self) -> &ComposingText {
        &self.composing
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    pub fn zenz_model_composing(&self) -> &ComposingText {
        self.full_composing.as_ref().unwrap_or(&self.composing)
    }

    pub fn pending_zenz_suffix_count(&self, tables: &InputTableRegistry) -> usize {
        predictive_input_source(self.zenz_model_composing(), tables).dropped_suffix_count
    }

    pub fn begin_segment_request(&mut self, tables: &InputTableRegistry) {
        if self.full_composing.is_some() || self.composing.is_at_end() {
            return;
        }
        let target = self.composing.prefix_to_cursor(tables);
        self.full_composing = Some(std::mem::replace(&mut self.composing, target));
    }

    pub fn end_segment_request(&mut self) {
        if let Some(full) = self.full_composing.take() {
            self.composing = full;
        }
    }

    pub fn import_dynamic_user_dictionary(
        &mut self,
        mut entries: Vec<DictionaryEntry>,
        mut shortcuts: Vec<DictionaryEntry>,
    ) {
        for entry in entries.iter_mut().chain(shortcuts.iter_mut()) {
            entry.metadata.insert(DictionaryMetadata::USER_DICTIONARY);
        }
        self.user_dictionary = entries;
        self.user_shortcuts = shortcuts;
        self.candidates.clear();
    }

    pub fn update_user_dictionary_path(
        &mut self,
        path: impl Into<std::path::PathBuf>,
    ) -> Result<(), DictionaryError> {
        self.file_user_dictionary = UserDictionary::open(path)?;
        self.candidates.clear();
        Ok(())
    }

    pub fn set_learning_memory(&mut self, memory: LearningMemory) -> Result<(), LearningError> {
        memory.validate()?;
        self.learning_memory = Some(memory);
        self.stable_prediction_cache = None;
        self.candidates.clear();
        Ok(())
    }

    pub fn set_foreign_completion_provider(
        &mut self,
        provider: Arc<dyn ForeignCompletionProvider>,
    ) {
        self.foreign_completion_provider = Some(provider);
        self.candidates.clear();
    }

    pub fn refresh_learning(&mut self) -> Result<(), LearningError> {
        if let Some(memory) = &self.learning_memory {
            memory.validate()?;
        }
        self.candidates.clear();
        Ok(())
    }

    pub fn commit_learning(&mut self) -> Result<bool, LearningError> {
        let committed = match &self.learning_memory {
            Some(memory) => memory.commit()?,
            None => false,
        };
        if committed {
            self.stable_prediction_cache = None;
        }
        self.refresh_learning()?;
        Ok(committed)
    }

    pub fn forget_learning(&mut self, candidate: &Candidate) -> Result<(), LearningError> {
        self.stable_prediction_cache = None;
        if let Some(memory) = &self.learning_memory {
            memory.forget(candidate)?;
        }
        self.refresh_learning()
    }

    pub fn reset_learning(&mut self) -> Result<(), LearningError> {
        self.stable_prediction_cache = None;
        if let Some(memory) = &self.learning_memory {
            memory.reset()?;
        }
        self.refresh_learning()
    }

    fn additional_dictionary(&self) -> Vec<DictionaryEntry> {
        self.user_dictionary
            .iter()
            .chain(self.file_user_dictionary.entries())
            .cloned()
            .collect()
    }

    pub fn insert_str(
        &mut self,
        value: &str,
        input_style: InputStyle,
        tables: &InputTableRegistry,
    ) {
        self.composing.insert_str(value, input_style, tables);
        self.candidates.clear();
        self.last_committed = None;
    }

    pub fn insert_key(
        &mut self,
        intention: Option<String>,
        input: impl Into<String>,
        modifiers: impl IntoIterator<Item = InputModifier>,
        input_style: InputStyle,
        tables: &InputTableRegistry,
    ) {
        self.composing.insert(
            vec![InputElement::new(
                InputPiece::Key {
                    intention,
                    input: input.into(),
                    modifiers: BTreeSet::from_iter(modifiers),
                },
                input_style,
            )],
            tables,
        );
        self.candidates.clear();
        self.last_committed = None;
    }

    pub fn insert_composition_separator(
        &mut self,
        input_style: InputStyle,
        tables: &InputTableRegistry,
    ) {
        if self
            .composing
            .input()
            .last()
            .is_some_and(|element| element.piece == InputPiece::CompositionSeparator)
        {
            return;
        }
        self.composing.insert(
            vec![InputElement::new(
                InputPiece::CompositionSeparator,
                input_style,
            )],
            tables,
        );
        self.candidates.clear();
        self.last_committed = None;
    }

    pub fn delete_backward(&mut self, count: usize, tables: &InputTableRegistry) {
        self.composing.delete_backward(count, tables);
        self.candidates.clear();
        self.last_committed = None;
        if self.composing.is_empty() {
            self.context = ConversionContext::default();
        }
    }

    pub fn delete_forward(&mut self, count: usize, tables: &InputTableRegistry) {
        self.composing.delete_forward(count, tables);
        self.candidates.clear();
        self.last_committed = None;
        if self.composing.is_empty() {
            self.context = ConversionContext::default();
        }
    }

    pub fn move_cursor(&mut self, count: isize) -> isize {
        let moved = self.composing.move_cursor(count);
        self.candidates.clear();
        self.last_committed = None;
        moved
    }

    pub fn transformed_text(&self, transform: TextTransform) -> String {
        let surface = self.composing.surface();
        let raw_input = raw_input(self.composing.input());
        match transform {
            TextTransform::Hiragana => to_hiragana(&surface),
            TextTransform::Katakana => to_katakana(&surface),
            TextTransform::HalfWidthKatakana => to_half_width(&to_katakana(&surface)),
            TextTransform::FullWidthRoman => to_full_width(&raw_input),
            TextTransform::HalfWidthRoman => raw_input,
        }
    }

    pub fn desktop_additional_candidates(
        &self,
        selected: Option<&Candidate>,
        tables: &InputTableRegistry,
    ) -> Vec<(Candidate, &'static str)> {
        let surface = selected.map_or_else(
            || self.composing.surface(),
            |candidate| {
                candidate
                    .entries
                    .iter()
                    .map(|entry| entry.ruby.as_str())
                    .collect()
            },
        );
        let composing_count = selected.map_or_else(
            || ComposingCount::Input(self.composing.input().len()),
            |candidate| candidate.composing_count.clone(),
        );
        let raw_input = selected.map_or_else(
            || raw_input(self.composing.input()),
            |candidate| {
                let mut remaining = self.composing.clone();
                remaining.complete_prefix(candidate.composing_count.clone(), tables);
                let consumed = self
                    .composing
                    .input()
                    .len()
                    .saturating_sub(remaining.input().len());
                raw_input(&self.composing.input()[..consumed])
            },
        );
        let katakana = to_katakana(&surface);
        [
            (raw_input.clone(), "英数"),
            (to_full_width(&raw_input), "全角英数"),
            (to_half_width(&katakana), "半角カナ"),
            (katakana, "カタカナ"),
            (to_hiragana(&surface), "ひらがな"),
        ]
        .into_iter()
        .map(|(text, annotation)| {
            let entry = DictionaryEntry {
                word: text.clone(),
                ruby: to_katakana(&surface),
                left_id: 1288,
                right_id: 1288,
                meaning_id: 501,
                base_value: 0.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            };
            (
                Candidate::single(text, 0.0, composing_count.clone(), 501, vec![entry])
                    .with_learning_target(false),
                annotation,
            )
        })
        .collect()
    }

    pub fn request_candidates(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        n_best: usize,
    ) -> Result<&[Candidate], DictionaryError> {
        let additional = self.additional_dictionary();
        let converter = converter.with_learning_memory(self.learning_memory.as_ref());
        self.candidates = converter.convert_with_entries(
            &self.composing,
            tables,
            n_best,
            self.context,
            &additional,
        )?;
        let mut seen: std::collections::HashSet<_> = self
            .candidates
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect();
        let mut first_clauses: Vec<_> = self
            .candidates
            .iter()
            .filter_map(Candidate::first_clause_candidate)
            .filter(|candidate| seen.insert(candidate.text.clone()))
            .collect();
        first_clauses.sort_by(|left, right| {
            right
                .ruby_count
                .cmp(&left.ruby_count)
                .then_with(|| right.value.total_cmp(&left.value))
        });
        first_clauses.truncate(5);
        self.candidates.extend(first_clauses);
        Ok(&self.candidates)
    }

    pub fn request_zenz_draft(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        n_best: usize,
        constraint: &PrefixConstraint,
    ) -> Result<&[Candidate], DictionaryError> {
        let additional = self.additional_dictionary();
        let converter = converter.with_learning_memory(self.learning_memory.as_ref());
        self.candidates = if constraint.is_empty() {
            converter.convert_with_entries(
                &self.composing,
                tables,
                n_best,
                self.context,
                &additional,
            )?
        } else {
            converter.convert_with_entries_and_prefix_constraint(
                &self.composing,
                tables,
                n_best,
                self.context,
                &additional,
                constraint,
            )?
        };
        Ok(&self.candidates)
    }

    pub fn set_zenz_candidates(&mut self, candidates: Vec<Candidate>) {
        self.candidates = candidates;
    }

    pub fn finalize_zenz_request(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        options: RequestOptions,
    ) -> Result<ConversionResult, DictionaryError> {
        self.finalize_zenz_request_with_prediction_override(converter, tables, options, None)
    }

    pub fn finalize_zenz_request_with_prediction_override(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        options: RequestOptions,
        prediction_override: Option<Vec<Candidate>>,
    ) -> Result<ConversionResult, DictionaryError> {
        let mut full = std::mem::take(&mut self.candidates);
        let mut leading_values = full
            .iter()
            .take(5)
            .map(|candidate| candidate.value)
            .collect::<Vec<_>>();
        leading_values.sort_by(|left, right| right.total_cmp(left));
        for (candidate, value) in full.iter_mut().take(5).zip(leading_values) {
            candidate.value = value;
        }
        self.assemble_request(converter, tables, options, full, prediction_override)
    }

    pub fn request_predictions(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        n_best: usize,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let additional = self.additional_dictionary();
        self.predictions(converter, tables, n_best, &additional)
    }

    pub fn predict_next_input_text(
        &mut self,
        tables: &InputTableRegistry,
        model: &mut dyn ZenzLanguageModel,
        version: &ZenzVersionConfig,
        left_context: &str,
        count: usize,
        max_entropy: Option<f32>,
    ) -> Result<(String, usize), ZenzInferenceError> {
        let input_style = self
            .composing
            .input()
            .last()
            .map(|element| element.input_style.clone())
            .unwrap_or(InputStyle::Direct);
        if let Some(cache) = &self.zenz_prediction_cache
            && cache.left_context == left_context
            && cache.version == *version
            && cache.input_style == input_style
            && let Some(prediction) = cache.remaining_prediction(&self.composing, count)
        {
            return Ok((prediction, 0));
        }

        let source = predictive_input_source(&self.composing, tables);
        let predicted_text = generate_next_input(
            model,
            ZenzInputGenerationRequest {
                left_context,
                composing_text: &source.base_surface,
                count,
                min_length: 1,
                max_entropy,
                version,
                possible_nexts: &source.possible_nexts,
            },
        )?;
        self.zenz_prediction_cache = (!predicted_text.is_empty()).then(|| ZenzPredictionCache {
            left_context: left_context.to_owned(),
            version: version.clone(),
            input_style,
            original_surface: self.composing.surface(),
            suffix_count: source.dropped_suffix_count,
            predicted_text: predicted_text.clone(),
        });
        Ok((predicted_text, source.dropped_suffix_count))
    }

    pub fn request_zenz_prediction<F>(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        model: &mut dyn ZenzLanguageModel,
        version: &ZenzVersionConfig,
        left_context: &str,
        mut evaluate_prediction: F,
    ) -> Result<Vec<Candidate>, ZenzPredictionError>
    where
        F: FnMut(
            &mut ConversionSession,
            &mut dyn ZenzLanguageModel,
        ) -> Result<(), ZenzPredictionError>,
    {
        let additional = self.additional_dictionary();
        let predictions = self.predictions(converter, tables, 5, &additional)?;
        if !predictions.is_empty() {
            return Ok(predictions);
        }
        let (predicted_text, suffix_count) =
            self.predict_next_input_text(tables, model, version, left_context, 10, Some(3.0))?;
        if predicted_text.is_empty() {
            return Ok(Vec::new());
        }
        let input_style = self
            .composing
            .input()
            .last()
            .map(|element| element.input_style.clone())
            .unwrap_or(InputStyle::Direct);
        let insert_text = if input_style == InputStyle::RomanToKana {
            to_hiragana(&predicted_text)
        } else {
            predicted_text
        };
        let mut predicted = self.composing.clone();
        if suffix_count > 0 {
            predicted.delete_backward(suffix_count, tables);
        }
        predicted.insert_str(&insert_text, input_style, tables);
        let consumed = self.composing.surface_graphemes().len();
        let mut prediction_session = self.clone();
        prediction_session.composing = predicted;
        prediction_session.full_composing = None;
        prediction_session.candidates.clear();
        evaluate_prediction(&mut prediction_session, model)?;
        Ok(prediction_session
            .candidates
            .into_iter()
            .map(|mut candidate| {
                candidate.composing_count = ComposingCount::Surface(consumed);
                candidate
            })
            .collect())
    }

    pub fn request(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        options: RequestOptions,
    ) -> Result<ConversionResult, DictionaryError> {
        let additional = self.additional_dictionary();
        let converter = converter.with_learning_memory(self.learning_memory.as_ref());
        let full = converter.convert_with_entries_and_typo(
            &self.composing,
            tables,
            options.n_best,
            self.context,
            &additional,
            options.typo_correction == TypoCorrectionMode::Enabled,
        )?;
        self.assemble_request(&converter, tables, options, full, None)
    }

    fn assemble_request(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        options: RequestOptions,
        full: Vec<Candidate>,
        prediction_override: Option<Vec<Candidate>>,
    ) -> Result<ConversionResult, DictionaryError> {
        let additional = self.additional_dictionary();
        let mut first_clauses = unique(
            full.iter()
                .filter_map(Candidate::first_clause_candidate)
                .collect(),
        );
        first_clauses.sort_by(|left, right| {
            right
                .ruby_count
                .cmp(&left.ruby_count)
                .then_with(|| right.value.total_cmp(&left.value))
        });
        first_clauses.truncate(5);

        let predictions = if options.japanese_prediction == PredictionMode::Disabled {
            self.stable_prediction_cache = None;
            Vec::new()
        } else if let Some(predictions) = prediction_override {
            predictions
        } else {
            self.predictions(converter, tables, 3, &additional)?
        };
        let foreign_predictions = if options.foreign_prediction == PredictionMode::Disabled {
            Vec::new()
        } else {
            self.foreign_completion_provider
                .as_ref()
                .map_or_else(Vec::new, |provider| {
                    foreign_predictions(
                        &self.composing,
                        options.keyboard_language,
                        provider.as_ref(),
                        -5.0,
                    )
                })
        };
        let mut leading: Vec<_> = full.iter().take(5).cloned().collect();
        let katakana = to_katakana(&self.composing.surface());
        leading.extend(
            self.user_shortcuts
                .iter()
                .chain(self.file_user_dictionary.shortcuts())
                .filter(|entry| entry.ruby == katakana)
                .cloned()
                .map(|entry| {
                    Candidate::single(
                        entry.word.clone(),
                        entry.value(),
                        crate::ComposingCount::Surface(
                            unicode_segmentation::UnicodeSegmentation::graphemes(
                                katakana.as_str(),
                                true,
                            )
                            .count(),
                        ),
                        entry.meaning_id,
                        vec![entry],
                    )
                    .with_learning_target(false)
                }),
        );
        leading = unique(leading);
        if options.japanese_prediction == PredictionMode::Automatic {
            leading.extend(predictions.iter().cloned());
            leading = unique(leading);
        }
        if options.foreign_prediction == PredictionMode::Automatic {
            leading.extend(foreign_predictions.iter().cloned());
            leading = unique(leading);
        }
        leading.sort_by(|left, right| right.value.total_cmp(&left.value));
        leading.truncate(5);
        if !leading.iter().take(3).any(|candidate| {
            candidate
                .entries
                .iter()
                .map(|entry| entry.ruby.as_str())
                .collect::<String>()
                == katakana
        }) && let Some(candidate) = full
            .iter()
            .find(|candidate| {
                candidate
                    .entries
                    .iter()
                    .map(|entry| entry.ruby.as_str())
                    .collect::<String>()
                    == katakana
            })
            .cloned()
        {
            leading.retain(|item| item.text != candidate.text);
            leading.insert(2.min(leading.len()), candidate);
        }

        let mut seen: std::collections::HashSet<_> = leading
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect();
        let first_for_main: Vec<_> = first_clauses
            .iter()
            .filter(|candidate| seen.insert(candidate.text.clone()))
            .cloned()
            .collect();
        let mut words = converter.word_candidates(&self.composing)?;
        words.extend(converter.representation_candidates(
            &self.composing,
            options.full_width_roman,
            options.half_width_kana,
        ));
        words = unique(words);
        words.sort_by(|left, right| {
            right
                .ruby_count
                .cmp(&left.ruby_count)
                .then_with(|| right.value.total_cmp(&left.value))
        });
        words.retain(|candidate| seen.insert(candidate.text.clone()));
        let mut special =
            crate::special_candidates(&self.composing, options.version_string.as_deref());
        if options.typography {
            special.extend(crate::typographical_candidates(&self.composing));
        }
        special.retain(|candidate| seen.insert(candidate.text.clone()));
        words.splice(5.min(words.len())..5.min(words.len()), special);

        let mut main_results = leading;
        main_results.extend(first_for_main);
        main_results.extend(words);
        let main_results = finalize_candidates(main_results);
        let predictions = finalize_candidates(predictions);
        let first_clauses = finalize_candidates(first_clauses);
        self.candidates = main_results.clone();
        Ok(ConversionResult {
            main_results,
            prediction_results: predictions,
            english_prediction_results: foreign_predictions,
            first_clause_results: first_clauses,
        })
    }

    pub fn select_candidate(
        &mut self,
        index: usize,
        tables: &InputTableRegistry,
    ) -> Result<String, SelectionError> {
        let candidate =
            self.candidates
                .get(index)
                .cloned()
                .ok_or(SelectionError::CandidateOutOfRange {
                    index,
                    candidate_count: self.candidates.len(),
                })?;
        self.select_candidate_value(candidate, tables)
    }

    pub fn select_candidate_value(
        &mut self,
        candidate: Candidate,
        tables: &InputTableRegistry,
    ) -> Result<String, SelectionError> {
        self.composing
            .complete_prefix(candidate.composing_count.clone(), tables);
        if let Some(last) = candidate.entries.last() {
            self.context = ConversionContext {
                right_id: last.right_id,
                meaning_id: candidate.last_meaning_id,
            };
        }
        self.candidates.clear();
        if candidate.is_learning_target
            && let Some(memory) = self.learning_memory.clone()
        {
            memory.learn(&candidate)?;
        }
        self.last_committed = Some(candidate.clone());
        if self.composing.is_empty() {
            self.composing.stop();
            self.context = ConversionContext::default();
            self.stable_prediction_cache = None;
            self.zenz_prediction_cache = None;
        }
        Ok(candidate.text)
    }

    pub fn remaining_after_candidate(
        &self,
        candidate: &Candidate,
        tables: &InputTableRegistry,
    ) -> String {
        let mut composing = self.composing.clone();
        composing.complete_prefix(candidate.composing_count.clone(), tables);
        composing.surface()
    }

    pub fn desktop_candidates(
        &self,
        result: &ConversionResult,
        tables: &InputTableRegistry,
    ) -> Vec<Candidate> {
        if result
            .first_clause_results
            .iter()
            .any(|candidate| self.remaining_after_candidate(candidate, tables).is_empty())
        {
            result.main_results.clone()
        } else {
            result.merged_desktop_candidates()
        }
    }

    pub fn consumed_surface_count(
        &self,
        candidate: &Candidate,
        tables: &InputTableRegistry,
    ) -> usize {
        let mut composing = self.composing.clone();
        composing.complete_prefix(candidate.composing_count.clone(), tables);
        self.composing
            .surface_graphemes()
            .len()
            .saturating_sub(composing.surface_graphemes().len())
    }

    pub fn request_post_composition_predictions(
        &self,
        predictor: &PostCompositionPredictor<'_>,
    ) -> Result<Vec<PostCompositionPrediction>, DictionaryError> {
        match &self.last_committed {
            Some(candidate) => predictor.predict(candidate),
            None => Ok(Vec::new()),
        }
    }

    pub fn reset(&mut self) {
        self.composing.stop();
        self.candidates.clear();
        self.context = ConversionContext::default();
        self.last_committed = None;
        self.stable_prediction_cache = None;
        self.zenz_prediction_cache = None;
    }

    fn predictions(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        n_best: usize,
        additional: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        if n_best == 0 {
            self.stable_prediction_cache = None;
            return Ok(Vec::new());
        }
        let source = predictive_input_source(&self.composing, tables);
        let stable = self
            .stable_prediction_cache
            .as_ref()
            .map(|cache| cache.compatible_candidates(&self.composing, &source))
            .unwrap_or_default();
        let converter = converter.with_learning_memory(self.learning_memory.as_ref());
        let fresh = converter.predict_with_entries(
            &self.composing,
            tables,
            n_best,
            self.context,
            additional,
        )?;
        let mut predictions = unique(stable);
        let mut seen = predictions
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect::<std::collections::HashSet<_>>();
        predictions.extend(
            fresh
                .into_iter()
                .filter(|candidate| seen.insert(candidate.text.clone())),
        );
        predictions.truncate(n_best);
        if predictions.is_empty() {
            self.stable_prediction_cache = None;
        } else {
            self.stable_prediction_cache = Some(StablePredictionCache {
                original_surface: self.composing.surface(),
                suffix_count: source.dropped_suffix_count,
                candidates: predictions.clone(),
            });
        }
        Ok(predictions)
    }
}

fn raw_input(input: &[InputElement]) -> String {
    input
        .iter()
        .filter_map(|element| match &element.piece {
            InputPiece::Character(value) => Some(value.as_str()),
            InputPiece::Key { input, .. } => Some(input.as_str()),
            InputPiece::CompositionSeparator => None,
        })
        .collect()
}

impl StablePredictionCache {
    fn compatible_candidates(
        &self,
        composing: &ComposingText,
        source: &PredictiveInputSource,
    ) -> Vec<Candidate> {
        let original_count =
            UnicodeSegmentation::graphemes(self.original_surface.as_str(), true).count();
        let retained_count = original_count.saturating_sub(self.suffix_count);
        let cached_base = UnicodeSegmentation::graphemes(self.original_surface.as_str(), true)
            .take(retained_count)
            .collect::<String>();
        if !source.base_surface.starts_with(&cached_base) {
            return Vec::new();
        }
        let current_surface = composing.surface();
        let current_ruby = to_katakana(&current_surface);
        let compatible_prefixes = if source.possible_nexts.is_empty() {
            vec![current_ruby.clone()]
        } else {
            source
                .possible_nexts
                .iter()
                .map(|suffix| format!("{}{suffix}", to_katakana(&source.base_surface)))
                .collect()
        };
        let consumed = composing.surface_graphemes().len();
        self.candidates
            .iter()
            .filter(|candidate| {
                let ruby = candidate
                    .entries
                    .iter()
                    .map(|entry| entry.ruby.as_str())
                    .collect::<String>();
                !candidate.text.is_empty()
                    && ruby != current_ruby
                    && compatible_prefixes
                        .iter()
                        .any(|prefix| ruby.starts_with(prefix))
            })
            .cloned()
            .map(|mut candidate| {
                candidate.composing_count = ComposingCount::Surface(consumed);
                candidate
            })
            .collect()
    }
}

impl ZenzPredictionCache {
    fn remaining_prediction(&self, composing: &ComposingText, count: usize) -> Option<String> {
        if count == 0 {
            return None;
        }
        let original = UnicodeSegmentation::graphemes(self.original_surface.as_str(), true)
            .collect::<Vec<_>>();
        let base = original[..original.len().saturating_sub(self.suffix_count)].concat();
        let current = composing.surface();
        let current = current.strip_prefix(&base)?;
        let predicted_insert = if self.input_style == InputStyle::RomanToKana {
            to_hiragana(&self.predicted_text)
        } else {
            self.predicted_text.clone()
        };
        predicted_insert.strip_prefix(current)?;
        let consumed = UnicodeSegmentation::graphemes(current, true).count();
        let remaining = UnicodeSegmentation::graphemes(self.predicted_text.as_str(), true)
            .skip(consumed)
            .take(count)
            .collect::<String>();
        if remaining.is_empty() {
            return None;
        }
        Some(remaining)
    }
}

#[derive(Debug)]
pub enum ZenzPredictionError {
    Dictionary(DictionaryError),
    Inference(ZenzInferenceError),
}

impl fmt::Display for ZenzPredictionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dictionary(error) => error.fmt(formatter),
            Self::Inference(error) => error.fmt(formatter),
        }
    }
}

impl Error for ZenzPredictionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dictionary(error) => Some(error),
            Self::Inference(error) => Some(error),
        }
    }
}

impl From<DictionaryError> for ZenzPredictionError {
    fn from(value: DictionaryError) -> Self {
        Self::Dictionary(value)
    }
}

impl From<ZenzInferenceError> for ZenzPredictionError {
    fn from(value: ZenzInferenceError) -> Self {
        Self::Inference(value)
    }
}

fn predictive_input_source(
    composing: &ComposingText,
    tables: &InputTableRegistry,
) -> PredictiveInputSource {
    let surface = composing.surface();
    let Some(style) = composing.input().last().map(|element| &element.input_style) else {
        return PredictiveInputSource {
            base_surface: surface,
            possible_nexts: Vec::new(),
            dropped_suffix_count: 0,
        };
    };
    if matches!(style, InputStyle::Direct) {
        return PredictiveInputSource {
            base_surface: surface,
            possible_nexts: Vec::new(),
            dropped_suffix_count: 0,
        };
    }
    let suffix_count = surface
        .chars()
        .rev()
        .take_while(|character| character.is_ascii_alphabetic())
        .count();
    if suffix_count == 0 {
        return PredictiveInputSource {
            base_surface: surface,
            possible_nexts: Vec::new(),
            dropped_suffix_count: 0,
        };
    }
    let suffix_start = surface.len() - suffix_count;
    let suffix = &surface[suffix_start..];
    let possible_nexts = tables
        .resolve(style)
        .map(|table| table.possible_nexts(suffix))
        .unwrap_or_default();
    if possible_nexts.is_empty() {
        PredictiveInputSource {
            base_surface: surface,
            possible_nexts,
            dropped_suffix_count: 0,
        }
    } else {
        PredictiveInputSource {
            base_surface: surface[..suffix_start].to_owned(),
            possible_nexts,
            dropped_suffix_count: suffix_count,
        }
    }
}

fn finalize_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates
        .into_iter()
        .map(Candidate::expand_templates)
        .collect()
}

fn unique(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut output: Vec<Candidate> = Vec::new();
    let mut indices = std::collections::HashMap::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| !candidate.text.is_empty())
    {
        if let Some(&index) = indices.get(&candidate.text) {
            let current: &mut Candidate = &mut output[index];
            if current.value < candidate.value || current.ruby_count < candidate.ruby_count {
                *current = candidate;
            }
        } else {
            indices.insert(candidate.text.clone(), output.len());
            output.push(candidate);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_candidate(text: &str, composing_count: ComposingCount) -> Candidate {
        Candidate::single(text.into(), 0.0, composing_count, 501, Vec::new())
    }

    fn prediction_candidate(text: &str, composing_count: ComposingCount) -> Candidate {
        Candidate::single(
            text.into(),
            0.0,
            composing_count,
            501,
            vec![DictionaryEntry {
                word: text.into(),
                ruby: to_katakana(text),
                left_id: 1288,
                right_id: 1288,
                meaning_id: 501,
                base_value: 0.0,
                adjustment: 0.0,
                metadata: DictionaryMetadata::default(),
            }],
        )
    }

    #[test]
    fn editing_invalidates_candidates_and_reset_clears_composition() {
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("kana", InputStyle::RomanToKana, &tables);
        assert_eq!(session.composing().surface(), "かな");
        session.move_cursor(-1);
        session.delete_forward(1, &tables);
        assert_eq!(session.composing().surface(), "か");
        assert!(session.candidates().is_empty());

        session.reset();
        assert!(session.composing().is_empty());
    }

    #[test]
    fn rejects_a_stale_candidate_index_without_changing_input() {
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("かな", InputStyle::Direct, &tables);
        let error = session.select_candidate(0, &tables).unwrap_err();
        assert_eq!(
            error,
            SelectionError::CandidateOutOfRange {
                index: 0,
                candidate_count: 0
            }
        );
        assert_eq!(session.composing().surface(), "かな");
    }

    #[test]
    fn limits_desktop_representations_to_the_selected_prefix() {
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("kanakana", InputStyle::RomanToKana, &tables);
        let entry = DictionaryEntry {
            word: "仮名".into(),
            ruby: "カナ".into(),
            left_id: 1288,
            right_id: 1288,
            meaning_id: 501,
            base_value: 0.0,
            adjustment: 0.0,
            metadata: DictionaryMetadata::default(),
        };
        let selected = Candidate::single(
            "仮名".into(),
            0.0,
            ComposingCount::Input(4),
            501,
            vec![entry],
        );

        let representations = session.desktop_additional_candidates(Some(&selected), &tables);
        assert_eq!(representations[0].0.text, "kana");
        assert_eq!(representations[1].0.text, "ｋａｎａ");
        assert_eq!(representations[4].0.text, "かな");
    }

    #[test]
    fn uses_only_main_results_when_a_first_clause_candidate_consumes_all_input() {
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("かな", InputStyle::Direct, &tables);
        let result = ConversionResult {
            main_results: vec![test_candidate("仮名", ComposingCount::Surface(2))],
            first_clause_results: vec![test_candidate("かな", ComposingCount::Surface(2))],
            ..ConversionResult::default()
        };

        assert_eq!(
            session.desktop_candidates(&result, &tables),
            result.main_results
        );
    }

    #[test]
    fn prepends_partial_first_clause_results_to_main_results() {
        let tables = InputTableRegistry::new();
        let mut session = ConversionSession::new();
        session.insert_str("かな", InputStyle::Direct, &tables);
        let first_clause = test_candidate("蚊", ComposingCount::Surface(1));
        let main = test_candidate("仮名", ComposingCount::Surface(2));
        let result = ConversionResult {
            main_results: vec![main.clone()],
            first_clause_results: vec![first_clause.clone()],
            ..ConversionResult::default()
        };

        assert_eq!(
            session.desktop_candidates(&result, &tables),
            vec![first_clause, main]
        );
    }

    #[test]
    fn zenz_prediction_cache_returns_only_the_unconsumed_direct_suffix() {
        let cache = ZenzPredictionCache {
            left_context: "左文脈".into(),
            version: ZenzVersionConfig::default(),
            input_style: InputStyle::Direct,
            original_surface: "あ".into(),
            suffix_count: 0,
            predicted_text: "いうえお".into(),
        };
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str("あい", InputStyle::Direct, &tables);
        assert_eq!(
            cache.remaining_prediction(&composing, 10),
            Some("うえお".into())
        );

        composing.insert_str("う", InputStyle::Direct, &tables);
        assert_eq!(cache.remaining_prediction(&composing, 1), Some("え".into()));
    }

    #[test]
    fn zenz_prediction_cache_handles_roman_input_and_rejects_divergence() {
        let roman = ZenzPredictionCache {
            left_context: String::new(),
            version: ZenzVersionConfig::default(),
            input_style: InputStyle::RomanToKana,
            original_surface: "k".into(),
            suffix_count: 1,
            predicted_text: "カナ".into(),
        };
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str("ka", InputStyle::RomanToKana, &tables);
        assert_eq!(
            roman.remaining_prediction(&composing, 10),
            Some("ナ".into())
        );
        composing.insert_str("na", InputStyle::RomanToKana, &tables);
        assert_eq!(roman.remaining_prediction(&composing, 10), None);

        let direct = ZenzPredictionCache {
            input_style: InputStyle::Direct,
            original_surface: "あ".into(),
            suffix_count: 0,
            predicted_text: "いう".into(),
            ..roman
        };
        let mut diverged = ComposingText::new();
        diverged.insert_str("あか", InputStyle::Direct, &tables);
        assert_eq!(direct.remaining_prediction(&diverged, 10), None);
    }

    #[test]
    fn stable_prediction_cache_keeps_only_prefix_compatible_candidates() {
        let cache = StablePredictionCache {
            original_surface: "あいうえおかきくk".into(),
            suffix_count: 1,
            candidates: vec![
                prediction_candidate("あいうえおかきくけこ", ComposingCount::Surface(9)),
                prediction_candidate("あいうえおかきくけど", ComposingCount::Surface(9)),
                prediction_candidate("別候補", ComposingCount::Surface(3)),
            ],
        };
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str("あいうえおかきくけ", InputStyle::Direct, &tables);
        let source = PredictiveInputSource {
            base_surface: composing.surface(),
            possible_nexts: Vec::new(),
            dropped_suffix_count: 0,
        };

        assert_eq!(
            cache
                .compatible_candidates(&composing, &source)
                .into_iter()
                .map(|candidate| candidate.text)
                .collect::<Vec<_>>(),
            ["あいうえおかきくけこ", "あいうえおかきくけど"]
        );
    }

    #[test]
    fn stable_prediction_cache_uses_roman_possible_nexts_and_updates_consumed_count() {
        let cache = StablePredictionCache {
            original_surface: "あいうえおかきくけ".into(),
            suffix_count: 0,
            candidates: vec![
                prediction_candidate("あいうえおかきくけこ", ComposingCount::Surface(9)),
                prediction_candidate("あいうえおかきくけど", ComposingCount::Surface(9)),
            ],
        };
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str("あいうえおかきくけ", InputStyle::Direct, &tables);
        composing.insert_str("k", InputStyle::RomanToKana, &tables);
        let source = predictive_input_source(&composing, &tables);
        let compatible = cache.compatible_candidates(&composing, &source);

        assert_eq!(
            compatible
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["あいうえおかきくけこ"]
        );
        assert_eq!(
            compatible[0].composing_count,
            ComposingCount::Surface(composing.surface_graphemes().len())
        );
    }
}
