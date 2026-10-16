mod index;
mod persistence;
mod policy;

#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::kana::to_katakana;
use crate::lattice::is_clause;
use crate::{
    Candidate, CharacterIdMap, DictionaryEntry, DictionaryMetadata, PostCompositionPrediction,
    PostPredictionKind,
};

use index::{MatchKind, RecallIndex};
pub use policy::PreferenceChallenge;
use policy::{ComparisonRecord, Endpoint};

// Counts, individual inputs, query output, and serialized bytes are independently
// bounded. Normal accepts never compete with comparisons for their retention budget.
const MAX_RECORDS: usize = 16_384;
const MAX_PATH_ENTRIES: usize = 64;
const MAX_READING_BYTES: usize = 4_096;
const MAX_TEXT_BYTES: usize = 4_096;
const MAX_PATH_BYTES: usize = 16_384;
const MAX_CONTEXT_GRAPHEMES: usize = 32;
const MAX_CONTEXT_BYTES: usize = 1_024;
const MAX_ACTION_BYTES: usize = 8_192;
const MAX_RECALL_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_COMPARISON_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_LOOKUP_PATHS: usize = 64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LearningMode {
    InputAndOutput,
    OnlyOutput,
    #[default]
    Nothing,
}

impl LearningMode {
    fn uses_memory(self) -> bool {
        self != Self::Nothing
    }

    fn updates_memory(self) -> bool {
        self == Self::InputAndOutput
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum LearningTask {
    Conversion,
    PartialConversion,
    Prediction,
    PostPrediction,
}

/// The actual consumed, observed reading, task, and a small suffix of left context.
/// In particular, a prediction's completed dictionary reading is not its scope.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LearningScope {
    pub reading: String,
    pub task: LearningTask,
    pub context: String,
    /// Exact action/fixed-prefix provenance; independent of bounded left context.
    #[serde(default)]
    pub action: String,
}

impl LearningScope {
    pub fn new(reading: impl AsRef<str>, task: LearningTask, context: impl AsRef<str>) -> Self {
        Self {
            reading: to_katakana(reading.as_ref()),
            task,
            context: bounded_context(context.as_ref()),
            action: String::new(),
        }
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = action.into();
        self
    }

    fn normalized(&self) -> Self {
        Self::new(&self.reading, self.task, &self.context).with_action(self.action.clone())
    }

    fn valid(&self) -> bool {
        !self.reading.is_empty()
            && self.reading.len() <= MAX_READING_BYTES
            && !self.reading.chars().any(char::is_control)
            && self.context.len() <= MAX_CONTEXT_BYTES
            && !self.context.contains('\0')
            && self.action.len() <= MAX_ACTION_BYTES
            && !self.action.contains('\0')
    }
}

fn bounded_context(context: &str) -> String {
    let mut suffix = Vec::new();
    let mut bytes = 0;
    for grapheme in UnicodeSegmentation::graphemes(context, true)
        .rev()
        .take(MAX_CONTEXT_GRAPHEMES)
    {
        if bytes + grapheme.len() > MAX_CONTEXT_BYTES {
            break;
        }
        bytes += grapheme.len();
        suffix.push(grapheme);
    }
    suffix.reverse();
    suffix.concat()
}

#[derive(Debug)]
pub enum LearningError {
    Io { path: PathBuf, source: io::Error },
    InvalidData(&'static str),
    Poisoned,
}

impl fmt::Display for LearningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to access {}: {source}", path.display())
            }
            Self::InvalidData(reason) => write!(formatter, "invalid learning memory: {reason}"),
            Self::Poisoned => write!(formatter, "learning memory lock is poisoned"),
        }
    }
}

impl Error for LearningError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidData(_) | Self::Poisoned => None,
        }
    }
}

/// Clones share one canonical state and one coalescing writer. Input-path methods
/// only change memory; commit/forget/reset wait for the single writer's barrier.
#[derive(Clone)]
pub struct LearningMemory {
    inner: Arc<MemoryInner>,
}

struct MemoryInner {
    runtime: Arc<Runtime>,
    writer: Mutex<Option<JoinHandle<()>>>,
    // Mode changes release the state lock while draining the writer. Serialize
    // them so a failed transition can restore its original mode atomically.
    mode_change: Mutex<()>,
}

struct Runtime {
    state: Mutex<LearningState>,
    changed: Condvar,
    #[cfg(test)]
    write_gate: Mutex<Option<Arc<tests::WriteGate>>>,
}

struct LearningState {
    directory: PathBuf,
    character_ids: CharacterIdMap,
    mode: LearningMode,
    max_count: usize,
    loaded: bool,
    store: CanonicalStore,
    recall_index: RecallIndex,
    revision: u64,
    durable_revision: u64,
    acknowledged_revision: u64,
    requested_save: u64,
    completed_save: u64,
    pending: bool,
    flush_requested: bool,
    stopping: bool,
    writer_stopped: bool,
    last_error: Option<persistence::SaveFailure>,
}

#[derive(Clone, Default)]
struct CanonicalStore {
    // Both vectors are oldest-to-newest LRU, not event logs or frequency counts.
    recall: Vec<RecallRecord>,
    comparisons: Vec<ComparisonRecord>,
}

#[derive(Clone)]
struct RecallRecord {
    path: Vec<DictionaryEntry>,
    reading: String,
}

impl RecallRecord {
    fn new(path: Vec<DictionaryEntry>) -> Self {
        let reading = path_reading(&path);
        Self { path, reading }
    }

    fn byte_budget(&self) -> usize {
        // Upper bound for JSON string escaping and all per-entry scalar fields.
        128 + self
            .path
            .iter()
            .map(|entry| 2 * (entry.word.len() + entry.ruby.len()) + 256)
            .sum::<usize>()
    }
}

impl LearningMemory {
    pub fn open(
        directory: impl Into<PathBuf>,
        mode: LearningMode,
        max_count: usize,
        character_ids: CharacterIdMap,
    ) -> Result<Self, LearningError> {
        let directory = directory.into();
        if mode.updates_memory() {
            fs::create_dir_all(&directory).map_err(|source| LearningError::Io {
                path: directory.clone(),
                source,
            })?;
        }
        // Deliberately do not read, recover, migrate, or remove the old memory.*
        // files. Their synthetic entries cannot recover original dictionary costs
        // or attest to a displayed default->selection correction.
        let mut store = if mode.uses_memory() {
            persistence::load(&directory, &character_ids)?
        } else {
            CanonicalStore::default()
        };
        let max_count = max_count.min(MAX_RECORDS);
        store.bound(max_count);
        let recall_index = RecallIndex::new(&store.recall);
        let runtime = Arc::new(Runtime {
            state: Mutex::new(LearningState {
                directory,
                character_ids,
                mode,
                max_count,
                loaded: mode.uses_memory(),
                store,
                recall_index,
                revision: 0,
                durable_revision: 0,
                acknowledged_revision: 0,
                requested_save: 0,
                completed_save: 0,
                pending: false,
                flush_requested: false,
                stopping: false,
                writer_stopped: false,
                last_error: None,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            write_gate: Mutex::new(None),
        });
        let writer = persistence::spawn(Arc::clone(&runtime))?;
        Ok(Self {
            inner: Arc::new(MemoryInner {
                runtime,
                writer: Mutex::new(Some(writer)),
                mode_change: Mutex::new(()),
            }),
        })
    }

    /// Single-entry recall only. Multiword records are available as original paths.
    pub fn entries(&self) -> Result<Vec<DictionaryEntry>, LearningError> {
        let state = self.lock()?;
        if !state.mode.uses_memory() {
            return Ok(Vec::new());
        }
        Ok(state
            .store
            .recall
            .iter()
            .rev()
            .filter(|record| record.path.len() == 1)
            .map(|record| recalled_entry(&record.path[0]))
            .collect())
    }

    pub fn exact_match(&self, ruby: &str) -> Result<Vec<DictionaryEntry>, LearningError> {
        self.single_entries(ruby, MatchKind::Exact, usize::MAX)
    }

    pub fn matches_from_start(
        &self,
        ruby: &str,
        maximum_length: usize,
    ) -> Result<Vec<DictionaryEntry>, LearningError> {
        self.single_entries(ruby, MatchKind::Prefixes, maximum_length)
    }

    pub fn predict(&self, ruby: &str) -> Result<Vec<DictionaryEntry>, LearningError> {
        self.single_entries(ruby, MatchKind::Completions, usize::MAX)
    }

    fn single_entries(
        &self,
        ruby: &str,
        kind: MatchKind,
        maximum_length: usize,
    ) -> Result<Vec<DictionaryEntry>, LearningError> {
        let state = self.lock()?;
        if !state.mode.uses_memory() || ruby.is_empty() {
            return Ok(Vec::new());
        }
        Ok(state
            .recall_index
            .lookup(&to_katakana(ruby), kind, maximum_length)
            .into_iter()
            .filter_map(|index| {
                let path = &state.store.recall[index].path;
                (path.len() == 1).then(|| recalled_entry(&path[0]))
            })
            .take(MAX_LOOKUP_PATHS)
            .collect())
    }

    /// Original paths whose reading is a prefix of `ruby`. `maximum_length`
    /// bounds reading graphemes, not entry count; at most 64 paths are returned.
    pub fn paths_from_start(
        &self,
        ruby: &str,
        maximum_length: usize,
    ) -> Result<Vec<Vec<DictionaryEntry>>, LearningError> {
        self.paths(ruby, MatchKind::Prefixes, maximum_length)
    }

    /// Original paths strictly completing `prefix`, with total reading length
    /// at most `maximum_length` graphemes. At most 64 paths are returned.
    pub fn predictive_paths(
        &self,
        prefix: &str,
        maximum_length: usize,
    ) -> Result<Vec<Vec<DictionaryEntry>>, LearningError> {
        self.paths(prefix, MatchKind::Completions, maximum_length)
    }

    fn paths(
        &self,
        ruby: &str,
        kind: MatchKind,
        maximum_length: usize,
    ) -> Result<Vec<Vec<DictionaryEntry>>, LearningError> {
        let state = self.lock()?;
        if !state.mode.uses_memory() || ruby.is_empty() {
            return Ok(Vec::new());
        }
        Ok(state
            .recall_index
            .lookup(&to_katakana(ruby), kind, maximum_length)
            .into_iter()
            .take(MAX_LOOKUP_PATHS)
            .map(|index| {
                state.store.recall[index]
                    .path
                    .iter()
                    .map(recalled_entry)
                    .collect()
            })
            .collect())
    }

    pub(crate) fn validate(&self) -> Result<(), LearningError> {
        drop(self.lock()?);
        Ok(())
    }

    /// Recall only: ordinary accepts do not create or refresh ranking evidence,
    /// and no count ever modifies DictionaryEntry costs or adjustments. Typo
    /// query entries may contain query-specific penalties, so without separate
    /// baseline provenance they must never overwrite or create canonical paths.
    pub fn learn(&self, candidate: &Candidate) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory()
            || !candidate.is_learning_target
            || candidate.is_typo_correction
            || !valid_candidate(candidate, &state.character_ids)
        {
            return Ok(());
        }
        state.memorize_paths(&candidate.entries, 0);
        self.notify_updated(&mut state);
        Ok(())
    }

    pub fn learn_post_prediction(
        &self,
        candidate: &Candidate,
        prediction: &PostCompositionPrediction,
    ) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory()
            || !candidate.is_learning_target
            || candidate.is_typo_correction
        {
            return Ok(());
        }
        let (mut path, update) = match &prediction.kind {
            PostPredictionKind::Additional { entries } => (candidate.entries.clone(), entries),
            PostPredictionKind::Replacement {
                target_entries,
                replacement_entries,
            } => {
                if !candidate.entries.ends_with(target_entries) {
                    return Ok(());
                }
                (
                    candidate.entries[..candidate.entries.len() - target_entries.len()].to_vec(),
                    replacement_entries,
                )
            }
        };
        if update.is_empty() {
            return Ok(());
        }
        let update_start = path.len();
        path.extend(update.iter().cloned());
        if !valid_path(&path, &state.character_ids) {
            return Ok(());
        }
        state.memorize_paths(&path, update_start);
        self.notify_updated(&mut state);
        Ok(())
    }

    /// Record one actual displayed default->selected correction. Caller owns
    /// event uniqueness and candidate validity/task/span checks. No broad scope
    /// is synthesized: exact left contexts cannot create a strong global habit.
    pub fn record_correction(
        &self,
        scope: &LearningScope,
        default: &Candidate,
        selected: &Candidate,
    ) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        let scope = scope.normalized();
        if !scope.valid()
            || default.is_typo_correction
            || selected.is_typo_correction
            || !valid_candidate(default, &state.character_ids)
            || !valid_candidate(selected, &state.character_ids)
            || default.text == selected.text
        {
            return Ok(());
        }
        policy::record(
            &mut state.store.comparisons,
            scope,
            Endpoint::new(default),
            Endpoint::new(selected),
        );
        let max_count = state.max_count;
        state.store.bound_comparisons(max_count);
        self.notify_updated(&mut state);
        Ok(())
    }

    pub fn challenge(
        &self,
        scope: &LearningScope,
        anchor: &Candidate,
        challenger: &Candidate,
    ) -> Result<Option<PreferenceChallenge>, LearningError> {
        let state = self.lock()?;
        if !state.mode.uses_memory() || anchor.text == challenger.text {
            return Ok(None);
        }
        let scope = scope.normalized();
        if !scope.valid() {
            return Ok(None);
        }
        Ok(policy::challenge(
            &state.store.comparisons,
            &scope,
            anchor,
            challenger,
        ))
    }

    /// Remove all matching word/phrase projections and every incident comparison
    /// (whether the forgotten endpoint won or lost), across readings and contexts.
    pub fn forget(&self, candidate: &Candidate) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        let targets = policy::ForgetTargets::new(candidate);
        state
            .store
            .recall
            .retain(|record| !targets.path_matches(&record.path));
        state.store.comparisons.retain(|record| {
            !targets.endpoint_matches(&record.first) && !targets.endpoint_matches(&record.second)
        });
        state.recall_index = RecallIndex::new(&state.store.recall);
        self.notify_updated(&mut state);
        self.durable_barrier(state)?;
        Ok(())
    }

    /// A durable barrier, including updates already saved by the background
    /// writer. Returns whether any updates have occurred since the last barrier.
    pub fn commit(&self) -> Result<bool, LearningError> {
        let state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(false);
        }
        let changed = state.revision > state.acknowledged_revision;
        self.durable_barrier(state)?;
        Ok(changed)
    }

    /// Publish an empty new-format store, leaving all rollback files untouched.
    pub fn reset(&self) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        state.store = CanonicalStore::default();
        state.recall_index = RecallIndex::default();
        self.notify_updated(&mut state);
        self.durable_barrier(state)?;
        Ok(())
    }

    pub fn set_mode(&self, mode: LearningMode) -> Result<(), LearningError> {
        let _transition = self
            .inner
            .mode_change
            .lock()
            .map_err(|_| LearningError::Poisoned)?;
        let mut state = self.lock()?;
        if state.mode == mode {
            return Ok(());
        }
        if mode.uses_memory() && !state.loaded {
            let mut store = persistence::load(&state.directory, &state.character_ids)?;
            store.bound(state.max_count);
            state.recall_index = RecallIndex::new(&store.recall);
            state.store = store;
            state.loaded = true;
        }
        if mode.updates_memory() {
            fs::create_dir_all(&state.directory).map_err(|source| LearningError::Io {
                path: state.directory.clone(),
                source,
            })?;
        }
        // Stop admitting writes before draining earlier admitted updates. A
        // failed transition is not a successful read-only switch: keep the old
        // writable mode available for explicit retry, and do not hide dirty
        // state behind a read-only mode that could otherwise write on Drop.
        let previous = state.mode;
        state.mode = mode;
        if let Err(error) = self.durable_barrier(state) {
            self.lock()?.mode = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn set_max_count(&self, max_count: usize) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        let max_count = max_count.min(MAX_RECORDS);
        if state.max_count == max_count {
            return Ok(());
        }
        state.max_count = max_count;
        state.store.bound(max_count);
        state.recall_index = RecallIndex::new(&state.store.recall);
        if state.mode.updates_memory() {
            self.notify_updated(&mut state);
        }
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, LearningState>, LearningError> {
        self.inner
            .runtime
            .state
            .lock()
            .map_err(|_| LearningError::Poisoned)
    }

    fn notify_updated(&self, state: &mut LearningState) {
        state.revision = state.revision.saturating_add(1);
        state.pending = true;
        self.inner.runtime.changed.notify_all();
    }

    fn durable_barrier(
        &self,
        mut state: MutexGuard<'_, LearningState>,
    ) -> Result<(), LearningError> {
        let target = state.revision;
        if state.durable_revision < target {
            state.requested_save = state.requested_save.saturating_add(1);
            let request = state.requested_save;
            state.pending = true;
            state.flush_requested = true;
            self.inner.runtime.changed.notify_all();
            while state.durable_revision < target {
                if state.writer_stopped {
                    return Err(LearningError::InvalidData("background writer stopped"));
                }
                if state.completed_save >= request {
                    return Err(state
                        .last_error
                        .as_ref()
                        .map(persistence::SaveFailure::to_error)
                        .unwrap_or(LearningError::InvalidData("save did not reach barrier")));
                }
                state = self
                    .inner
                    .runtime
                    .changed
                    .wait(state)
                    .map_err(|_| LearningError::Poisoned)?;
            }
        }
        state.acknowledged_revision = state.acknowledged_revision.max(target);
        Ok(())
    }
}

impl Drop for MemoryInner {
    fn drop(&mut self) {
        {
            let mut state = self
                .runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.stopping = true;
            if state.revision > state.durable_revision {
                state.pending = true;
                state.flush_requested = true;
            }
            self.runtime.changed.notify_all();
        }
        if let Some(writer) = self
            .writer
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = writer.join();
        }
    }
}

impl CanonicalStore {
    fn memorize(&mut self, path: &[DictionaryEntry]) {
        if let Some(index) = self
            .recall
            .iter()
            .position(|record| same_path(&record.path, path))
        {
            self.recall.remove(index);
        }
        self.recall.push(RecallRecord::new(path.to_vec()));
    }

    fn bound(&mut self, max_count: usize) {
        bound_records(
            &mut self.recall,
            max_count,
            MAX_RECALL_BYTES,
            RecallRecord::byte_budget,
        );
        self.bound_comparisons(max_count);
    }

    fn bound_comparisons(&mut self, max_count: usize) {
        bound_records(
            &mut self.comparisons,
            max_count,
            MAX_COMPARISON_BYTES,
            ComparisonRecord::byte_budget,
        );
    }
}

fn bound_records<T>(
    records: &mut Vec<T>,
    maximum: usize,
    byte_limit: usize,
    size: impl Fn(&T) -> usize,
) {
    let mut bytes: usize = records.iter().map(&size).sum();
    let mut remove = 0;
    while records.len() - remove > maximum || bytes > byte_limit {
        bytes -= size(&records[remove]);
        remove += 1;
    }
    records.drain(..remove);
}

impl LearningState {
    fn memorize_paths(&mut self, entries: &[DictionaryEntry], update_start: usize) {
        for entry in entries
            .iter()
            .skip(update_start)
            .filter(|entry| learns_individual_word(entry))
        {
            self.store.memorize(std::slice::from_ref(entry));
        }
        // Clause and adjacent-clause recall retain every original lexical entry;
        // no phrase is ever turned into a shortcut with omitted internal costs.
        let ranges = clause_ranges(entries);
        for &(start, end) in &ranges {
            if end > update_start && end - start > 1 {
                self.store.memorize(&entries[start..end]);
            }
        }
        for pair in ranges.windows(2) {
            if pair[1].1 > update_start {
                self.store.memorize(&entries[pair[0].0..pair[1].1]);
            }
        }
        self.store.memorize(entries);
        bound_records(
            &mut self.store.recall,
            self.max_count,
            MAX_RECALL_BYTES,
            RecallRecord::byte_budget,
        );
        self.recall_index = RecallIndex::new(&self.store.recall);
    }
}

fn learns_individual_word(entry: &DictionaryEntry) -> bool {
    let left = entry.left_id;
    !(147..=554).contains(&left)
        && !(557..=560).contains(&left)
        && !(1297..=1305).contains(&left)
        && !(6..=9).contains(&left)
        && !matches!(left, 0 | 1316)
}

fn clause_ranges(entries: &[DictionaryEntry]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if index == 0
            || is_clause(
                usize::from(entries[index - 1].right_id),
                usize::from(entry.left_id),
            )
        {
            ranges.push((index, index + 1));
        } else if let Some((_, end)) = ranges.last_mut() {
            *end = index + 1;
        }
    }
    ranges
}

fn valid_candidate(candidate: &Candidate, character_ids: &CharacterIdMap) -> bool {
    valid_path(&candidate.entries, character_ids)
        && candidate.text
            == candidate
                .entries
                .iter()
                .map(|entry| entry.word.as_str())
                .collect::<String>()
}

fn valid_path(path: &[DictionaryEntry], character_ids: &CharacterIdMap) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_ENTRIES
        && path
            .iter()
            .map(|entry| entry.word.len() + entry.ruby.len())
            .sum::<usize>()
            <= MAX_PATH_BYTES
        && path.iter().map(|entry| entry.word.len()).sum::<usize>() <= MAX_TEXT_BYTES
        && path.iter().map(|entry| entry.ruby.len()).sum::<usize>() <= MAX_READING_BYTES
        && path.iter().all(|entry| {
            !entry.word.is_empty()
                && !entry.word.chars().any(char::is_control)
                && !entry.ruby.is_empty()
                && !entry.ruby.chars().any(char::is_control)
                && entry.base_value.is_finite()
                && entry.adjustment.is_finite()
                && character_ids.encode(&to_katakana(&entry.ruby)).is_some()
        })
}

fn path_reading(path: &[DictionaryEntry]) -> String {
    to_katakana(
        &path
            .iter()
            .map(|entry| entry.ruby.as_str())
            .collect::<String>(),
    )
}

fn same_path(left: &[DictionaryEntry], right: &[DictionaryEntry]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.word == right.word
                && left.ruby == right.ruby
                && left.left_id == right.left_id
                && left.right_id == right.right_id
                && left.meaning_id == right.meaning_id
        })
}

fn recalled_entry(entry: &DictionaryEntry) -> DictionaryEntry {
    let mut entry = entry.clone();
    entry.metadata.insert(DictionaryMetadata::LEARNED);
    entry
}
