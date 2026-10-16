use std::collections::BTreeMap;

use unicode_segmentation::UnicodeSegmentation;

use super::RecallRecord;

#[derive(Clone, Copy)]
pub(super) enum MatchKind {
    Exact,
    Prefixes,
    Completions,
}

/// Positions into the canonical recall vector, never a separately scored store.
#[derive(Default)]
pub(super) struct RecallIndex {
    readings: BTreeMap<String, Vec<usize>>,
}

impl RecallIndex {
    pub(super) fn new(records: &[RecallRecord]) -> Self {
        let mut index = Self::default();
        for (position, record) in records.iter().enumerate() {
            index
                .readings
                .entry(record.reading.clone())
                .or_default()
                .push(position);
        }
        index
    }

    pub(super) fn lookup(&self, ruby: &str, kind: MatchKind, maximum_length: usize) -> Vec<usize> {
        if ruby.is_empty() || maximum_length == 0 {
            return Vec::new();
        }
        let mut matches: Vec<usize> = Vec::new();
        match kind {
            MatchKind::Exact => {
                if UnicodeSegmentation::graphemes(ruby, true).count() <= maximum_length {
                    matches.extend(self.readings.get(ruby).into_iter().flatten());
                }
            }
            MatchKind::Prefixes => {
                let mut end = 0;
                for grapheme in UnicodeSegmentation::graphemes(ruby, true).take(maximum_length) {
                    end += grapheme.len();
                    matches.extend(self.readings.get(&ruby[..end]).into_iter().flatten());
                }
            }
            MatchKind::Completions => {
                for (reading, positions) in self.readings.range(ruby.to_owned()..) {
                    if !reading.starts_with(ruby) {
                        break;
                    }
                    if reading != ruby
                        && UnicodeSegmentation::graphemes(reading.as_str(), true).count()
                            <= maximum_length
                    {
                        matches.extend(positions);
                    }
                }
            }
        }
        // Recall recency chooses a bounded candidate protection lane; it does
        // not change lexical scores or the comparison policy's probabilities.
        matches.sort_unstable_by(|left, right| right.cmp(left));
        matches
    }
}
