use super::{ConversionSession, candidate_dictionary_text, candidate_reading};
use crate::{
    Candidate, ComposingCount, ComposingText, DictionaryError, InputTableRegistry, LearningScope,
    LearningTask, NormalConverter, PrefixConstraint, TypoCorrectionMode, ZenzComparisonRequest,
    ZenzEvaluator, ZenzLanguageModel, ZenzVersionConfig, to_katakana,
};
use unicode_segmentation::UnicodeSegmentation;

/// Optional, bounded local comparison; never part of ordinary Zenzai greedy acceptance.
pub struct PersonalizationComparison<'a> {
    pub model: &'a mut dyn ZenzLanguageModel,
    pub evaluator: &'a mut ZenzEvaluator,
    pub version: &'a ZenzVersionConfig,
    pub token_budget: usize,
}

#[derive(Clone)]
pub(super) struct Presentation {
    source: String,
    default: Candidate,
    scope: LearningScope,
    choices: Vec<(Candidate, LearningScope)>,
}

impl ConversionSession {
    pub(super) fn personal_candidates(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        prediction: bool,
        typo_correction: TypoCorrectionMode,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let Some(memory) = &self.learning_memory else {
            return Ok(Vec::new());
        };
        let additional = self.additional_dictionary();
        let recalled = (|| {
            let mut candidates = converter.personal_candidates(
                &self.composing,
                tables,
                &additional,
                self.context,
                memory,
                crate::lattice::PersonalRecallOptions {
                    maximum: 4,
                    prediction,
                },
            )?;
            if !prediction && typo_correction == TypoCorrectionMode::Enabled {
                candidates.extend(converter.personal_typo_candidates(
                    &self.composing,
                    tables,
                    self.context,
                    memory,
                    4 - candidates.len(),
                )?);
            }
            Ok(candidates)
        })();
        let mut candidates = match recalled {
            Ok(candidates) => candidates,
            Err(DictionaryError::Learning(error)) => {
                self.personalization_warning = Some(error.to_string());
                return Ok(Vec::new());
            }
            Err(error) => return Err(error),
        };
        if let Some(prefix) = self.accepted_prediction() {
            let fixed = candidate_dictionary_text(prefix);
            candidates.retain(|candidate| candidate_dictionary_text(candidate).starts_with(&fixed));
        }
        Ok(candidates)
    }

    fn learning_scope(
        &self,
        candidate: &Candidate,
        tables: &InputTableRegistry,
        left_context: &str,
    ) -> Option<LearningScope> {
        if !candidate.is_learning_target
            || candidate.is_typo_correction
            || candidate.entries.is_empty()
        {
            return None;
        }
        let consumed = self.consumed_surface_count(candidate, tables);
        if self
            .retained_prediction()
            .is_some_and(|fixed| consumed < candidate_reading(fixed).graphemes(true).count())
        {
            // Segmenting inside a fixed reading cannot turn it into an ordinary
            // unconstrained action or reuse ordinary partial-conversion evidence.
            return None;
        }
        let observed = self
            .composing
            .surface_graphemes()
            .iter()
            .take(consumed)
            .map(String::as_str)
            .collect::<String>();
        let reading = to_katakana(&observed);
        let completed = candidate_reading(candidate);
        if consumed == 0 || !completed.starts_with(&reading) {
            return None;
        }
        let source = self.full_composing.as_ref().unwrap_or(&self.composing);
        let task = if completed != reading {
            LearningTask::Prediction
        } else if consumed < source.surface_graphemes().len() {
            LearningTask::PartialConversion
        } else {
            LearningTask::Conversion
        };
        let fixed = self
            .accepted_prediction()
            .map(candidate_dictionary_text)
            .unwrap_or_default();
        if !candidate_dictionary_text(candidate).starts_with(&fixed) {
            return None;
        }
        Some(
            LearningScope::new(reading, task, left_context)
                .with_action(format!("span={consumed};fixed={fixed}")),
        )
    }

    /// Invalidate an observation when an adapter hides its candidate presentation.
    pub fn discard_presentation(&mut self) {
        self.presentation = None;
    }

    /// Captures the actual personalized default once, not a later navigation highlight.
    /// Adapters call this after merging the final visible list and choosing its initial default.
    pub fn observe_presentation(
        &mut self,
        candidates: &[Candidate],
        default_index: usize,
        tables: &InputTableRegistry,
        left_context: &str,
    ) {
        self.presentation = None;
        if self.learning_memory.is_none() {
            return;
        }
        let Some(default) = candidates.get(default_index) else {
            return;
        };
        let Some(scope) = self.learning_scope(default, tables, left_context) else {
            return;
        };
        let choices = candidates
            .iter()
            .filter_map(|candidate| {
                self.learning_scope(candidate, tables, left_context)
                    .map(|scope| (candidate.clone(), scope))
            })
            .collect();
        self.presentation = Some(Presentation {
            source: self
                .full_composing
                .as_ref()
                .unwrap_or(&self.composing)
                .surface(),
            default: default.clone(),
            scope,
            choices,
        });
    }

    /// Used only while choosing a newly presented initial default, never on navigation.
    pub fn reanchor_presentation(&mut self, default: &Candidate) {
        if let Some(shown) = &mut self.presentation
            && let Some((candidate, scope)) = shown
                .choices
                .iter()
                .find(|(candidate, _)| same_candidate(candidate, default))
        {
            shown.default = candidate.clone();
            shown.scope = scope.clone();
        }
    }

    pub(super) fn correction_observation(
        &self,
        selected: &Candidate,
        tables: &InputTableRegistry,
    ) -> Option<(LearningScope, Candidate)> {
        let shown = self.presentation.as_ref()?;
        if self.composing.surface() != shown.source || selected.text == shown.default.text {
            return None;
        }
        // Selection identity includes the original dictionary path, not merely the display string.
        let (_, selected_scope) = shown
            .choices
            .iter()
            .find(|(candidate, _)| same_candidate(candidate, selected))?;
        if selected_scope != &shown.scope {
            return None;
        }
        let current = self.learning_scope(selected, tables, &shown.scope.context)?;
        if current != shown.scope {
            return None;
        }
        Some((shown.scope.clone(), shown.default.clone()))
    }

    /// Reorders only small same-task/span groups. Unscored candidates retain their relative order.
    /// The evidence distribution is computed by LearningMemory over the whole original scope.
    pub fn personalize_candidates(
        &mut self,
        converter: &NormalConverter<'_>,
        tables: &InputTableRegistry,
        candidates: &mut Vec<Candidate>,
        left_context: &str,
        mut comparison: Option<PersonalizationComparison<'_>>,
    ) {
        let Some(memory) = self.learning_memory.clone() else {
            return;
        };
        let mut visited = Vec::new();
        // At most four visible task/span groups, one bounded LM comparison per request.
        for anchor_index in 0..candidates.len().min(16) {
            let Some(scope) = self.learning_scope(&candidates[anchor_index], tables, left_context)
            else {
                continue;
            };
            if visited.contains(&scope) {
                continue;
            }
            if visited.len() == 4 {
                break;
            }
            visited.push(scope.clone());
            let anchor = &candidates[anchor_index];
            let mut challengers = candidates
                .iter()
                .enumerate()
                .skip(anchor_index + 1)
                .filter(|(_, candidate)| {
                    self.learning_scope(candidate, tables, left_context)
                        .as_ref()
                        == Some(&scope)
                })
                .filter_map(|(index, candidate)| {
                    memory
                        .challenge(&scope, anchor, candidate)
                        .ok()
                        .flatten()
                        .map(|evidence| (index, evidence))
                })
                .collect::<Vec<_>>();
            if challengers.is_empty() {
                continue;
            }
            challengers.sort_by(|left, right| {
                let strength = |e: &crate::PreferenceChallenge| {
                    e.confidence * (e.preferred_probability - e.anchor_probability)
                };
                strength(&right.1)
                    .total_cmp(&strength(&left.1))
                    .then_with(|| left.0.cmp(&right.0))
            });
            if let Some((index, _)) = challengers
                .iter()
                .find(|(_, evidence)| evidence.wins_without_model())
            {
                promote(candidates, *index, anchor_index);
                continue;
            }
            // Completion is a different objective; no conversion scorer or unconditioned LM prior.
            if matches!(
                scope.task,
                LearningTask::Prediction | LearningTask::PostPrediction
            ) {
                continue;
            }
            challengers.truncate(3);
            let mut group = vec![anchor.clone()];
            group.extend(
                challengers
                    .iter()
                    .map(|(index, _)| candidates[*index].clone()),
            );
            let dictionary_scores = group
                .iter()
                .map(|candidate| converter.candidate_path_value(candidate, self.context))
                .collect::<Result<Vec<_>, _>>();
            let Ok(dictionary_scores) = dictionary_scores else {
                continue;
            };
            let base_scores = if let Some(comparison) = comparison.as_mut() {
                if comparison.token_budget == 0 {
                    continue;
                }
                // Normalize count representation, not the true consumed span validated above.
                let consumed = self.consumed_surface_count(anchor, tables);
                for candidate in &mut group {
                    candidate.composing_count = ComposingCount::Surface(consumed);
                }
                let input = to_katakana(&self.zenz_model_composing().surface());
                let constraint = self.constrain_zenz_prefix(PrefixConstraint::default());
                let result = comparison.evaluator.compare_candidates(
                    comparison.model,
                    ZenzComparisonRequest {
                        input: &input,
                        input_cursor_position: Some(consumed),
                        candidates: &group,
                        prefix_constraint: &constraint,
                        version: comparison.version,
                        token_budget: comparison.token_budget,
                    },
                );
                comparison.token_budget = 0;
                match result {
                    Ok(Some(scores)) => scores
                        .into_iter()
                        .zip(&dictionary_scores)
                        .map(|(lm, dictionary)| lm + 0.05 * f64::from(*dictionary))
                        .collect::<Vec<_>>(),
                    Ok(None) => continue,
                    Err(error) => {
                        self.personalization_warning = Some(error.to_string());
                        continue;
                    }
                }
            } else {
                dictionary_scores
                    .iter()
                    .map(|score| 0.2 * f64::from(*score))
                    .collect()
            };
            let Some(base) = softmax(&base_scores) else {
                continue;
            };
            let winner = challengers
                .iter()
                .enumerate()
                .filter_map(|(offset, (index, evidence))| {
                    let margin = evidence.margin(base[offset + 1], base[0]);
                    (margin > 0.0).then_some((*index, margin))
                })
                .max_by(|left, right| {
                    left.1
                        .total_cmp(&right.1)
                        .then_with(|| right.0.cmp(&left.0))
                });
            if let Some((index, _)) = winner {
                promote(candidates, index, anchor_index);
            }
        }
    }

    /// Finds the personalized equivalent of a main/preview default after Desktop list merging.
    pub fn preferred_for(
        &self,
        baseline: &Candidate,
        candidates: &[Candidate],
        tables: &InputTableRegistry,
        left_context: &str,
    ) -> Candidate {
        let Some(scope) = self.learning_scope(baseline, tables, left_context) else {
            return baseline.clone();
        };
        candidates
            .iter()
            .find(|candidate| {
                self.learning_scope(candidate, tables, left_context)
                    .as_ref()
                    == Some(&scope)
            })
            .unwrap_or(baseline)
            .clone()
    }

    pub fn take_personalization_warning(&mut self) -> Option<String> {
        self.personalization_warning.take()
    }
}

fn same_candidate(left: &Candidate, right: &Candidate) -> bool {
    left.text == right.text
        && left.composing_count == right.composing_count
        && left.entries == right.entries
}

pub(super) fn append_unique(
    candidates: &mut Vec<Candidate>,
    recalled: impl IntoIterator<Item = Candidate>,
    composing: &ComposingText,
    tables: &InputTableRegistry,
) {
    for candidate in recalled {
        let consumed = consumed_surface_count(composing, &candidate, tables);
        if !candidates.iter().any(|existing| {
            existing.text == candidate.text
                && consumed_surface_count(composing, existing, tables) == consumed
        }) {
            candidates.push(candidate);
        }
    }
}

pub(super) fn consumed_surface_count(
    composing: &ComposingText,
    candidate: &Candidate,
    tables: &InputTableRegistry,
) -> usize {
    let mut remaining = composing.clone();
    remaining.complete_prefix(candidate.composing_count.clone(), tables);
    composing
        .surface_graphemes()
        .len()
        .saturating_sub(remaining.surface_graphemes().len())
}

fn promote(candidates: &mut Vec<Candidate>, index: usize, anchor: usize) {
    let winner = candidates.remove(index);
    candidates.insert(anchor, winner);
}

fn softmax(scores: &[f64]) -> Option<Vec<f64>> {
    if scores.is_empty() || scores.iter().any(|score| !score.is_finite()) {
        return None;
    }
    let maximum = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights = scores
        .iter()
        .map(|score| (score - maximum).exp())
        .collect::<Vec<_>>();
    let total = weights.iter().sum::<f64>();
    Some(weights.into_iter().map(|weight| weight / total).collect())
}
