//! Text-free selection agreement. Raw provenance stays inside a non-serializable snapshot.
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{Candidate, DictionaryMetadata, InputTableRegistry, LearningScope, LearningTask};

use super::ConversionSession;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementStatus {
    Comparable,
    NoPresentation,
    StalePresentation,
    UnsupportedDefault,
    SelectionNotPresented,
    UnsupportedSelection,
    DifferentScope,
    BaselineUnavailable,
    BaselineDifferentScope,
}

/// Only enums, counts and booleans may cross the measurement/persistence boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SelectionMeasurement {
    pub status: MeasurementStatus,
    pub task: Option<LearningTask>,
    pub selected_task: Option<LearningTask>,
    pub default_matches_selection: Option<bool>,
    pub baseline_matches_selection: Option<bool>,
    pub default_changed: Option<bool>,
    pub source_surface_count: usize,
    pub consumed_surface_count: usize,
    pub candidate_count: usize,
    pub selected_rank: Option<usize>,
    pub selected_recalled: bool,
    pub default_recalled: bool,
    pub fixed_prefix: bool,
}

impl SelectionMeasurement {
    pub fn unavailable(status: MeasurementStatus) -> Self {
        Self {
            status,
            task: None,
            selected_task: None,
            default_matches_selection: None,
            baseline_matches_selection: None,
            default_changed: None,
            source_surface_count: 0,
            consumed_surface_count: 0,
            candidate_count: 0,
            selected_rank: None,
            selected_recalled: false,
            default_recalled: false,
            fixed_prefix: false,
        }
    }
}

/// Deliberately has no Debug/Serialize implementation: it contains private text in RAM only.
#[derive(Clone)]
pub struct MeasurementPresentation {
    source: String,
    fixed_prefix: String,
    baseline: Option<LearningScope>,
    baseline_text: Option<String>,
    default: Candidate,
    scope: Option<LearningScope>,
    choices: Vec<Candidate>,
}

impl MeasurementPresentation {
    /// Additional visible candidates do not replace the initial default or its baseline.
    pub fn update_choices(&mut self, choices: &[Candidate]) {
        self.choices = choices.to_vec();
    }
}

impl ConversionSession {
    /// Capture a baseline chosen by the adapter BEFORE local reranking and an actual initial
    /// default chosen AFTER merging/reranking. No learning-memory handle or inference is needed.
    pub fn measure_presentation(
        &self,
        baseline: Option<&Candidate>,
        default: &Candidate,
        candidates: &[Candidate],
        tables: &InputTableRegistry,
        left_context: &str,
    ) -> MeasurementPresentation {
        MeasurementPresentation {
            source: self
                .full_composing
                .as_ref()
                .unwrap_or(&self.composing)
                .surface(),
            fixed_prefix: self
                .accepted_prediction()
                .map(super::candidate_dictionary_text)
                .unwrap_or_default(),
            baseline: baseline
                .and_then(|candidate| self.learning_scope(candidate, tables, left_context)),
            baseline_text: baseline.map(|candidate| candidate.text.clone()),
            default: default.clone(),
            scope: self.learning_scope(default, tables, left_context),
            choices: candidates.to_vec(),
        }
    }

    /// Call before selecting/consuming a prefix. Equality of count enum variants is irrelevant;
    /// identity uses the real consumed surface and original dictionary path.
    pub fn measure_selection(
        &self,
        shown: &MeasurementPresentation,
        selected: &Candidate,
        tables: &InputTableRegistry,
    ) -> SelectionMeasurement {
        let mut result = SelectionMeasurement {
            source_surface_count: shown.source.graphemes(true).count(),
            consumed_surface_count: self.consumed_surface_count(selected, tables),
            candidate_count: shown.choices.len(),
            selected_recalled: recalled(selected),
            default_recalled: recalled(&shown.default),
            fixed_prefix: !shown.fixed_prefix.is_empty(),
            ..SelectionMeasurement::unavailable(MeasurementStatus::StalePresentation)
        };
        let fixed_prefix = self
            .accepted_prediction()
            .map(super::candidate_dictionary_text)
            .unwrap_or_default();
        if self
            .full_composing
            .as_ref()
            .unwrap_or(&self.composing)
            .surface()
            != shown.source
            || fixed_prefix != shown.fixed_prefix
        {
            return result;
        }
        let Some(scope) = &shown.scope else {
            result.status = MeasurementStatus::UnsupportedDefault;
            return result;
        };
        result.task = Some(scope.task);
        let selected_scope = self.learning_scope(selected, tables, &scope.context);
        result.selected_task = selected_scope.as_ref().map(|scope| scope.task);
        result.selected_rank = shown.choices.iter().position(|candidate| {
            candidate.text == selected.text
                && candidate.entries == selected.entries
                && self.consumed_surface_count(candidate, tables) == result.consumed_surface_count
        });
        if result.selected_rank.is_none() {
            result.status = MeasurementStatus::SelectionNotPresented;
            return result;
        }
        let Some(selected_scope) = selected_scope else {
            result.status = MeasurementStatus::UnsupportedSelection;
            return result;
        };
        if selected_scope != *scope {
            result.status = MeasurementStatus::DifferentScope;
            return result;
        }
        result.default_matches_selection = Some(selected.text == shown.default.text);
        let Some(baseline) = &shown.baseline else {
            result.status = MeasurementStatus::BaselineUnavailable;
            return result;
        };
        if baseline != scope {
            result.status = MeasurementStatus::BaselineDifferentScope;
            return result;
        }
        result.baseline_matches_selection = shown
            .baseline_text
            .as_ref()
            .map(|text| *text == selected.text);
        result.default_changed = shown
            .baseline_text
            .as_ref()
            .map(|text| *text != shown.default.text);
        result.status = MeasurementStatus::Comparable;
        result
    }
}

fn recalled(candidate: &Candidate) -> bool {
    candidate
        .entries
        .iter()
        .any(|entry| entry.metadata.contains(DictionaryMetadata::LEARNED))
}
