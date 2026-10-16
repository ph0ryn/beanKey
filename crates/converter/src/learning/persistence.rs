use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{CharacterIdMap, DictionaryEntry, DictionaryMetadata};

use super::policy::ComparisonRecord;
use super::{
    CanonicalStore, LearningError, MAX_RECORDS, RecallRecord, Runtime, same_path, valid_path,
};

pub(super) const STORE_FILE: &str = "personalization-v1.json";
pub(super) const TEMPORARY_FILE: &str = ".personalization-v1.json.tmp";
const VERSION: u32 = 1;
const MAX_FILE_BYTES: usize = 32 * 1_024 * 1_024;
const COALESCE_DELAY: Duration = Duration::from_millis(100);

// The public dictionary representation intentionally has no serialization
// dependency. This DTO preserves every currently defined metadata flag and,
// critically, both original cost fields for every node in an original path.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEntry {
    word: String,
    ruby: String,
    left_id: u16,
    right_id: u16,
    meaning_id: u16,
    base_value: f32,
    adjustment: f32,
    learned: bool,
    user_dictionary: bool,
}

impl From<&DictionaryEntry> for StoredEntry {
    fn from(entry: &DictionaryEntry) -> Self {
        Self {
            word: entry.word.clone(),
            ruby: entry.ruby.clone(),
            left_id: entry.left_id,
            right_id: entry.right_id,
            meaning_id: entry.meaning_id,
            base_value: entry.base_value,
            adjustment: entry.adjustment,
            learned: entry.metadata.contains(DictionaryMetadata::LEARNED),
            user_dictionary: entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY),
        }
    }
}

impl From<StoredEntry> for DictionaryEntry {
    fn from(entry: StoredEntry) -> Self {
        let mut metadata = DictionaryMetadata::default();
        if entry.learned {
            metadata.insert(DictionaryMetadata::LEARNED);
        }
        if entry.user_dictionary {
            metadata.insert(DictionaryMetadata::USER_DICTIONARY);
        }
        Self {
            word: entry.word,
            ruby: entry.ruby,
            left_id: entry.left_id,
            right_id: entry.right_id,
            meaning_id: entry.meaning_id,
            base_value: entry.base_value,
            adjustment: entry.adjustment,
            metadata,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredMemory {
    version: u32,
    recall: Vec<Vec<StoredEntry>>,
    comparisons: Vec<ComparisonRecord>,
}

pub(super) fn load(
    directory: &Path,
    character_ids: &CharacterIdMap,
) -> Result<CanonicalStore, LearningError> {
    let path = directory.join(STORE_FILE);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CanonicalStore::default());
        }
        Err(source) => return Err(LearningError::Io { path, source }),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| LearningError::Io {
            path: path.clone(),
            source,
        })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(LearningError::InvalidData(
            "personalization file exceeds byte limit",
        ));
    }
    let stored: StoredMemory = serde_json::from_slice(&bytes)
        .map_err(|_| LearningError::InvalidData("invalid personalization JSON"))?;
    if stored.version != VERSION {
        return Err(LearningError::InvalidData(
            "unsupported personalization version",
        ));
    }
    if stored.recall.len() > MAX_RECORDS || stored.comparisons.len() > MAX_RECORDS {
        return Err(LearningError::InvalidData(
            "personalization record count exceeds limit",
        ));
    }
    let mut store = CanonicalStore::default();
    for path in stored.recall {
        let path: Vec<DictionaryEntry> = path.into_iter().map(Into::into).collect();
        if !valid_path(&path, character_ids) {
            continue;
        }
        // Canonicalize duplicate input rather than treating repeated JSON entries
        // as multiple recall or comparison observations.
        if let Some(index) = store
            .recall
            .iter()
            .position(|record| same_path(&record.path, &path))
        {
            store.recall.remove(index);
        }
        store.recall.push(RecallRecord::new(path));
    }
    for comparison in stored.comparisons {
        if !comparison.valid() {
            continue;
        }
        if let Some(index) = store
            .comparisons
            .iter()
            .position(|record| record.same_pair(&comparison))
        {
            store.comparisons.remove(index);
        }
        store.comparisons.push(comparison);
    }
    Ok(store)
}

fn encode(store: &CanonicalStore) -> Result<Vec<u8>, LearningError> {
    let stored = StoredMemory {
        version: VERSION,
        recall: store
            .recall
            .iter()
            .map(|record| record.path.iter().map(StoredEntry::from).collect())
            .collect(),
        comparisons: store.comparisons.clone(),
    };
    let bytes = serde_json::to_vec(&stored)
        .map_err(|_| LearningError::InvalidData("cannot serialize personalization"))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(LearningError::InvalidData(
            "serialized personalization exceeds byte limit",
        ));
    }
    Ok(bytes)
}

/// Only the single writer calls this. A failure before publication leaves the
/// previous complete file unchanged. A failed directory fsync after publication
/// still leaves a complete atomic snapshot, never partially written JSON.
fn save(directory: &Path, store: &CanonicalStore) -> Result<(), LearningError> {
    let bytes = encode(store)?;
    let temporary = directory.join(TEMPORARY_FILE);
    let destination = directory.join(STORE_FILE);
    // Do not follow a stale temporary symlink. An abandoned temporary file is
    // disposable, never a recovery source that could undo forget/reset.
    match fs::symlink_metadata(&temporary) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            return Err(LearningError::Io {
                path: temporary,
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "temporary path is a directory",
                ),
            });
        }
        Ok(_) => fs::remove_file(&temporary).map_err(|source| LearningError::Io {
            path: temporary.clone(),
            source,
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(LearningError::Io {
                path: temporary,
                source,
            });
        }
    }
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| LearningError::Io {
                path: temporary.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|source| LearningError::Io {
                path: temporary.clone(),
                source,
            })?;
        fs::rename(&temporary, &destination).map_err(|source| LearningError::Io {
            path: destination,
            source,
        })?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| LearningError::Io {
                path: directory.to_owned(),
                source,
            })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// io::Error is not Clone; barriers must be able to report the last failed
/// attempt to every waiter without moving it out of the shared state.
#[derive(Clone)]
pub(super) enum SaveFailure {
    Io {
        path: PathBuf,
        kind: io::ErrorKind,
        message: String,
    },
    InvalidData(&'static str),
    Poisoned,
}

impl SaveFailure {
    fn from_error(error: LearningError) -> Self {
        match error {
            LearningError::Io { path, source } => Self::Io {
                path,
                kind: source.kind(),
                message: source.to_string(),
            },
            LearningError::InvalidData(reason) => Self::InvalidData(reason),
            LearningError::Poisoned => Self::Poisoned,
        }
    }

    pub(super) fn to_error(&self) -> LearningError {
        match self {
            Self::Io {
                path,
                kind,
                message,
            } => LearningError::Io {
                path: path.clone(),
                source: io::Error::new(*kind, message.clone()),
            },
            Self::InvalidData(reason) => LearningError::InvalidData(reason),
            Self::Poisoned => LearningError::Poisoned,
        }
    }
}

pub(super) fn spawn(runtime: Arc<Runtime>) -> Result<JoinHandle<()>, LearningError> {
    let path = runtime
        .state
        .lock()
        .map_err(|_| LearningError::Poisoned)?
        .directory
        .clone();
    thread::Builder::new()
        .name("beankey-personalization".into())
        .spawn(move || {
            // An unexpected worker panic must wake durable barriers instead of
            // leaving a session permanently blocked waiting for a missing writer.
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| writer_loop(&runtime)));
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.writer_stopped = true;
            runtime.changed.notify_all();
        })
        .map_err(|source| LearningError::Io { path, source })
}

fn writer_loop(runtime: &Runtime) {
    loop {
        let (directory, snapshot, revision, request) = {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !state.pending {
                if state.stopping {
                    return;
                }
                state = runtime
                    .changed
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            // One bounded coalescing window, not an unbounded queue or debounce
            // that can be postponed forever by continual input.
            let deadline = Instant::now() + COALESCE_DELAY;
            while !state.flush_requested && !state.stopping {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                (state, _) = runtime
                    .changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|error| error.into_inner());
            }
            state.pending = false;
            state.flush_requested = false;
            (
                state.directory.clone(),
                state.store.clone(),
                state.revision,
                state.requested_save,
            )
        };
        #[cfg(test)]
        {
            let gate = runtime.write_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.captured.wait();
                gate.proceed.wait();
            }
        }
        // No state mutex, filesystem I/O, or serialization on input-path updates.
        // Only this thread publishes snapshots, in monotonically increasing
        // revision order; a barrier waits until its revision is durable.
        let result = save(&directory, &snapshot);
        let mut state = runtime
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.completed_save = state.completed_save.max(request);
        match result {
            Ok(()) => {
                state.durable_revision = state.durable_revision.max(revision);
                state.last_error = None;
                if state.revision == revision {
                    // A barrier may have requested the already in-flight
                    // revision. Do not publish a redundant stale snapshot after
                    // that barrier (or a read-only mode switch) has returned.
                    state.pending = false;
                    state.flush_requested = false;
                }
            }
            Err(error) => state.last_error = Some(SaveFailure::from_error(error)),
        }
        runtime.changed.notify_all();
        // Failed writes remain dirty, but do not spin/retry until another input,
        // explicit barrier, or the one final drop-flush attempt requests a save.
    }
}
