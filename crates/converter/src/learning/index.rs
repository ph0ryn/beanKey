use std::collections::BTreeMap;

use crate::{CharacterIdMap, Louds};

use super::{LearningRecord, encode_louds};

#[derive(Clone, Copy)]
pub(super) enum MatchKind {
    Exact,
    Prefixes,
    Completions,
}

#[derive(Default)]
struct TrieNode {
    children: BTreeMap<u8, usize>,
    records: Vec<usize>,
}

pub(super) struct MemoryTrie {
    nodes: Vec<TrieNode>,
}

impl Default for MemoryTrie {
    fn default() -> Self {
        Self {
            nodes: vec![TrieNode::default()],
        }
    }
}

impl MemoryTrie {
    pub(super) fn new(records: &[LearningRecord], characters: &CharacterIdMap) -> Self {
        let mut trie = Self::default();
        for (index, record) in records.iter().enumerate() {
            if let Some(ids) = characters.encode(&record.entry.ruby) {
                trie.insert(&ids, index);
            }
        }
        trie
    }

    pub(super) fn insert(&mut self, ids: &[u8], record: usize) {
        let mut node = 0;
        for id in ids {
            node = if let Some(child) = self.nodes[node].children.get(id) {
                *child
            } else {
                let child = self.nodes.len();
                self.nodes.push(TrieNode::default());
                self.nodes[node].children.insert(*id, child);
                child
            };
        }
        self.nodes[node].records.push(record);
    }

    pub(super) fn lookup(&self, ids: &[u8], kind: MatchKind) -> Vec<usize> {
        let mut node = 0;
        let mut records = Vec::new();
        for id in ids {
            let Some(child) = self.nodes[node].children.get(id) else {
                records.sort_unstable();
                return records;
            };
            node = *child;
            if matches!(kind, MatchKind::Prefixes) {
                records.extend_from_slice(&self.nodes[node].records);
            }
        }
        match kind {
            MatchKind::Exact => records.extend_from_slice(&self.nodes[node].records),
            MatchKind::Completions => {
                // Upstream includes the exact node in temporary-memory predictions.
                let mut pending = vec![node];
                while let Some(node) = pending.pop() {
                    records.extend_from_slice(&self.nodes[node].records);
                    pending.extend(self.nodes[node].children.values().copied());
                }
            }
            MatchKind::Prefixes => {}
        }
        records.sort_unstable();
        records
    }

    pub(super) fn encode(&self) -> EncodedIndex {
        let mut bits = vec![true, false];
        let mut characters = vec![0, 0];
        let mut blocks = vec![Vec::new(), Vec::new()];
        let mut current: Vec<_> = self.nodes[0]
            .children
            .iter()
            .map(|(character, index)| (*character, *index))
            .collect();
        bits.extend(std::iter::repeat_n(true, current.len()));
        bits.push(false);
        while !current.is_empty() {
            let mut next = Vec::new();
            for (character, node) in current {
                characters.push(character);
                blocks.push(self.nodes[node].records.clone());
                bits.extend(std::iter::repeat_n(true, self.nodes[node].children.len()));
                bits.push(false);
                next.extend(
                    self.nodes[node]
                        .children
                        .iter()
                        .map(|(character, index)| (*character, *index)),
                );
            }
            current = next;
        }
        EncodedIndex {
            louds: encode_louds(&bits),
            characters,
            blocks,
        }
    }
}

pub(super) struct EncodedIndex {
    pub louds: Vec<u8>,
    pub characters: Vec<u8>,
    pub blocks: Vec<Vec<usize>>,
}

pub(super) struct MemoryLouds {
    louds: Louds,
    blocks: Vec<Vec<usize>>,
}

impl MemoryLouds {
    pub(super) fn new(records: &[LearningRecord], characters: &CharacterIdMap) -> Self {
        let encoded = MemoryTrie::new(records, characters).encode();
        Self {
            louds: Louds::parse(&encoded.louds, &encoded.characters)
                .expect("the learning trie encoder produces valid LOUDS"),
            blocks: encoded.blocks,
        }
    }

    pub(super) fn lookup(&self, ids: &[u8], kind: MatchKind) -> Vec<usize> {
        let mut node = 1;
        let mut records = Vec::new();
        for id in ids {
            let Some(child) = self.louds.search_child(node, *id) else {
                records.sort_unstable();
                return records;
            };
            node = child;
            if matches!(kind, MatchKind::Prefixes) {
                records.extend_from_slice(&self.blocks[node]);
            }
        }
        match kind {
            MatchKind::Exact => records.extend_from_slice(&self.blocks[node]),
            MatchKind::Completions => {
                // Upstream bounds persistent prediction traversal to 700 nodes,
                // without the main dictionary's depth limit or exact node.
                for node in self.louds.descendants(ids, usize::MAX, 700) {
                    records.extend_from_slice(&self.blocks[node]);
                }
            }
            MatchKind::Prefixes => {}
        }
        records.sort_unstable();
        records
    }
}
