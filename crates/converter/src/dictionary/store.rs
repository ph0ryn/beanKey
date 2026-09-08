use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use unicode_segmentation::UnicodeSegmentation;

use super::{
    CharacterIdMap, DictionaryBinaryError, DictionaryEntry, Louds, LoudsError, MeaningMatrix,
    escaped_identifier, parse_connection_cost_line, parse_entry_shard,
};

const SHARD_SHIFT: usize = 11;
const SHARD_MASK: usize = (1 << SHARD_SHIFT) - 1;
const DEFAULT_CONNECTION_COST: f32 = -25.0;

#[derive(Debug)]
pub enum DictionaryError {
    Io { path: PathBuf, source: io::Error },
    Binary(DictionaryBinaryError),
    Louds(LoudsError),
    Learning(crate::LearningError),
    Poisoned(&'static str),
}

impl fmt::Display for DictionaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Binary(error) => error.fmt(formatter),
            Self::Louds(error) => error.fmt(formatter),
            Self::Learning(error) => error.fmt(formatter),
            Self::Poisoned(name) => write!(formatter, "dictionary {name} cache is poisoned"),
        }
    }
}

impl Error for DictionaryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Binary(error) => Some(error),
            Self::Louds(error) => Some(error),
            Self::Learning(error) => Some(error),
            Self::Poisoned(_) => None,
        }
    }
}

impl From<DictionaryBinaryError> for DictionaryError {
    fn from(value: DictionaryBinaryError) -> Self {
        Self::Binary(value)
    }
}

impl From<LoudsError> for DictionaryError {
    fn from(value: LoudsError) -> Self {
        Self::Louds(value)
    }
}

impl From<crate::LearningError> for DictionaryError {
    fn from(value: crate::LearningError) -> Self {
        Self::Learning(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictionaryMatch {
    pub surface_end: usize,
    pub entries: Vec<DictionaryEntry>,
}

pub struct DictionaryStore {
    root: PathBuf,
    character_ids: CharacterIdMap,
    meaning_matrix: MeaningMatrix,
    louds: Mutex<HashMap<String, Option<Arc<Louds>>>>,
    shards: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    connection_lines: Mutex<HashMap<usize, Option<Arc<Vec<f32>>>>>,
}

impl DictionaryStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, DictionaryError> {
        let root = root.into();
        let character_path = root.join("louds/charID.chid");
        let character_ids = CharacterIdMap::parse(&read_to_string(&character_path)?)?;
        let meaning_path = root.join("mm.binary");
        let meaning_matrix = MeaningMatrix::parse(&read(&meaning_path)?)?;
        Ok(Self {
            root,
            character_ids,
            meaning_matrix,
            louds: Mutex::new(HashMap::new()),
            shards: Mutex::new(HashMap::new()),
            connection_lines: Mutex::new(HashMap::new()),
        })
    }

    pub fn character_ids(&self) -> &CharacterIdMap {
        &self.character_ids
    }

    pub fn exact_match(&self, ruby: &str) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        let Some((identifier, ids)) = self.lookup_key(ruby) else {
            return Ok(Vec::new());
        };
        let Some(louds) = self.load_louds(identifier)? else {
            return Ok(Vec::new());
        };
        let Some(node) = louds.search(&ids) else {
            return Ok(Vec::new());
        };
        self.load_entries(identifier, [node])
    }

    pub fn matches_from_start(
        &self,
        ruby: &str,
        maximum_length: usize,
    ) -> Result<Vec<DictionaryMatch>, DictionaryError> {
        let graphemes: Vec<_> = UnicodeSegmentation::graphemes(ruby, true).collect();
        let Some(identifier) = graphemes.first().copied() else {
            return Ok(Vec::new());
        };
        let Some(louds) = self.load_louds(identifier)? else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        let mut output = Vec::new();
        for (index, grapheme) in graphemes.into_iter().take(maximum_length).enumerate() {
            let Some(id) = self.character_ids.id(grapheme) else {
                break;
            };
            ids.push(id);
            let Some(node) = louds.search(&ids) else {
                break;
            };
            let entries = self.load_entries(identifier, [node])?;
            if !entries.is_empty() {
                output.push(DictionaryMatch {
                    surface_end: index + 1,
                    entries,
                });
            }
        }
        Ok(output)
    }

    pub fn entries_after_prefix(
        &self,
        ruby: &str,
        maximum_depth: usize,
        maximum_count: usize,
    ) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        let Some((identifier, ids)) = self.lookup_key(ruby) else {
            return Ok(Vec::new());
        };
        let Some(louds) = self.load_louds(identifier)? else {
            return Ok(Vec::new());
        };
        self.load_entries(
            identifier,
            louds.descendants(&ids, maximum_depth, maximum_count),
        )
    }

    pub fn connection_cost(&self, former: usize, latter: usize) -> Result<f32, DictionaryError> {
        if let Some(line) = self
            .lock(&self.connection_lines, "connection")?
            .get(&former)
        {
            return Ok(line
                .as_ref()
                .and_then(|line| line.get(latter))
                .copied()
                .unwrap_or(DEFAULT_CONNECTION_COST));
        }

        let path = self.root.join(format!("cb/{former}.binary"));
        let line = match fs::read(&path) {
            Ok(binary) => Some(Arc::new(parse_connection_cost_line(&binary)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(DictionaryError::Io { path, source }),
        };
        let value = line
            .as_ref()
            .and_then(|line| line.get(latter))
            .copied()
            .unwrap_or(DEFAULT_CONNECTION_COST);
        self.lock(&self.connection_lines, "connection")?
            .insert(former, line);
        Ok(value)
    }

    pub fn meaning_cost(&self, former: usize, latter: usize) -> Option<f32> {
        self.meaning_matrix.get(former, latter)
    }

    pub fn zero_hint_entries(
        &self,
        right_id: u16,
    ) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        let path = self.root.join(format!("p/pc_{right_id}.csv"));
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(DictionaryError::Io { path, source }),
        };
        Ok(content.lines().map(parse_prediction_entry).collect())
    }

    fn lookup_key<'a>(&self, ruby: &'a str) -> Option<(&'a str, Vec<u8>)> {
        let identifier = UnicodeSegmentation::graphemes(ruby, true).next()?;
        Some((identifier, self.character_ids.encode(ruby)?))
    }

    fn load_louds(&self, identifier: &str) -> Result<Option<Arc<Louds>>, DictionaryError> {
        if let Some(value) = self.lock(&self.louds, "LOUDS")?.get(identifier) {
            return Ok(value.clone());
        }
        let escaped = escaped_identifier(identifier);
        let bits_path = self.root.join(format!("louds/{escaped}.louds"));
        let chars_path = self.root.join(format!("louds/{escaped}.loudschars2"));
        let value = match (read_optional(&bits_path)?, read_optional(&chars_path)?) {
            (Some(bits), Some(chars)) => Some(Arc::new(Louds::parse(&bits, &chars)?)),
            _ => None,
        };
        self.lock(&self.louds, "LOUDS")?
            .insert(identifier.to_owned(), value.clone());
        Ok(value)
    }

    fn load_entries(
        &self,
        identifier: &str,
        nodes: impl IntoIterator<Item = usize>,
    ) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        let mut by_shard: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for node in nodes {
            by_shard
                .entry(node >> SHARD_SHIFT)
                .or_default()
                .push(node & SHARD_MASK);
        }
        let escaped = escaped_identifier(identifier);
        let mut output = Vec::new();
        for (shard, indices) in by_shard {
            let file = format!("{escaped}{shard}");
            let binary = if let Some(binary) = self.lock(&self.shards, "shard")?.get(&file) {
                Arc::clone(binary)
            } else {
                let path = self.root.join(format!("louds/{file}.loudstxt3"));
                let binary = Arc::new(read(&path)?);
                self.lock(&self.shards, "shard")?
                    .insert(file, Arc::clone(&binary));
                binary
            };
            output.extend(parse_entry_shard(&binary, indices)?);
        }
        Ok(output)
    }

    fn lock<'a, T>(
        &self,
        mutex: &'a Mutex<T>,
        name: &'static str,
    ) -> Result<MutexGuard<'a, T>, DictionaryError> {
        mutex.lock().map_err(|_| DictionaryError::Poisoned(name))
    }
}

fn parse_prediction_entry(line: &str) -> DictionaryEntry {
    let mut fields = line.split(',');
    let ruby = fields.next().unwrap_or_default().to_owned();
    let word = match fields.next().unwrap_or_default() {
        "" => ruby.clone(),
        word => word.to_owned(),
    };
    let left_id = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let right_id = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(left_id);
    let meaning_id = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let base_value = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(-30.0);
    DictionaryEntry {
        word,
        ruby,
        left_id,
        right_id,
        meaning_id,
        base_value,
        adjustment: 0.0,
        metadata: Default::default(),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, DictionaryError> {
    fs::read(path).map_err(|source| DictionaryError::Io {
        path: path.to_owned(),
        source,
    })
}

fn read_to_string(path: &Path) -> Result<String, DictionaryError> {
    fs::read_to_string(path).map_err(|source| DictionaryError::Io {
        path: path.to_owned(),
        source,
    })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, DictionaryError> {
    match fs::read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DictionaryError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}
