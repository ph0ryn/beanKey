use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use unicode_segmentation::UnicodeSegmentation;

use crate::lattice::is_clause;
use crate::{
    Candidate, CharacterIdMap, DictionaryEntry, DictionaryMetadata, Louds,
    PostCompositionPrediction, PostPredictionKind, parse_entry_shard,
};

const LOUDS_FILE: &str = "memory.louds";
const LOUDS_CHARS_FILE: &str = "memory.loudschars2";
const METADATA_FILE: &str = "memory.memorymetadata";
const PAUSE_FILE: &str = ".pause";
const SHARD_SIZE: usize = 2_048;
const METADATA_ELEMENT_SIZE: usize = 6;

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

#[derive(Clone)]
pub struct LearningMemory {
    inner: Arc<Mutex<LearningState>>,
}

struct LearningState {
    directory: PathBuf,
    character_ids: CharacterIdMap,
    mode: LearningMode,
    max_count: usize,
    today: u16,
    persisted: Vec<LearningRecord>,
    temporary: Vec<LearningRecord>,
}

#[derive(Clone)]
struct LearningRecord {
    entry: DictionaryEntry,
    last_used_day: u16,
    last_updated_day: u16,
    count: u8,
}

impl LearningMemory {
    pub fn open(
        directory: impl Into<PathBuf>,
        mode: LearningMode,
        max_count: usize,
        character_ids: CharacterIdMap,
    ) -> Result<Self, LearningError> {
        Self::open_on_day(
            directory.into(),
            mode,
            max_count,
            character_ids,
            current_day(),
        )
    }

    fn open_on_day(
        directory: PathBuf,
        mode: LearningMode,
        max_count: usize,
        character_ids: CharacterIdMap,
        today: u16,
    ) -> Result<Self, LearningError> {
        fs::create_dir_all(&directory).map_err(|source| LearningError::Io {
            path: directory.clone(),
            source,
        })?;
        let mut persisted = if mode.uses_memory() {
            recover_if_paused(&directory)?;
            decode_directory(&directory)?
        } else {
            Vec::new()
        };
        if mode.uses_memory() {
            persisted.retain(|record| valid_learning_entry(&record.entry, &character_ids));
            decay(&mut persisted, today);
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(LearningState {
                directory,
                character_ids,
                mode,
                max_count,
                today,
                persisted,
                temporary: Vec::new(),
            })),
        })
    }

    pub fn entries(&self) -> Result<Vec<DictionaryEntry>, LearningError> {
        let state = self.lock()?;
        if !state.mode.uses_memory() {
            return Ok(Vec::new());
        }
        let mut output = state
            .persisted
            .iter()
            .chain(&state.temporary)
            .map(learned_entry)
            .collect::<Vec<_>>();
        deduplicate_entries(&mut output);
        Ok(output)
    }

    pub fn learn(&self, candidate: &Candidate) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        let today = state.today;
        let character_ids = state.character_ids.clone();
        for entry in candidate
            .entries
            .iter()
            .filter(|entry| learns_individual_word(entry))
        {
            memorize(&mut state.temporary, entry.clone(), today, &character_ids);
        }
        for entry in learned_clause_entries(&candidate.entries) {
            memorize(&mut state.temporary, entry, today, &character_ids);
        }
        if candidate.entries.len() > 1 {
            memorize(
                &mut state.temporary,
                join_entries(&candidate.entries),
                today,
                &character_ids,
            );
        }
        Ok(())
    }

    pub fn learn_post_prediction(
        &self,
        candidate: &Candidate,
        prediction: &PostCompositionPrediction,
    ) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        let (mut prefix, update) = match &prediction.kind {
            PostPredictionKind::Additional { entries } => {
                (candidate.entries.clone(), entries.clone())
            }
            PostPredictionKind::Replacement {
                target_entries,
                replacement_entries,
            } => {
                let prefix_count = candidate.entries.len().saturating_sub(target_entries.len());
                (
                    candidate.entries[..prefix_count].to_vec(),
                    replacement_entries.clone(),
                )
            }
        };
        let update_start = prefix.len();
        prefix.extend(update.iter().cloned());
        let today = state.today;
        let character_ids = state.character_ids.clone();
        for entry in update.iter().filter(|entry| learns_individual_word(entry)) {
            memorize(&mut state.temporary, entry.clone(), today, &character_ids);
        }
        for entry in learned_clause_entries_after(&prefix, update_start) {
            memorize(&mut state.temporary, entry, today, &character_ids);
        }
        if prefix.len() > 1 {
            memorize(
                &mut state.temporary,
                join_entries(&prefix),
                today,
                &character_ids,
            );
        }
        Ok(())
    }

    pub fn forget(&self, candidate: &Candidate) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() {
            return Ok(());
        }
        let temporary_targets: HashSet<_> = candidate
            .entries
            .iter()
            .map(|entry| (entry.ruby.clone(), entry.word.clone()))
            .collect();
        let persistent_targets: HashSet<_> = candidate
            .entries
            .iter()
            .map(|entry| entry.word.clone())
            .collect();
        state.temporary.retain(|record| {
            !temporary_targets.contains(&(record.entry.ruby.clone(), record.entry.word.clone()))
        });
        state
            .persisted
            .retain(|record| !persistent_targets.contains(&record.entry.word));
        let mut persisted = state.persisted.clone();
        save_records(&state, &mut persisted)?;
        state.persisted = persisted;
        Ok(())
    }

    pub fn commit(&self) -> Result<bool, LearningError> {
        let mut state = self.lock()?;
        if !state.mode.updates_memory() || state.temporary.is_empty() {
            return Ok(false);
        }
        let mut persisted = state.persisted.clone();
        for record in &state.temporary {
            merge_record(&mut persisted, record.clone());
        }
        save_records(&state, &mut persisted)?;
        state.persisted = persisted;
        state.temporary.clear();
        Ok(true)
    }

    pub fn reset(&self) -> Result<(), LearningError> {
        let mut state = self.lock()?;
        state.persisted.clear();
        state.temporary.clear();
        for entry in read_directory(&state.directory)? {
            let path = entry.path();
            let name = entry.file_name();
            if is_memory_file(&name.to_string_lossy()) {
                remove_file_if_present(&path)?;
            }
        }
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, LearningState>, LearningError> {
        self.inner.lock().map_err(|_| LearningError::Poisoned)
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

fn learned_clause_entries(entries: &[DictionaryEntry]) -> Vec<DictionaryEntry> {
    learned_clause_ranges(entries)
        .windows(2)
        .map(|pair| join_entries(&entries[pair[0].0..pair[1].1]))
        .collect()
}

fn learned_clause_entries_after(
    entries: &[DictionaryEntry],
    update_start: usize,
) -> Vec<DictionaryEntry> {
    learned_clause_ranges(entries)
        .windows(2)
        .filter(|pair| pair[1].1 > update_start)
        .map(|pair| join_entries(&entries[pair[0].0..pair[1].1]))
        .collect()
}

fn learned_clause_ranges(entries: &[DictionaryEntry]) -> Vec<(usize, usize)> {
    let mut clauses: Vec<Vec<DictionaryEntry>> = Vec::new();
    let mut ranges = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if clauses.last().is_none_or(|clause| {
            clause.last().is_some_and(|previous| {
                is_clause(usize::from(previous.right_id), usize::from(entry.left_id))
            })
        }) {
            clauses.push(vec![entry.clone()]);
            ranges.push((index, index + 1));
        } else if let Some(clause) = clauses.last_mut() {
            clause.push(entry.clone());
            if let Some((_, end)) = ranges.last_mut() {
                *end = index + 1;
            }
        }
    }
    ranges
}

fn join_entries(entries: &[DictionaryEntry]) -> DictionaryEntry {
    let first = entries.first().expect("joined entries are nonempty");
    let last = entries.last().expect("joined entries are nonempty");
    DictionaryEntry {
        word: entries.iter().map(|entry| entry.word.as_str()).collect(),
        ruby: entries.iter().map(|entry| entry.ruby.as_str()).collect(),
        left_id: first.left_id,
        right_id: last.right_id,
        meaning_id: last.meaning_id,
        base_value: entries.iter().map(|entry| entry.base_value).sum(),
        adjustment: 0.0,
        metadata: DictionaryMetadata::default(),
    }
}

fn memorize(
    records: &mut Vec<LearningRecord>,
    entry: DictionaryEntry,
    today: u16,
    character_ids: &CharacterIdMap,
) {
    if !valid_learning_entry(&entry, character_ids) {
        return;
    }
    if let Some(record) = records
        .iter_mut()
        .find(|record| same_entry(&record.entry, &entry))
    {
        record.count = record.count.saturating_add(1);
        record.last_used_day = today;
        record.entry = entry;
    } else {
        records.push(LearningRecord {
            entry,
            last_used_day: today,
            last_updated_day: today,
            count: 1,
        });
    }
}

fn valid_learning_entry(entry: &DictionaryEntry, character_ids: &CharacterIdMap) -> bool {
    !entry.word.chars().any(char::is_control)
        && !entry.ruby.is_empty()
        && !entry.ruby.chars().any(char::is_control)
        && character_ids.encode(&entry.ruby).is_some()
}

fn merge_record(records: &mut Vec<LearningRecord>, incoming: LearningRecord) {
    if let Some(record) = records
        .iter_mut()
        .find(|record| same_entry(&record.entry, &incoming.entry))
    {
        record.count = record.count.saturating_add(incoming.count);
        record.last_used_day = record.last_used_day.max(incoming.last_used_day);
        record.last_updated_day = record.last_updated_day.max(incoming.last_updated_day);
        record.entry = incoming.entry;
    } else {
        records.push(incoming);
    }
}

fn same_entry(left: &DictionaryEntry, right: &DictionaryEntry) -> bool {
    left.ruby == right.ruby
        && left.word == right.word
        && left.left_id == right.left_id
        && left.right_id == right.right_id
}

fn learned_entry(record: &LearningRecord) -> DictionaryEntry {
    let mut entry = record.entry.clone();
    let learned_value = learning_value(record.count, &entry.ruby);
    entry.adjustment = learned_value - entry.base_value;
    entry.metadata.insert(DictionaryMetadata::LEARNED);
    entry
}

fn learning_value(count: u8, ruby: &str) -> f32 {
    let length = UnicodeSegmentation::graphemes(ruby, true).count().max(1) as f64;
    let distance = 1.0 - f64::from(count) / 255.0;
    (-1.0 - 4.0 / length - 3.0 * distance.powi(3)) as f32
}

fn decay(records: &mut Vec<LearningRecord>, today: u16) {
    records.retain_mut(|record| {
        if today < record.last_used_day || today < record.last_updated_day {
            record.last_used_day = today;
            record.last_updated_day = today;
            record.count = 1;
        }
        if today.saturating_sub(record.last_used_day) >= 128 {
            return false;
        }
        while today.saturating_sub(record.last_updated_day) > 32 {
            record.count >>= 1;
            record.last_updated_day = record.last_updated_day.saturating_add(32);
        }
        record.count > 0
    });
}

fn deduplicate_entries(entries: &mut Vec<DictionaryEntry>) {
    let mut output = Vec::new();
    for entry in std::mem::take(entries) {
        if let Some(existing) = output
            .iter_mut()
            .find(|existing| same_entry(existing, &entry))
        {
            if existing.value() < entry.value() {
                *existing = entry;
            }
        } else {
            output.push(entry);
        }
    }
    *entries = output;
}

fn save_records(
    state: &LearningState,
    records: &mut Vec<LearningRecord>,
) -> Result<(), LearningError> {
    decay(records, state.today);
    records.sort_by_key(|record| std::cmp::Reverse(record.last_used_day));
    records.truncate(state.max_count);
    let files = encode_files(records, &state.character_ids);
    write_temporary(&state.directory, LOUDS_FILE, &files.louds)?;
    write_temporary(&state.directory, LOUDS_CHARS_FILE, &files.characters)?;
    write_temporary(&state.directory, METADATA_FILE, &files.metadata)?;
    for (index, shard) in files.shards.iter().enumerate() {
        write_temporary(&state.directory, &shard_file(index), shard)?;
    }
    let pause = state.directory.join(PAUSE_FILE);
    fs::write(&pause, []).map_err(|source| LearningError::Io {
        path: pause.clone(),
        source,
    })?;
    restore_temporary(&state.directory, LOUDS_CHARS_FILE)?;
    restore_temporary(&state.directory, METADATA_FILE)?;
    for index in 0..files.shards.len() {
        restore_temporary(&state.directory, &shard_file(index))?;
    }
    // The LOUDS topology is the first file read by upstream, so publish it last.
    restore_temporary(&state.directory, LOUDS_FILE)?;
    remove_file_if_present(&pause)?;
    Ok(())
}

fn recover_if_paused(directory: &Path) -> Result<(), LearningError> {
    let pause = directory.join(PAUSE_FILE);
    if !pause.exists() {
        return Ok(());
    }
    restore_temporary(directory, LOUDS_CHARS_FILE)?;
    restore_temporary(directory, METADATA_FILE)?;
    let mut shards = read_directory(directory)?
        .into_iter()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("memory") && name.ends_with(".loudstxt3.2"))
        .collect::<Vec<_>>();
    shards.sort();
    for temporary in shards {
        restore_temporary(directory, temporary.trim_end_matches(".2"))?;
    }
    restore_temporary(directory, LOUDS_FILE)?;
    remove_file_if_present(&pause)?;
    Ok(())
}

struct EncodedMemory {
    louds: Vec<u8>,
    characters: Vec<u8>,
    metadata: Vec<u8>,
    shards: Vec<Vec<u8>>,
}

#[derive(Default)]
struct MemoryTrieNode {
    children: BTreeMap<u8, usize>,
    record_indices: Vec<usize>,
}

fn encode_files(records: &[LearningRecord], character_ids: &CharacterIdMap) -> EncodedMemory {
    let mut nodes = vec![MemoryTrieNode::default()];
    for (record_index, record) in records.iter().enumerate() {
        let Some(characters) = character_ids.encode(&record.entry.ruby) else {
            continue;
        };
        let mut node_index = 0;
        for character in characters {
            let next = if let Some(next) = nodes[node_index].children.get(&character) {
                *next
            } else {
                let next = nodes.len();
                nodes.push(MemoryTrieNode::default());
                nodes[node_index].children.insert(character, next);
                next
            };
            node_index = next;
        }
        if nodes[node_index].record_indices.len() < u8::MAX as usize {
            nodes[node_index].record_indices.push(record_index);
        }
    }

    let mut bits = vec![true, false];
    let mut characters = vec![0, 0];
    let mut blocks = vec![Vec::new(), Vec::new()];
    let mut metadata_blocks = vec![Vec::new(), Vec::new()];
    let mut current = nodes[0]
        .children
        .iter()
        .map(|(character, index)| (*character, *index))
        .collect::<Vec<_>>();
    bits.extend(std::iter::repeat_n(true, current.len()));
    bits.push(false);
    while !current.is_empty() {
        for (character, node_index) in &current {
            characters.push(*character);
            blocks.push(nodes[*node_index].record_indices.clone());
            metadata_blocks.push(nodes[*node_index].record_indices.clone());
            bits.extend(std::iter::repeat_n(true, nodes[*node_index].children.len()));
            bits.push(false);
        }
        current = current
            .into_iter()
            .flat_map(|(_, node_index)| {
                nodes[node_index]
                    .children
                    .iter()
                    .map(|(character, index)| (*character, *index))
                    .collect::<Vec<_>>()
            })
            .collect();
    }

    let mut metadata = u32::try_from(metadata_blocks.len())
        .unwrap_or(u32::MAX)
        .to_le_bytes()
        .to_vec();
    for block in metadata_blocks {
        metadata.push(u8::try_from(block.len()).unwrap_or(u8::MAX));
        for index in block {
            let record = &records[index];
            metadata.extend(record.last_used_day.to_le_bytes());
            metadata.extend(record.last_updated_day.to_le_bytes());
            metadata.push(record.count);
            // Swift's MetadataElement has two-byte alignment and a one-byte tail pad.
            metadata.push(0);
        }
    }

    let shards = blocks
        .chunks(SHARD_SIZE)
        .map(|blocks| encode_entry_shard(blocks, records))
        .collect();
    EncodedMemory {
        louds: encode_louds(&bits),
        characters,
        metadata,
        shards,
    }
}

fn encode_louds(bits: &[bool]) -> Vec<u8> {
    let mut output = Vec::new();
    for chunk in bits.chunks(64) {
        let mut word = 0_u64;
        for (index, value) in chunk.iter().enumerate() {
            if *value {
                word |= 1 << (63 - index);
            }
        }
        for index in chunk.len()..64 {
            word |= 1 << (63 - index);
        }
        output.extend(word.to_le_bytes());
    }
    output
}

fn encode_entry_shard(blocks: &[Vec<usize>], records: &[LearningRecord]) -> Vec<u8> {
    let payloads = blocks
        .iter()
        .map(|indices| encode_entry_block(indices, records))
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    output.extend(
        u16::try_from(payloads.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    let mut offset = 2 + payloads.len() * 4;
    for payload in &payloads {
        output.extend(u32::try_from(offset).unwrap_or(u32::MAX).to_le_bytes());
        offset = offset.saturating_add(payload.len());
    }
    for payload in payloads {
        output.extend(payload);
    }
    output
}

fn encode_entry_block(indices: &[usize], records: &[LearningRecord]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend(
        u16::try_from(indices.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    for index in indices {
        let record = &records[*index];
        output.extend(record.entry.left_id.to_le_bytes());
        output.extend(record.entry.right_id.to_le_bytes());
        output.extend(record.entry.meaning_id.to_le_bytes());
        output.extend(learning_value(record.count, &record.entry.ruby).to_le_bytes());
    }
    let ruby = indices
        .first()
        .map(|index| records[*index].entry.ruby.as_str())
        .unwrap_or("");
    output.extend(ruby.as_bytes());
    for index in indices {
        output.push(b'\t');
        let word = &records[*index].entry.word;
        if word != ruby {
            output.extend(word.as_bytes());
        }
    }
    output
}

fn decode_directory(directory: &Path) -> Result<Vec<LearningRecord>, LearningError> {
    let Some(metadata) = read_optional(&directory.join(METADATA_FILE))? else {
        return Ok(Vec::new());
    };
    let louds_bytes = read_required(&directory.join(LOUDS_FILE))?;
    let characters = read_required(&directory.join(LOUDS_CHARS_FILE))?;
    let louds = Louds::parse(&louds_bytes, &characters)
        .map_err(|_| LearningError::InvalidData("invalid LOUDS topology"))?;
    let mut cursor = 0;
    let node_count = read_u32(&metadata, &mut cursor)? as usize;
    if node_count != characters.len() || node_count != louds.node_count() {
        return Err(LearningError::InvalidData("inconsistent node counts"));
    }
    let mut metadata_blocks = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let count = *metadata
            .get(cursor)
            .ok_or(LearningError::InvalidData("truncated metadata block"))?
            as usize;
        cursor += 1;
        let mut block = Vec::with_capacity(count);
        for _ in 0..count {
            let last_used_day = read_u16(&metadata, &mut cursor)?;
            let last_updated_day = read_u16(&metadata, &mut cursor)?;
            let count = *metadata
                .get(cursor)
                .ok_or(LearningError::InvalidData("truncated metadata count"))?;
            cursor += METADATA_ELEMENT_SIZE - 4;
            block.push((last_used_day, last_updated_day, count));
        }
        metadata_blocks.push(block);
    }
    if cursor != metadata.len() {
        return Err(LearningError::InvalidData("trailing metadata bytes"));
    }

    let mut records = Vec::new();
    let mut loaded_shard = usize::MAX;
    let mut shard = Vec::new();
    for (node, block) in metadata_blocks.into_iter().enumerate() {
        let shard_index = node / SHARD_SIZE;
        if shard_index != loaded_shard {
            shard = read_required(&directory.join(shard_file(shard_index)))?;
            loaded_shard = shard_index;
        }
        let entries = parse_entry_shard(&shard, [node % SHARD_SIZE])
            .map_err(|_| LearningError::InvalidData("invalid learning entry shard"))?;
        if entries.len() != block.len() {
            return Err(LearningError::InvalidData(
                "entry and metadata counts differ",
            ));
        }
        records.extend(entries.into_iter().zip(block).map(
            |(entry, (last_used_day, last_updated_day, count))| LearningRecord {
                entry,
                last_used_day,
                last_updated_day,
                count,
            },
        ));
    }
    Ok(records)
}

fn write_temporary(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), LearningError> {
    let path = directory.join(format!("{name}.2"));
    fs::write(&path, bytes).map_err(|source| LearningError::Io { path, source })
}

fn restore_temporary(directory: &Path, name: &str) -> Result<(), LearningError> {
    let source_path = directory.join(format!("{name}.2"));
    let destination = directory.join(name);
    fs::copy(&source_path, &destination)
        .map(|_| ())
        .map_err(|source| LearningError::Io {
            path: destination,
            source,
        })
}

fn shard_file(index: usize) -> String {
    format!("memory{index}.loudstxt3")
}

fn read_required(path: &Path) -> Result<Vec<u8>, LearningError> {
    fs::read(path).map_err(|source| LearningError::Io {
        path: path.to_owned(),
        source,
    })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, LearningError> {
    match fs::read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LearningError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn read_directory(path: &Path) -> Result<Vec<fs::DirEntry>, LearningError> {
    fs::read_dir(path)
        .and_then(Iterator::collect)
        .map_err(|source| LearningError::Io {
            path: path.to_owned(),
            source,
        })
}

fn remove_file_if_present(path: &Path) -> Result<(), LearningError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LearningError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn is_memory_file(name: &str) -> bool {
    name == PAUSE_FILE
        || name == "memory.bin"
        || name == "memory.bin.2"
        || name.ends_with(".loudstxt3")
        || name.ends_with(".loudstxt3.2")
        || name.ends_with(".loudschars2")
        || name.ends_with(".loudschars2.2")
        || name.ends_with(".memorymetadata")
        || name.ends_with(".memorymetadata.2")
        || name.ends_with(".louds")
        || name.ends_with(".louds.2")
        || name.ends_with("learningMemory.txt")
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, LearningError> {
    Ok(u16::from_le_bytes(read_array(bytes, cursor)?))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, LearningError> {
    Ok(u32::from_le_bytes(read_array(bytes, cursor)?))
}

fn read_array<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], LearningError> {
    let value = bytes
        .get(*cursor..cursor.saturating_add(N))
        .ok_or(LearningError::InvalidData("truncated numeric field"))?;
    *cursor += N;
    value
        .try_into()
        .map_err(|_| LearningError::InvalidData("invalid numeric field"))
}

fn current_day() -> u16 {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    u16::try_from(days.saturating_sub(19_000)).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character_ids() -> CharacterIdMap {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("data/azooKey_dictionary_storage/Dictionary/louds/charID.chid");
        CharacterIdMap::parse(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bean-key-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_encoded_files(directory: &Path, files: &EncodedMemory) {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join(LOUDS_FILE), &files.louds).unwrap();
        fs::write(directory.join(LOUDS_CHARS_FILE), &files.characters).unwrap();
        fs::write(directory.join(METADATA_FILE), &files.metadata).unwrap();
        for (index, shard) in files.shards.iter().enumerate() {
            fs::write(directory.join(shard_file(index)), shard).unwrap();
        }
    }

    fn memory(mode: LearningMode, persisted: Vec<LearningRecord>) -> LearningMemory {
        LearningMemory {
            inner: Arc::new(Mutex::new(LearningState {
                directory: PathBuf::new(),
                character_ids: character_ids(),
                mode,
                max_count: 128,
                today: 10,
                persisted,
                temporary: Vec::new(),
            })),
        }
    }

    fn entry(word: &str, ruby: &str) -> DictionaryEntry {
        DictionaryEntry {
            word: word.into(),
            ruby: ruby.into(),
            left_id: 1285,
            right_id: 1285,
            meaning_id: 501,
            base_value: -12.0,
            adjustment: 0.0,
            metadata: DictionaryMetadata::default(),
        }
    }

    #[test]
    fn halves_counts_after_each_complete_thirty_two_day_period() {
        let mut records = vec![LearningRecord {
            entry: entry("語", "ゴ"),
            last_used_day: 100,
            last_updated_day: 100,
            count: 4,
        }];
        decay(&mut records, 165);
        assert_eq!(records[0].count, 1);
        decay(&mut records, 197);
        assert!(records.is_empty());
    }

    #[test]
    fn round_trips_the_upstream_persistence_format() {
        let records = vec![LearningRecord {
            entry: entry("単語", "タンゴ"),
            last_used_day: 10,
            last_updated_day: 9,
            count: 3,
        }];
        let directory = temporary_directory("learning-round-trip");
        let files = encode_files(&records, &character_ids());
        write_encoded_files(&directory, &files);
        let decoded = decode_directory(&directory).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(decoded[0].entry.word, "単語");
        assert_eq!(decoded[0].count, 3);
        assert_eq!(
            files.metadata.len(),
            4 + files.characters.len() + METADATA_ELEMENT_SIZE
        );
        assert_eq!(
            &files.metadata[..4],
            &u32::try_from(files.characters.len()).unwrap().to_le_bytes()
        );
        assert_eq!(
            &files.metadata[files.metadata.len() - METADATA_ELEMENT_SIZE..],
            &[10, 0, 9, 0, 3, 0]
        );
        assert!(Louds::parse(&files.louds, &files.characters).is_ok());
    }

    #[test]
    fn honors_read_only_and_disabled_learning_modes() {
        let persisted = LearningRecord {
            entry: entry("既存", "キソン"),
            last_used_day: 10,
            last_updated_day: 10,
            count: 1,
        };
        let candidate = Candidate::single(
            "新規".into(),
            -12.0,
            crate::ComposingCount::Surface(2),
            501,
            vec![entry("新規", "シンキ")],
        );
        let read_only = memory(LearningMode::OnlyOutput, vec![persisted.clone()]);
        read_only.learn(&candidate).unwrap();
        assert!(!read_only.commit().unwrap());
        assert_eq!(read_only.entries().unwrap().len(), 1);

        let disabled = memory(LearningMode::Nothing, vec![persisted]);
        disabled.learn(&candidate).unwrap();
        assert!(!disabled.commit().unwrap());
        assert!(disabled.entries().unwrap().is_empty());
    }

    #[test]
    fn commit_without_pending_learning_does_not_create_memory_files() {
        let directory = temporary_directory("learning-empty-commit");
        let memory = LearningMemory::open(
            &directory,
            LearningMode::InputAndOutput,
            128,
            character_ids(),
        )
        .unwrap();

        assert!(!memory.commit().unwrap());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opening_memory_recovers_and_clears_an_upstream_pause_marker() {
        let directory = temporary_directory("learning-pause-recovery");
        let memory = LearningMemory::open(
            &directory,
            LearningMode::InputAndOutput,
            128,
            character_ids(),
        )
        .unwrap();
        let candidate = Candidate::single(
            "テスト".into(),
            -10.0,
            crate::ComposingCount::Surface(3),
            501,
            vec![entry("テスト", "テスト")],
        );
        memory.learn(&candidate).unwrap();
        assert!(memory.commit().unwrap());
        fs::write(directory.join(PAUSE_FILE), []).unwrap();

        let recovered =
            LearningMemory::open(&directory, LearningMode::OnlyOutput, 128, character_ids())
                .unwrap();

        assert!(!directory.join(PAUSE_FILE).exists());
        assert!(
            recovered
                .entries()
                .unwrap()
                .iter()
                .any(|entry| entry.word == "テスト")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn forgets_a_persisted_surface_across_all_readings_and_parts_of_speech() {
        let directory = temporary_directory("coarse-forget");
        fs::create_dir_all(&directory).unwrap();
        let mut first = entry("表層", "ヒョウソウ");
        first.left_id = 1_285;
        first.right_id = 1_285;
        let mut second = entry("表層", "オモテソウ");
        second.left_id = 1_288;
        second.right_id = 1_288;
        let persisted = [first, second]
            .into_iter()
            .map(|entry| LearningRecord {
                entry,
                last_used_day: 10,
                last_updated_day: 10,
                count: 1,
            })
            .collect();
        let memory = LearningMemory {
            inner: Arc::new(Mutex::new(LearningState {
                directory: directory.clone(),
                character_ids: character_ids(),
                mode: LearningMode::InputAndOutput,
                max_count: 128,
                today: 10,
                persisted,
                temporary: Vec::new(),
            })),
        };
        let target = Candidate::single(
            "表層".into(),
            -12.0,
            crate::ComposingCount::Surface(4),
            501,
            vec![entry("表層", "ヒョウソウ")],
        );

        memory.forget(&target).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert!(
            memory
                .entries()
                .unwrap()
                .iter()
                .all(|entry| entry.word != "表層")
        );
    }

    #[test]
    fn disabled_learning_ignores_corrupt_persistence() {
        let directory = temporary_directory("disabled-learning");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(METADATA_FILE), b"corrupt").unwrap();

        let disabled =
            LearningMemory::open(&directory, LearningMode::Nothing, 128, character_ids()).unwrap();
        assert!(disabled.entries().unwrap().is_empty());
        disabled.reset().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ignores_control_text_in_existing_learning_memory() {
        let directory = temporary_directory("control-learning");
        let today = current_day();
        let records = [("正常", "セイジョウ"), ("\r", "\r"), ("語\t", "ゴ\t")]
            .into_iter()
            .map(|(word, ruby)| LearningRecord {
                entry: entry(word, ruby),
                last_used_day: today,
                last_updated_day: today,
                count: 1,
            })
            .collect::<Vec<_>>();
        write_encoded_files(&directory, &encode_files(&records, &character_ids()));

        let recovered =
            LearningMemory::open(&directory, LearningMode::OnlyOutput, 128, character_ids())
                .unwrap();
        let entries = recovered.entries().unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert!(entries.iter().any(|entry| entry.word == "正常"));
        assert!(entries.iter().all(|entry| {
            !entry.word.chars().any(char::is_control) && !entry.ruby.chars().any(char::is_control)
        }));
    }

    #[test]
    fn learns_only_the_new_part_of_a_post_composition_prediction() {
        let memory = memory(LearningMode::InputAndOutput, Vec::new());
        let base_entry = entry("今日", "キョウ");
        let base = Candidate::single(
            "今日".into(),
            -12.0,
            crate::ComposingCount::Surface(3),
            501,
            vec![base_entry],
        );
        let prediction = PostCompositionPrediction {
            text: "は".into(),
            value: -13.0,
            kind: PostPredictionKind::Additional {
                entries: vec![entry("は", "ハ")],
            },
            is_terminal: false,
        };

        memory.learn_post_prediction(&base, &prediction).unwrap();
        let learned = memory.entries().unwrap();

        assert!(learned.iter().any(|entry| entry.word == "は"));
        assert!(learned.iter().any(|entry| entry.word == "今日は"));
        assert!(!learned.iter().any(|entry| entry.word == "今日"));
    }
}
