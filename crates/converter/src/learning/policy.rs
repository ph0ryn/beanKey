use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{Candidate, DictionaryEntry};

use super::{LearningScope, MAX_PATH_ENTRIES, MAX_READING_BYTES, MAX_TEXT_BYTES, path_reading};

pub(super) const MAX_PAIR_SUPPORT: i8 = 8;

/// A conservative local choice policy, not a frequency or BT estimate.
/// Evidence probabilities retain the mass of endpoints absent from a shortlist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreferenceChallenge {
    pub confidence: f64,
    pub preferred_probability: f64,
    pub anchor_probability: f64,
}

impl PreferenceChallenge {
    /// Mixture margin. The base arguments must be probabilities in [0, 1], not
    /// lexical scores/logits; a positive result prefers the challenger.
    pub fn margin(&self, base_preferred: f64, base_anchor: f64) -> f64 {
        (1.0 - self.confidence) * (base_preferred - base_anchor)
            + self.confidence * (self.preferred_probability - self.anchor_probability)
    }

    /// True even for arbitrarily unfavorable base-model odds; no additive score
    /// cap can permanently exclude a repeatedly corrected rare name.
    pub fn wins_without_model(&self) -> bool {
        self.confidence * (self.preferred_probability - self.anchor_probability)
            > 1.0 - self.confidence
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EndpointKey {
    text: String,
    reading: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Endpoint {
    key: EndpointKey,
    // Needed for coherent word/phrase projection deletion even when this
    // endpoint is the loser, has another reading, or has alternate segmentation.
    words: Vec<String>,
}

impl Endpoint {
    pub(super) fn new(candidate: &Candidate) -> Self {
        Self {
            key: EndpointKey {
                text: candidate.text.clone(),
                reading: path_reading(&candidate.entries),
            },
            words: candidate
                .entries
                .iter()
                .map(|entry| entry.word.clone())
                .collect(),
        }
    }

    pub(super) fn valid(&self) -> bool {
        !self.key.text.is_empty()
            && self.key.text.len() <= MAX_TEXT_BYTES
            && !self.key.text.chars().any(char::is_control)
            && !self.key.reading.is_empty()
            && self.key.reading.len() <= MAX_READING_BYTES
            && !self.key.reading.chars().any(char::is_control)
            && !self.words.is_empty()
            && self.words.len() <= MAX_PATH_ENTRIES
            && self
                .words
                .iter()
                .all(|word| !word.is_empty() && !word.chars().any(char::is_control))
            && self.words.concat() == self.key.text
    }

    fn byte_budget(&self) -> usize {
        512 + 2
            * (self.key.text.len()
                + self.key.reading.len()
                + self.words.iter().map(String::len).sum::<usize>())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ComparisonRecord {
    pub(super) scope: LearningScope,
    pub(super) first: Endpoint,
    pub(super) second: Endpoint,
    // Canonical unordered pair: positive means `second` was selected over
    // `first`, negative the reverse. No separate wins/losses survive cancellation.
    pub(super) support: i8,
}

impl ComparisonRecord {
    pub(super) fn same_pair(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.first.key == other.first.key
            && self.second.key == other.second.key
    }

    pub(super) fn valid(&self) -> bool {
        self.scope.valid()
            && self.scope == self.scope.normalized()
            && self.first.valid()
            && self.second.valid()
            && self.first.key < self.second.key
            && self.first.key.text != self.second.key.text
            && self.support != 0
            && self.support.unsigned_abs() <= MAX_PAIR_SUPPORT as u8
    }

    pub(super) fn byte_budget(&self) -> usize {
        512 + 6 * (self.scope.reading.len() + self.scope.context.len() + self.scope.action.len())
            + self.first.byte_budget()
            + self.second.byte_budget()
    }
}

pub(super) fn record(
    records: &mut Vec<ComparisonRecord>,
    scope: LearningScope,
    default: Endpoint,
    selected: Endpoint,
) {
    let (first, second, delta) = if default.key < selected.key {
        (default, selected, 1)
    } else {
        (selected, default, -1)
    };
    let mut incoming = ComparisonRecord {
        scope,
        first,
        second,
        support: delta,
    };
    if let Some(index) = records
        .iter()
        .position(|record| record.same_pair(&incoming))
    {
        let previous = records.remove(index);
        incoming.support = (previous.support + delta).clamp(-MAX_PAIR_SUPPORT, MAX_PAIR_SUPPORT);
    }
    if incoming.support != 0 {
        records.push(incoming);
    }
}

pub(super) fn challenge(
    records: &[ComparisonRecord],
    scope: &LearningScope,
    anchor: &Candidate,
    challenger: &Candidate,
) -> Option<PreferenceChallenge> {
    let anchor_key = Endpoint::new(anchor).key;
    let challenger_key = Endpoint::new(challenger).key;
    let mut net: HashMap<&EndpointKey, i64> = HashMap::new();
    for record in records.iter().filter(|record| &record.scope == scope) {
        *net.entry(&record.first.key).or_default() -= i64::from(record.support);
        *net.entry(&record.second.key).or_default() += i64::from(record.support);
    }
    // Cycles and balanced endpoints produce no positive support. Unrelated
    // winners retain their probability mass, but cannot inflate this candidate's
    // confidence (which depends solely on its own net wins minus losses).
    let preferred = (*net.get(&challenger_key)?).max(0) as f64;
    let anchor = net.get(&anchor_key).copied().unwrap_or(0).max(0) as f64;
    if preferred <= anchor {
        return None;
    }
    let total: f64 = net.values().map(|support| (*support).max(0) as f64).sum();
    Some(PreferenceChallenge {
        confidence: preferred / (preferred + 2.0),
        preferred_probability: preferred / total,
        anchor_probability: anchor / total,
    })
}

pub(super) struct ForgetTargets {
    words: HashSet<String>,
}

impl ForgetTargets {
    pub(super) fn new(candidate: &Candidate) -> Self {
        let mut words: HashSet<_> = candidate
            .entries
            .iter()
            .map(|entry| entry.word.clone())
            .filter(|word| !word.is_empty())
            .collect();
        if !candidate.text.is_empty() {
            words.insert(candidate.text.clone());
        }
        Self { words }
    }

    fn text_matches(&self, text: &str) -> bool {
        // Coarse deletion is intentional, across readings and segmentations:
        // forgetting a phrase must also remove a longer stored projection that
        // contains it, even if that projection used different dictionary nodes.
        self.words.iter().any(|word| text.contains(word))
    }

    pub(super) fn path_matches(&self, path: &[DictionaryEntry]) -> bool {
        self.text_matches(
            &path
                .iter()
                .map(|entry| entry.word.as_str())
                .collect::<String>(),
        )
    }

    pub(super) fn endpoint_matches(&self, endpoint: &Endpoint) -> bool {
        self.text_matches(&endpoint.key.text)
    }
}
