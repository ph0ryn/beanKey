use std::collections::HashMap;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::kana::{to_hiragana, to_katakana};
use crate::{
    ComposingCount, ComposingText, DictionaryEntry, DictionaryError, DictionaryMetadata,
    DictionaryStore, InputTableRegistry, PrefixConstraint,
};

const BOS_CLASS_ID: usize = 0;
const BOS_MEANING_ID: usize = 500;
const DICTIONARY_THRESHOLD: f32 = -17.0;
const MAXIMUM_DICTIONARY_LENGTH: usize = 20;
const MAXIMUM_PERSONAL_LENGTH: usize = 128;
const MAXIMUM_PERSONAL_CANDIDATES: usize = 4;
const MAXIMUM_PERSONAL_PATHS: usize = 16;
const MAXIMUM_PERSONAL_TYPOS: usize = 64;

pub(crate) struct PersonalRecallOptions {
    pub maximum: usize,
    pub prediction: bool,
}
const PREDICTION_UNUSABLE_RIGHT_IDS: &[u16] = &[
    13, 14, 15, 16, 17, 18, 25, 26, 27, 28, 33, 34, 40, 41, 42, 46, 47, 50, 56, 57, 58, 59, 60, 61,
    62, 63, 64, 74, 75, 76, 77, 78, 79, 86, 87, 88, 93, 94, 95, 99, 100, 103, 107, 108, 109, 110,
    111, 112, 119, 120, 121, 122, 127, 128, 134, 135, 136, 140, 141, 144, 369, 372, 373, 377, 378,
    379, 380, 381, 382, 389, 390, 391, 392, 397, 398, 401, 402, 404, 405, 406, 408, 410, 411, 412,
    413, 416, 417, 418, 419, 420, 421, 426, 427, 431, 433, 434, 437, 438, 441, 442, 443, 447, 448,
    450, 452, 455, 457, 462, 463, 464, 470, 471, 472, 476, 477, 480, 483, 489, 490, 493, 494, 495,
    496, 504, 527, 528, 533, 534, 537, 540, 542, 548, 551, 553, 561, 562, 564, 565, 566, 567, 569,
    571, 572, 574, 575, 576, 577, 579, 581, 582, 585, 587, 589, 590, 591, 594, 595, 596, 597, 598,
    600, 601, 603, 604, 606, 609, 611, 614, 617, 618, 620, 621, 622, 624, 626, 627, 629, 630, 631,
    634, 636, 638, 641, 642, 644, 647, 648, 650, 653, 654, 656, 659, 660, 662, 665, 666, 668, 671,
    672, 673, 675, 676, 677, 678, 681, 682, 683, 684, 687, 688, 691, 692, 693, 694, 697, 698, 699,
    700, 703, 704, 707, 708, 709, 710, 713, 714, 715, 716, 721, 722, 724, 725, 727, 729, 730, 732,
    733, 736, 737, 739, 740, 742, 744, 745, 747, 748, 750, 752, 753, 755, 756, 758, 760, 761, 763,
    764, 766, 768, 769, 770, 771, 774, 775, 776, 777, 778, 779, 780, 781, 782, 783, 786, 787, 790,
    791, 793, 794, 795, 798, 800, 801, 804, 805, 806, 807, 810, 811, 814, 815, 816, 820, 821, 822,
    823, 824, 825, 829, 830, 831, 835, 837, 840, 842, 845, 847, 850, 852, 855, 859, 860, 862, 865,
    866, 868, 869, 871, 872, 873, 875, 877, 878, 880, 881, 884, 885, 887, 888, 889, 890, 891, 893,
    895, 896, 898, 899, 900, 901, 903, 905, 906, 908, 909, 910, 911, 913, 915, 916, 917, 918, 921,
    922, 923, 924, 925, 928, 929, 931, 932, 934, 935, 936, 939, 941, 943, 944, 945, 946, 947, 948,
    949, 950, 951, 952, 958, 959, 960, 961, 962, 963, 964, 965, 966, 967, 973, 974, 975, 976, 977,
    983, 984, 985, 986, 987, 988, 989, 990, 995, 996, 997, 998, 999, 1000, 1001, 1002, 1007, 1008,
    1009, 1010, 1015, 1016, 1017, 1018, 1021, 1022, 1023, 1024, 1029, 1030, 1031, 1032, 1033, 1034,
    1035, 1036, 1041, 1042, 1043, 1044, 1045, 1046, 1047, 1048, 1057, 1058, 1060, 1061, 1063, 1065,
    1066, 1067, 1068, 1069, 1070, 1071, 1072, 1073, 1074, 1075, 1076, 1077, 1078, 1079, 1080, 1081,
    1082, 1083, 1084, 1085, 1086, 1087, 1088, 1089, 1090, 1104, 1105, 1106, 1107, 1108, 1109, 1110,
    1111, 1112, 1113, 1114, 1115, 1116, 1117, 1118, 1119, 1120, 1121, 1122, 1123, 1124, 1125, 1126,
    1127, 1128, 1129, 1130, 1131, 1132, 1133, 1134, 1135, 1136, 1137, 1138, 1139, 1140, 1141, 1142,
    1143, 1144, 1145, 1146, 1147, 1148, 1149, 1150, 1151, 1152, 1153, 1154, 1155, 1156, 1157, 1158,
    1159, 1160, 1161, 1162, 1163, 1164, 1165, 1166, 1167, 1168, 1182, 1183, 1184, 1185, 1186, 1187,
    1188, 1189, 1190, 1191, 1192, 1193, 1194, 1208, 1209, 1210, 1211, 1212, 1213, 1214, 1215, 1220,
    1221, 1222, 1223, 1224, 1225, 1226, 1227, 1228, 1229, 1230, 1231, 1240, 1241, 1242, 1243, 1248,
    1249, 1250, 1251, 1256, 1257, 1258, 1259, 1260, 1261, 1262, 1263, 1268, 1269, 1270, 1271, 1276,
    1278,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversionContext {
    pub right_id: u16,
    pub meaning_id: u16,
}

impl Default for ConversionContext {
    fn default() -> Self {
        Self {
            right_id: BOS_CLASS_ID as u16,
            meaning_id: BOS_MEANING_ID as u16,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum LatticeIndex {
    Input(usize),
    Surface(usize),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LatticeRange {
    Input { from: usize, to: usize },
    Surface { from: usize, to: usize },
}

impl LatticeRange {
    fn end(self) -> LatticeIndex {
        match self {
            Self::Input { to, .. } => LatticeIndex::Input(to),
            Self::Surface { to, .. } => LatticeIndex::Surface(to),
        }
    }

    fn count(self) -> ComposingCount {
        match self {
            Self::Input { from, to } => ComposingCount::Input(to - from),
            Self::Surface { from, to } => ComposingCount::Surface(to - from),
        }
    }

    fn starts_at_zero(self) -> bool {
        match self {
            Self::Input { from, .. } | Self::Surface { from, .. } => from == 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum DualIndex {
    Input(usize),
    Surface(usize),
    Both { input: usize, surface: usize },
}

impl DualIndex {
    fn input(self) -> Option<usize> {
        match self {
            Self::Input(index) | Self::Both { input: index, .. } => Some(index),
            Self::Surface(_) => None,
        }
    }

    fn surface(self) -> Option<usize> {
        match self {
            Self::Surface(index) | Self::Both { surface: index, .. } => Some(index),
            Self::Input(_) => None,
        }
    }
}

struct DualIndexMap {
    input_to_surface: HashMap<usize, usize>,
    surface_to_input: HashMap<usize, usize>,
    identity: bool,
}

impl DualIndexMap {
    fn new(composing: &ComposingText, tables: &InputTableRegistry) -> Self {
        let input_to_surface = composing.input_to_surface_map(tables);
        let identity = composing.input().len() == composing.surface_graphemes().len()
            && (0..=composing.input().len())
                .all(|index| input_to_surface.get(&index) == Some(&index));
        let mut surface_to_input = HashMap::with_capacity(input_to_surface.len());
        for (&input, &surface) in &input_to_surface {
            surface_to_input.entry(surface).or_insert(input);
        }
        Self {
            input_to_surface,
            surface_to_input,
            identity,
        }
    }

    fn dual(&self, index: LatticeIndex) -> DualIndex {
        if self.identity {
            let index = match index {
                LatticeIndex::Input(index) | LatticeIndex::Surface(index) => index,
            };
            return DualIndex::Both {
                input: index,
                surface: index,
            };
        }
        match index {
            LatticeIndex::Input(input) => match self.input_to_surface.get(&input) {
                Some(&surface) => DualIndex::Both { input, surface },
                None => DualIndex::Input(input),
            },
            LatticeIndex::Surface(surface) => match self.surface_to_input.get(&surface) {
                Some(&input) => DualIndex::Both { input, surface },
                None => DualIndex::Surface(surface),
            },
        }
    }

    fn indices(&self, input_count: usize, surface_count: usize) -> Vec<DualIndex> {
        if self.identity {
            return (0..input_count.min(surface_count))
                .map(|index| DualIndex::Both {
                    input: index,
                    surface: index,
                })
                .collect();
        }
        let mut indices = Vec::with_capacity(input_count + surface_count);
        let mut surface_pointer = 0;
        for input in 0..input_count {
            if let Some(&surface) = self.input_to_surface.get(&input) {
                for value in surface_pointer.min(surface)..surface {
                    indices.push(DualIndex::Surface(value));
                }
                if surface_pointer <= surface && surface < surface_count {
                    indices.push(DualIndex::Both { input, surface });
                } else {
                    indices.push(DualIndex::Input(input));
                }
                surface_pointer = surface + 1;
            } else {
                indices.push(DualIndex::Input(input));
            }
        }
        for surface in surface_pointer.min(surface_count)..surface_count {
            indices.push(DualIndex::Surface(surface));
        }
        indices
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub value: f32,
    pub composing_count: ComposingCount,
    pub last_meaning_id: u16,
    pub entries: Vec<DictionaryEntry>,
    pub ruby_count: usize,
    pub is_learning_target: bool,
    pub is_typo_correction: bool,
    first_clause: Option<FirstClause>,
}

impl Candidate {
    pub fn first_clause_candidate(&self) -> Option<Self> {
        let first = self.first_clause.as_ref()?;
        if first.entry_end + 1 == self.entries.len() {
            return None;
        }
        let entries = self.entries[..=first.entry_end].to_vec();
        let ruby_count = entries
            .iter()
            .map(|entry| UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count())
            .sum();
        Some(Self {
            text: first.text.clone(),
            value: first.value,
            composing_count: first
                .ranges
                .iter()
                .copied()
                .map(LatticeRange::count)
                .reduce(combine_counts)
                .unwrap_or(ComposingCount::Input(0)),
            last_meaning_id: u16::try_from(first.meaning_id).unwrap_or(u16::MAX),
            entries,
            ruby_count,
            is_learning_target: self.is_learning_target,
            is_typo_correction: self.is_typo_correction,
            first_clause: None,
        })
    }

    fn prediction(
        text: String,
        value: f32,
        composing_count: ComposingCount,
        last_meaning_id: u16,
        entries: Vec<DictionaryEntry>,
    ) -> Self {
        Self::single(text, value, composing_count, last_meaning_id, entries)
    }

    pub(crate) fn single(
        text: String,
        value: f32,
        composing_count: ComposingCount,
        last_meaning_id: u16,
        entries: Vec<DictionaryEntry>,
    ) -> Self {
        let ruby_count = entries
            .iter()
            .map(|entry| UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count())
            .sum();
        Self {
            text,
            value,
            composing_count,
            last_meaning_id,
            entries,
            ruby_count,
            is_learning_target: true,
            is_typo_correction: false,
            first_clause: None,
        }
    }

    pub(crate) fn with_learning_target(mut self, value: bool) -> Self {
        self.is_learning_target = value;
        self
    }

    pub(crate) fn expand_templates(mut self) -> Self {
        let expanded = crate::expand_templates(&self.text);
        if expanded != self.text {
            self.text = expanded;
            self.is_learning_target = false;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
struct FirstClause {
    text: String,
    value: f32,
    meaning_id: usize,
    ranges: Vec<LatticeRange>,
    entry_end: usize,
}

#[derive(Clone)]
struct RegisteredPath {
    entry: DictionaryEntry,
    range: LatticeRange,
    previous: Option<Arc<Self>>,
    total: f32,
}

#[derive(Clone)]
struct Predecessor {
    path: Option<Arc<RegisteredPath>>,
    total: f32,
}

struct LatticeNode {
    entry: DictionaryEntry,
    range: LatticeRange,
    predecessors: Vec<Predecessor>,
}

struct ConversionModifiers<'a> {
    additional_entries: &'a [DictionaryEntry],
    need_typo_correction: bool,
    constraint: Option<&'a PrefixConstraint>,
}

pub struct NormalConverter<'a> {
    dictionary: &'a DictionaryStore,
}

impl<'a> NormalConverter<'a> {
    pub fn new(dictionary: &'a DictionaryStore) -> Self {
        Self { dictionary }
    }

    pub fn convert(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_context(composing, tables, n_best, ConversionContext::default())
    }

    pub fn convert_with_context(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries(composing, tables, n_best, context, &[])
    }

    pub fn convert_with_typo_correction(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries_and_typo(composing, tables, n_best, context, &[], true)
    }

    pub fn convert_with_prefix_constraint(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        constraint: &PrefixConstraint,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries_typo_and_constraint(
            composing,
            tables,
            n_best,
            context,
            ConversionModifiers {
                additional_entries: &[],
                need_typo_correction: false,
                constraint: Some(constraint),
            },
        )
    }

    pub(crate) fn convert_with_entries(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries_and_typo(
            composing,
            tables,
            n_best,
            context,
            additional_entries,
            false,
        )
    }

    pub(crate) fn convert_with_entries_and_typo(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
        need_typo_correction: bool,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries_typo_and_constraint(
            composing,
            tables,
            n_best,
            context,
            ConversionModifiers {
                additional_entries,
                need_typo_correction,
                constraint: None,
            },
        )
    }

    pub(crate) fn convert_with_entries_and_prefix_constraint(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
        constraint: &PrefixConstraint,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.convert_with_entries_typo_and_constraint(
            composing,
            tables,
            n_best,
            context,
            ConversionModifiers {
                additional_entries,
                need_typo_correction: false,
                constraint: Some(constraint),
            },
        )
    }

    fn convert_with_entries_typo_and_constraint(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        modifiers: ConversionModifiers<'_>,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        if n_best == 0 || composing.is_empty() {
            return Ok(Vec::new());
        }

        let surface_count = composing.surface_graphemes().len();
        let index_map = DualIndexMap::new(composing, tables);
        let indices = index_map.indices(composing.input().len(), surface_count);
        let katakana = to_katakana(&composing.surface());
        let katakana_graphemes: Vec<_> = UnicodeSegmentation::graphemes(katakana.as_str(), true)
            .map(str::to_owned)
            .collect();

        let mut nodes = Vec::new();
        let mut surface_nodes = vec![Vec::new(); surface_count];
        let mut input_nodes = vec![Vec::new(); composing.input().len()];
        for start in 0..surface_count {
            let suffix = katakana_graphemes[start..].concat();
            for matched in self
                .dictionary
                .matches_from_start(&suffix, MAXIMUM_DICTIONARY_LENGTH)?
            {
                for entry in matched.entries {
                    if should_remove(&entry) {
                        continue;
                    }
                    let node_index = nodes.len();
                    nodes.push(LatticeNode {
                        entry,
                        range: LatticeRange::Surface {
                            from: start,
                            to: start + matched.surface_end,
                        },
                        predecessors: if start == 0 {
                            vec![Predecessor {
                                path: None,
                                total: 0.0,
                            }]
                        } else {
                            Vec::new()
                        },
                    });
                    surface_nodes[start].push(node_index);
                }
            }
            for entry in single_character_entries(&katakana_graphemes[start]) {
                let node_index = nodes.len();
                nodes.push(LatticeNode {
                    entry,
                    range: LatticeRange::Surface {
                        from: start,
                        to: start + 1,
                    },
                    predecessors: if start == 0 {
                        vec![Predecessor {
                            path: None,
                            total: 0.0,
                        }]
                    } else {
                        Vec::new()
                    },
                });
                surface_nodes[start].push(node_index);
            }
            for entry in modifiers.additional_entries {
                let ruby_count = UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count();
                if ruby_count == 0
                    || ruby_count > MAXIMUM_DICTIONARY_LENGTH
                    || ruby_count > surface_count - start
                    || !suffix.starts_with(&entry.ruby)
                    || should_remove(entry)
                {
                    continue;
                }
                let node_index = nodes.len();
                nodes.push(LatticeNode {
                    entry: entry.clone(),
                    range: LatticeRange::Surface {
                        from: start,
                        to: start + ruby_count,
                    },
                    predecessors: if start == 0 {
                        vec![Predecessor {
                            path: None,
                            total: 0.0,
                        }]
                    } else {
                        Vec::new()
                    },
                });
                surface_nodes[start].push(node_index);
            }
        }

        if modifiers.need_typo_correction {
            for (start, start_nodes) in input_nodes.iter_mut().enumerate() {
                for prefix in crate::typo::typo_prefixes(
                    &composing.input()[start..],
                    tables,
                    MAXIMUM_DICTIONARY_LENGTH,
                ) {
                    let mut entries = self.dictionary.exact_match(&prefix.ruby)?;
                    entries.extend(
                        modifiers
                            .additional_entries
                            .iter()
                            .filter(|entry| entry.ruby == prefix.ruby)
                            .cloned(),
                    );
                    for entry in entries {
                        let adjustment = typo_adjustment(&entry, prefix.penalty);
                        let entry = entry.adjusted(adjustment);
                        if should_remove(&entry) {
                            continue;
                        }
                        let node_index = nodes.len();
                        nodes.push(LatticeNode {
                            entry,
                            range: LatticeRange::Input {
                                from: start,
                                to: start + prefix.consumed,
                            },
                            predecessors: if start == 0 {
                                vec![Predecessor {
                                    path: None,
                                    total: 0.0,
                                }]
                            } else {
                                Vec::new()
                            },
                        });
                        start_nodes.push(node_index);
                    }
                }
            }
        }

        let mut results = Vec::new();
        for index in indices {
            let mut current_nodes: Vec<usize> = Vec::new();
            if let Some(surface_index) = index.surface() {
                current_nodes.extend(surface_nodes[surface_index].iter().copied());
            }
            if let Some(input_index) = index.input() {
                current_nodes.extend(input_nodes[input_index].iter().copied());
            }
            for node_index in current_nodes {
                let (entry, range, predecessors) = {
                    let node = &nodes[node_index];
                    (node.entry.clone(), node.range, node.predecessors.clone())
                };
                if predecessors.is_empty() {
                    continue;
                }
                let mut completed = Vec::with_capacity(predecessors.len());
                for predecessor in predecessors {
                    let head_connection = if range.starts_at_zero() {
                        self.dictionary.connection_cost(
                            usize::from(context.right_id),
                            usize::from(entry.left_id),
                        )?
                    } else {
                        0.0
                    };
                    let total = predecessor.total + entry.value() + head_connection;
                    let completed_path = Arc::new(RegisteredPath {
                        entry: entry.clone(),
                        range,
                        previous: predecessor.path,
                        total,
                    });
                    if modifiers.constraint.is_none_or(|constraint| {
                        if enforces_constraint(constraint, &entry) {
                            path_can_continue(&completed_path, constraint)
                        } else {
                            path_matches_fixed_prefix(&completed_path, constraint)
                        }
                    }) {
                        completed.push(completed_path);
                    }
                }

                let next_index = index_map.dual(range.end());
                if next_index.surface() == Some(surface_count)
                    || next_index.input() == Some(composing.input().len())
                {
                    results.extend(completed.into_iter().filter(|path| {
                        modifiers.constraint.is_none_or(|constraint| {
                            if enforces_constraint(constraint, &entry) {
                                path_satisfies(path, constraint)
                            } else {
                                path_matches_fixed_prefix(path, constraint)
                            }
                        })
                    }));
                    continue;
                }
                let mut next_nodes: Vec<usize> = Vec::new();
                if let Some(next_surface) = next_index.surface() {
                    next_nodes.extend(surface_nodes[next_surface].iter().copied());
                }
                if let Some(next_input) = next_index.input() {
                    next_nodes.extend(input_nodes[next_input].iter().copied());
                }
                for next_node_index in next_nodes {
                    let connection = self.dictionary.connection_cost(
                        usize::from(entry.right_id),
                        usize::from(nodes[next_node_index].entry.left_id),
                    )?;
                    for path in &completed {
                        if modifiers.constraint.is_some_and(|constraint| {
                            if enforces_constraint(constraint, &nodes[next_node_index].entry) {
                                !path_and_word_can_continue(
                                    path,
                                    &nodes[next_node_index].entry.word,
                                    constraint,
                                )
                            } else {
                                !path_and_word_matches_fixed_prefix(
                                    path,
                                    &nodes[next_node_index].entry.word,
                                    constraint,
                                )
                            }
                        }) {
                            continue;
                        }
                        insert_predecessor(
                            &mut nodes[next_node_index].predecessors,
                            Predecessor {
                                path: Some(Arc::clone(path)),
                                total: path.total + connection,
                            },
                            n_best,
                        );
                    }
                }
            }
        }

        let mut candidates: Vec<_> = results
            .into_iter()
            .map(|path| self.make_candidate(path, context))
            .collect();
        candidates.sort_by(|left, right| right.value.total_cmp(&left.value));
        Ok(unique_candidates(candidates))
    }

    // Recall is deliberately outside the ordinary lattice and its per-node beam.
    // Surroundings use the bounded, memory-free baseline, including explicit
    // dictionaries. Remembered words retain their original lexical/boundary costs.
    pub(crate) fn personal_candidates(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        additional_entries: &[DictionaryEntry],
        context: ConversionContext,
        memory: &crate::LearningMemory,
        options: PersonalRecallOptions,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let PersonalRecallOptions {
            maximum,
            prediction,
        } = options;
        let maximum = maximum.min(MAXIMUM_PERSONAL_CANDIDATES);
        let consumed = composing.surface_graphemes().len();
        if maximum == 0 || consumed == 0 || consumed > MAXIMUM_PERSONAL_LENGTH {
            return Ok(Vec::new());
        }
        let ruby = to_katakana(&composing.surface());
        let graphemes: Vec<_> = UnicodeSegmentation::graphemes(ruby.as_str(), true).collect();
        let mut output = Vec::new();
        let mut partials = Vec::new();
        let mut examined = 0;
        for start in 0..consumed {
            let suffix = graphemes[start..].concat();
            let mut paths = if prediction {
                memory.predictive_paths(&suffix, MAXIMUM_PERSONAL_LENGTH)?
            } else {
                memory.paths_from_start(&suffix, MAXIMUM_PERSONAL_LENGTH)?
            };
            // Structural retrieval only: a phrase must not lose its protected
            // slot to several shorter prefix records. No evidence or score is
            // used here, and equal lengths retain the memory's retrieval order.
            paths.sort_by_key(|path| {
                std::cmp::Reverse(
                    path.iter()
                        .map(|entry| {
                            UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count()
                        })
                        .sum::<usize>(),
                )
            });
            let mut prefix_entries = None;
            for forced in paths {
                if examined == MAXIMUM_PERSONAL_PATHS || output.len() == maximum {
                    break;
                }
                examined += 1;
                let Some(forced_count) = personal_path_length(&forced) else {
                    continue;
                };
                let forced_ruby = to_katakana(
                    &forced
                        .iter()
                        .map(|entry| entry.ruby.as_str())
                        .collect::<String>(),
                );
                if if prediction {
                    !forced_ruby.starts_with(&suffix)
                } else {
                    !suffix.starts_with(&forced_ruby)
                } {
                    continue;
                }
                if prefix_entries.is_none() {
                    prefix_entries = Some(self.personal_surrounding_entries(
                        &graphemes[..start].concat(),
                        tables,
                        additional_entries,
                        context,
                    )?);
                }
                let Some(prefix) = prefix_entries.as_ref().and_then(Option::as_ref) else {
                    continue;
                };
                let mut entries = prefix.clone();
                entries.extend(forced);
                let end = if prediction {
                    consumed
                } else {
                    start + forced_count
                };
                if personal_path_length(&entries).is_none() {
                    continue;
                }
                let Some(path) = self.register_entry_path(&entries, end, context)? else {
                    continue;
                };
                let recalled = self.make_candidate(path, context);
                if !recalled.value.is_finite() {
                    continue;
                }
                if end == consumed {
                    push_personal_candidate(&mut output, recalled, maximum);
                    continue;
                }

                // Keep a usable prefix even if completing the remainder fails.
                let suffix_context = ConversionContext {
                    right_id: entries
                        .last()
                        .expect("a recalled path is nonempty")
                        .right_id,
                    meaning_id: recalled.last_meaning_id,
                };
                push_personal_candidate(&mut partials, recalled, maximum);
                if let Some(suffix_entries) = self.personal_surrounding_entries(
                    &graphemes[end..].concat(),
                    tables,
                    additional_entries,
                    suffix_context,
                )? {
                    entries.extend(suffix_entries);
                    if personal_path_length(&entries).is_some() {
                        if let Some(path) = self.register_entry_path(&entries, consumed, context)? {
                            push_personal_candidate(
                                &mut output,
                                self.make_candidate(path, context),
                                maximum,
                            );
                        }
                    }
                }
            }
            if examined == MAXIMUM_PERSONAL_PATHS || output.len() == maximum {
                break;
            }
        }
        for partial in partials {
            push_personal_candidate(&mut output, partial, maximum);
        }
        Ok(output)
    }

    // Compatibility recall for whole-input corrections stays separate from the
    // ordinary typo lattice. Its existing 20-element horizon also bounds typo
    // enumeration; recalled entries themselves never pass dictionary pruning.
    pub(crate) fn personal_typo_candidates(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        context: ConversionContext,
        memory: &crate::LearningMemory,
        maximum: usize,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let maximum = maximum.min(MAXIMUM_PERSONAL_CANDIDATES);
        let input = composing.input();
        if maximum == 0 || input.is_empty() || input.len() > MAXIMUM_DICTIONARY_LENGTH {
            return Ok(Vec::new());
        }
        let mut prefixes = crate::typo::typo_prefixes(input, tables, MAXIMUM_DICTIONARY_LENGTH);
        prefixes.retain(|prefix| prefix.consumed == input.len());
        prefixes.sort_by(|left, right| {
            left.penalty
                .total_cmp(&right.penalty)
                .then_with(|| left.ruby.cmp(&right.ruby))
        });
        let mut output = Vec::new();
        let mut examined = 0;
        for prefix in prefixes.into_iter().take(MAXIMUM_PERSONAL_TYPOS) {
            if examined == MAXIMUM_PERSONAL_PATHS || output.len() == maximum {
                break;
            }
            for original in memory.paths_from_start(&prefix.ruby, MAXIMUM_PERSONAL_LENGTH)? {
                let ruby = to_katakana(
                    &original
                        .iter()
                        .map(|entry| entry.ruby.as_str())
                        .collect::<String>(),
                );
                if ruby != prefix.ruby {
                    continue;
                }
                if examined == MAXIMUM_PERSONAL_PATHS || output.len() == maximum {
                    return Ok(output);
                }
                examined += 1;
                if personal_path_length(&original).is_none() {
                    continue;
                }
                // The enumerator supplies aggregate, not per-word, penalties.
                // Applying it conservatively to each cloned entry avoids a
                // zero-cost correction without changing any persisted path.
                let entries: Vec<_> = original
                    .into_iter()
                    .map(|entry| {
                        let adjustment = typo_adjustment(&entry, prefix.penalty);
                        entry.adjusted(adjustment)
                    })
                    .collect();
                let Some(path) = self.register_entry_path(&entries, prefix.consumed, context)?
                else {
                    continue;
                };
                let mut candidate = self.make_candidate(path, context);
                candidate.composing_count = ComposingCount::Input(prefix.consumed);
                candidate.is_typo_correction = true;
                // Corrected ruby offsets need not correspond to original Roman
                // input boundaries, so expose only safe whole-input acceptance.
                candidate.first_clause = None;
                push_personal_candidate(&mut output, candidate, maximum);
            }
        }
        Ok(output)
    }

    fn personal_surrounding_entries(
        &self,
        ruby: &str,
        tables: &InputTableRegistry,
        additional_entries: &[DictionaryEntry],
        context: ConversionContext,
    ) -> Result<Option<Vec<DictionaryEntry>>, DictionaryError> {
        if ruby.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let mut composing = ComposingText::new();
        composing.insert_str(ruby, crate::InputStyle::Direct, tables);
        Ok(self
            .convert_with_entries(&composing, tables, 1, context, additional_entries)?
            .into_iter()
            .next()
            .map(|candidate| candidate.entries))
    }

    pub(crate) fn candidate_path_value(
        &self,
        candidate: &Candidate,
        context: ConversionContext,
    ) -> Result<f32, DictionaryError> {
        Ok(self
            .register_entry_path(&candidate.entries, candidate.ruby_count, context)?
            .map_or(0.0, |path| self.make_candidate(path, context).value))
    }

    fn register_entry_path(
        &self,
        entries: &[DictionaryEntry],
        consumed: usize,
        context: ConversionContext,
    ) -> Result<Option<Arc<RegisteredPath>>, DictionaryError> {
        let mut previous = None;
        let mut right_id = context.right_id;
        let mut total = 0.0;
        let mut surface = 0_usize;
        for entry in entries {
            let connection = self
                .dictionary
                .connection_cost(usize::from(right_id), usize::from(entry.left_id))?;
            total = if previous.is_none() {
                entry.value() + connection
            } else {
                total + connection + entry.value()
            };
            let end = surface
                .saturating_add(UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count());
            previous = Some(Arc::new(RegisteredPath {
                entry: entry.clone(),
                // Prediction may add reading, but never consumes unobserved text.
                range: LatticeRange::Surface {
                    from: surface.min(consumed),
                    to: end.min(consumed),
                },
                previous,
                total,
            }));
            surface = end;
            right_id = entry.right_id;
        }
        Ok(previous)
    }

    pub fn predict(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        self.predict_with_entries(composing, tables, n_best, context, &[])
    }

    pub(crate) fn predict_with_entries(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        if n_best == 0 || composing.is_empty() {
            return Ok(Vec::new());
        }
        let converted =
            self.convert_with_entries(composing, tables, 1, context, additional_entries)?;
        if let Some(candidate) = converted.first() {
            let predictions = self.predict_from_candidate(
                composing,
                candidate,
                n_best,
                context,
                additional_entries,
            )?;
            if !predictions.is_empty() {
                return Ok(predictions);
            }
        }
        self.predict_whole_input(composing, tables, n_best, context, additional_entries)
    }

    fn predict_whole_input(
        &self,
        composing: &ComposingText,
        tables: &InputTableRegistry,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let input_style = composing
            .input()
            .last()
            .map(|element| &element.input_style)
            .unwrap_or(&crate::InputStyle::Direct);
        let surface = composing.surface();
        let prefixes = if matches!(input_style, crate::InputStyle::Direct) {
            vec![to_katakana(&surface)]
        } else {
            let roman_count = surface
                .chars()
                .rev()
                .take_while(|character| character.is_ascii_alphabetic())
                .count();
            if roman_count == 0 {
                vec![to_katakana(&surface)]
            } else {
                let split = surface.len() - roman_count;
                let base = surface[..split].to_owned();
                if base.is_empty() {
                    return Ok(Vec::new());
                }
                let roman = &surface[split..];
                tables
                    .resolve(input_style)
                    .map(|table| {
                        table
                            .possible_nexts(roman)
                            .into_iter()
                            .map(|suffix| format!("{}{suffix}", to_katakana(&base)))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        };
        self.predict_for_ruby_prefixes(
            &prefixes,
            &[],
            UnicodeSegmentation::graphemes(surface.as_str(), true).count(),
            n_best,
            context,
            additional_entries,
        )
    }

    fn predict_from_candidate(
        &self,
        composing: &ComposingText,
        candidate: &Candidate,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        if candidate.entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut clause_starts = vec![0];
        for index in 1..candidate.entries.len() {
            if is_clause(
                usize::from(candidate.entries[index - 1].right_id),
                usize::from(candidate.entries[index].left_id),
            ) {
                clause_starts.push(index);
            }
        }
        let consumed = composing.surface_graphemes().len();
        let mut output = Vec::new();
        let mut successful_groups = 0;
        for &start in clause_starts.iter().rev() {
            let ruby = candidate.entries[start..]
                .iter()
                .map(|entry| entry.ruby.as_str())
                .collect::<String>();
            let predictions = self.predict_for_ruby_prefixes(
                &[ruby],
                &candidate.entries[..start],
                consumed,
                5,
                context,
                additional_entries,
            )?;
            if !predictions.is_empty() {
                output.extend(predictions);
                successful_groups += 1;
            }
            if successful_groups == 2 {
                if start > 0 {
                    let full_ruby = candidate
                        .entries
                        .iter()
                        .map(|entry| entry.ruby.as_str())
                        .collect::<String>();
                    output.extend(self.predict_for_ruby_prefixes(
                        &[full_ruby],
                        &[],
                        consumed,
                        5,
                        context,
                        additional_entries,
                    )?);
                }
                break;
            }
        }
        output = unique_candidates(output);
        output.sort_by(|left, right| right.value.total_cmp(&left.value));
        output.truncate(n_best);
        Ok(output)
    }

    fn predict_for_ruby_prefixes(
        &self,
        ruby_prefixes: &[String],
        prefix_entries: &[DictionaryEntry],
        consumed: usize,
        n_best: usize,
        context: ConversionContext,
        additional_entries: &[DictionaryEntry],
    ) -> Result<Vec<Candidate>, DictionaryError> {
        let (prefix_value, prefix_right, prefix_meaning) =
            self.score_entries(prefix_entries, context)?;
        let prefix_text = prefix_entries
            .iter()
            .map(|entry| entry.word.as_str())
            .collect::<String>();
        let mut output = Vec::new();
        for prefix in ruby_prefixes {
            let prefix_count = UnicodeSegmentation::graphemes(prefix.as_str(), true).count();
            let maximum_depth = match prefix_count {
                1 => 3,
                2 => 5,
                _ => usize::MAX,
            };
            let mut entries = self
                .dictionary
                .entries_after_prefix(prefix, maximum_depth, 700)?;
            entries.retain(|entry| prediction_usable(entry.right_id));
            entries.extend(
                additional_entries
                    .iter()
                    .filter(|entry| entry.ruby.starts_with(prefix) && entry.ruby != *prefix)
                    .cloned(),
            );
            for entry in entries {
                let entry_ruby_count =
                    UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count();
                let penalty = -(entry_ruby_count.saturating_sub(prefix_count) as f32);
                let meaning = if includes_meaning(&entry) {
                    self.dictionary
                        .meaning_cost(usize::from(prefix_meaning), usize::from(entry.meaning_id))
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                let value = prefix_value
                    + self
                        .dictionary
                        .connection_cost(usize::from(prefix_right), usize::from(entry.left_id))?
                    + entry.value()
                    + meaning
                    + penalty;
                let last_meaning_id = if includes_meaning(&entry) {
                    entry.meaning_id
                } else {
                    prefix_meaning
                };
                let mut candidate_entries = prefix_entries.to_vec();
                candidate_entries.push(entry.clone());
                output.push(Candidate::prediction(
                    format!("{prefix_text}{}", entry.word),
                    value,
                    ComposingCount::Surface(consumed),
                    last_meaning_id,
                    candidate_entries,
                ));
            }
        }
        output.sort_by(|left, right| right.value.total_cmp(&left.value));
        output = unique_candidates(output);
        output.truncate(n_best);
        Ok(output)
    }

    fn score_entries(
        &self,
        entries: &[DictionaryEntry],
        context: ConversionContext,
    ) -> Result<(f32, u16, u16), DictionaryError> {
        if entries.is_empty() {
            return Ok((0.0, context.right_id, context.meaning_id));
        }
        let mut value = 0.0;
        let mut previous_right = context.right_id;
        let mut clause_meanings = vec![usize::from(context.meaning_id)];
        for (index, entry) in entries.iter().enumerate() {
            let starts_clause =
                index > 0 && is_clause(usize::from(previous_right), usize::from(entry.left_id));
            if starts_clause {
                clause_meanings.push(if includes_meaning(entry) {
                    usize::from(entry.meaning_id)
                } else {
                    BOS_MEANING_ID
                });
            }
            let meaning = clause_meanings
                .last_mut()
                .expect("an entry always belongs to a clause");
            if (*meaning == BOS_MEANING_ID && entry.meaning_id != BOS_MEANING_ID as u16)
                || includes_meaning(entry)
            {
                *meaning = usize::from(entry.meaning_id);
            }
            value += self
                .dictionary
                .connection_cost(usize::from(previous_right), usize::from(entry.left_id))?
                + entry.value();
            previous_right = entry.right_id;
        }
        let mut previous_meaning = usize::from(context.meaning_id);
        for meaning in clause_meanings {
            value += self
                .dictionary
                .meaning_cost(previous_meaning, meaning)
                .unwrap_or(0.0);
            previous_meaning = meaning;
        }
        Ok((
            value,
            previous_right,
            u16::try_from(previous_meaning).unwrap_or(u16::MAX),
        ))
    }

    pub fn word_candidates(
        &self,
        composing: &ComposingText,
    ) -> Result<Vec<Candidate>, DictionaryError> {
        if composing.is_empty() {
            return Ok(Vec::new());
        }
        let katakana = to_katakana(&composing.surface());
        let mut output = Vec::new();
        for matched in self
            .dictionary
            .matches_from_start(&katakana, MAXIMUM_DICTIONARY_LENGTH)?
        {
            for entry in matched.entries {
                let value = entry.value();
                output.push(Candidate::single(
                    entry.word.clone(),
                    value,
                    ComposingCount::Surface(matched.surface_end),
                    entry.meaning_id,
                    vec![entry],
                ));
            }
        }
        output = unique_candidates(output);
        output.sort_by(|left, right| {
            right
                .ruby_count
                .cmp(&left.ruby_count)
                .then_with(|| right.value.total_cmp(&left.value))
        });
        Ok(output)
    }

    pub fn representation_candidates(
        &self,
        composing: &ComposingText,
        full_width_roman: bool,
        half_width_kana: bool,
    ) -> Vec<Candidate> {
        let katakana = to_katakana(&composing.surface());
        let composing_count = ComposingCount::Input(composing.input().len());
        let mut output = Vec::new();
        let katakana_value = -14.0 * katakana_score(&katakana);
        output.push(representation_candidate(
            katakana.clone(),
            katakana.clone(),
            katakana_value,
            composing_count.clone(),
        ));
        output.push(representation_candidate(
            to_hiragana(&katakana),
            katakana.clone(),
            -14.5,
            composing_count.clone(),
        ));
        output.push(representation_candidate(
            katakana.to_uppercase(),
            katakana.clone(),
            -14.6,
            composing_count.clone(),
        ));
        if full_width_roman {
            output.push(representation_candidate(
                to_full_width(&katakana),
                katakana.clone(),
                -14.7,
                composing_count.clone(),
            ));
        }
        if half_width_kana {
            output.push(representation_candidate(
                to_half_width(&katakana),
                katakana.clone(),
                -15.0,
                composing_count,
            ));
        }
        output
    }

    fn make_candidate(&self, path: Arc<RegisteredPath>, context: ConversionContext) -> Candidate {
        let mut paths = Vec::new();
        let mut cursor = Some(path);
        while let Some(current) = cursor {
            paths.push(Arc::clone(&current));
            cursor = current.previous.clone();
        }
        paths.reverse();

        let is_typo_correction = paths
            .iter()
            .any(|path| matches!(path.range, LatticeRange::Input { .. }));
        let mut clauses = vec![Clause {
            text: String::new(),
            value: 0.0,
            meaning_id: usize::from(context.meaning_id),
            ranges: Vec::new(),
            entry_end: 0,
        }];
        let mut entries = Vec::with_capacity(paths.len());
        let mut previous_right = BOS_CLASS_ID;
        for current in paths {
            let entry = &current.entry;
            let continue_clause = clauses.last().is_some_and(|clause| clause.text.is_empty())
                || !is_clause(previous_right, usize::from(entry.left_id));
            if !continue_clause {
                clauses.push(Clause {
                    text: String::new(),
                    value: current.total,
                    meaning_id: if includes_meaning(entry) {
                        usize::from(entry.meaning_id)
                    } else {
                        BOS_MEANING_ID
                    },
                    ranges: Vec::new(),
                    entry_end: entries.len(),
                });
            }
            let clause = clauses.last_mut().expect("a candidate has a clause");
            clause.text.push_str(&entry.word);
            clause.value = current.total;
            clause.ranges.push(current.range);
            clause.entry_end = entries.len();
            if (clause.meaning_id == BOS_MEANING_ID && entry.meaning_id != BOS_MEANING_ID as u16)
                || includes_meaning(entry)
            {
                clause.meaning_id = usize::from(entry.meaning_id);
            }
            previous_right = usize::from(entry.right_id);
            entries.push(entry.clone());
        }

        let mut previous_meaning = BOS_MEANING_ID;
        let mut meaning_value = 0.0;
        for clause in &clauses {
            meaning_value += self
                .dictionary
                .meaning_cost(previous_meaning, clause.meaning_id)
                .unwrap_or(0.0);
            previous_meaning = clause.meaning_id;
        }
        let text: String = clauses.iter().map(|clause| clause.text.as_str()).collect();
        let composing_count = clauses
            .iter()
            .flat_map(|clause| clause.ranges.iter().copied())
            .map(LatticeRange::count)
            .reduce(combine_counts)
            .unwrap_or(ComposingCount::Input(0));
        let ruby_count = entries
            .iter()
            .map(|entry| UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count())
            .sum();
        let first_clause = clauses.first().map(|clause| FirstClause {
            text: clause.text.clone(),
            value: clause.value,
            meaning_id: clause.meaning_id,
            ranges: clause.ranges.clone(),
            entry_end: clause.entry_end,
        });
        Candidate {
            text,
            value: clauses
                .last()
                .map_or(meaning_value, |clause| clause.value + meaning_value),
            composing_count,
            last_meaning_id: u16::try_from(previous_meaning).unwrap_or(u16::MAX),
            entries,
            ruby_count,
            is_learning_target: true,
            is_typo_correction,
            first_clause,
        }
    }
}

struct Clause {
    text: String,
    value: f32,
    meaning_id: usize,
    ranges: Vec<LatticeRange>,
    entry_end: usize,
}

fn insert_predecessor(values: &mut Vec<Predecessor>, value: Predecessor, n_best: usize) {
    let index = values
        .iter()
        .rposition(|existing| existing.total >= value.total)
        .map_or(0, |index| index + 1);
    if index == n_best {
        return;
    }
    if values.len() >= n_best {
        values.pop();
    }
    values.insert(index, value);
}

fn personal_path_length(entries: &[DictionaryEntry]) -> Option<usize> {
    if entries.is_empty() || entries.len() > MAXIMUM_PERSONAL_LENGTH {
        return None;
    }
    let mut length = 0_usize;
    for entry in entries {
        // These are the fixed dictionary binary format's matrix dimensions.
        // Persisted boundary IDs must not select unrelated/out-of-range costs.
        if entry.left_id >= 1319
            || entry.right_id >= 1319
            || entry.meaning_id >= 502
            || entry.word.is_empty()
            || entry.ruby.is_empty()
            || !entry.base_value.is_finite()
            || !entry.adjustment.is_finite()
            || !entry.value().is_finite()
        {
            return None;
        }
        length = length
            .checked_add(UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count())?;
        if length > MAXIMUM_PERSONAL_LENGTH {
            return None;
        }
    }
    Some(length)
}

fn push_personal_candidate(output: &mut Vec<Candidate>, candidate: Candidate, maximum: usize) {
    if output.len() < maximum
        && !candidate.text.is_empty()
        && candidate.value.is_finite()
        && !output.iter().any(|existing| {
            existing.text == candidate.text && existing.composing_count == candidate.composing_count
        })
    {
        output.push(candidate);
    }
}

fn unique_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut output: Vec<Candidate> = Vec::new();
    let mut indices = HashMap::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| !candidate.text.is_empty())
    {
        if let Some(&index) = indices.get(&candidate.text) {
            let existing: &mut Candidate = &mut output[index];
            if existing.value < candidate.value || existing.ruby_count < candidate.ruby_count {
                *existing = candidate;
            }
        } else {
            indices.insert(candidate.text.clone(), output.len());
            output.push(candidate);
        }
    }
    output.sort_by(|left, right| right.value.total_cmp(&left.value));
    output
}

fn combine_counts(left: ComposingCount, right: ComposingCount) -> ComposingCount {
    match (left, right) {
        (ComposingCount::Input(left), ComposingCount::Input(right)) => {
            ComposingCount::Input(left + right)
        }
        (ComposingCount::Surface(left), ComposingCount::Surface(right)) => {
            ComposingCount::Surface(left + right)
        }
        (left, right) => ComposingCount::Composite(Box::new(left), Box::new(right)),
    }
}

fn should_remove(entry: &DictionaryEntry) -> bool {
    let distance = entry.value() - DICTIONARY_THRESHOLD;
    if distance < 0.0 {
        return true;
    }
    let word_count = UnicodeSegmentation::graphemes(entry.word.as_str(), true).count();
    word_count == 0 || -2.0 / (word_count as f32) < -distance
}

fn typo_adjustment(entry: &DictionaryEntry, penalty: f32) -> f32 {
    let word_count = UnicodeSegmentation::graphemes(entry.word.as_str(), true)
        .count()
        .max(1) as f32;
    let ratio = if (147..=554).contains(&entry.left_id) {
        2.5
    } else {
        1.0
    };
    (-1.0 / word_count) * penalty * ratio
}

fn word_type(class_id: usize) -> u8 {
    if matches!(class_id, 0 | 1316) {
        return 3;
    }
    if matches!(class_id, 1315 | 6 | 557 | 558 | 559 | 560) {
        return 0;
    }
    if (561..868).contains(&class_id)
        || (1283..1297).contains(&class_id)
        || (1306..1310).contains(&class_id)
        || (11..53).contains(&class_id)
        || (555..557).contains(&class_id)
        || (1281..1283).contains(&class_id)
        || matches!(class_id, 1314 | 3 | 2 | 4 | 5 | 1 | 9)
    {
        return 1;
    }
    2
}

pub(crate) fn is_clause(former: usize, latter: usize) -> bool {
    let latter_type = word_type(latter);
    if latter_type == 3 {
        return false;
    }
    let former_type = word_type(former);
    if former_type == 3 {
        return false;
    }
    matches!(latter_type, 0 | 1) && former_type != 0
}

pub(crate) fn includes_meaning(entry: &DictionaryEntry) -> bool {
    let left = usize::from(entry.left_id);
    let right = usize::from(entry.right_id);
    (895..=1280).contains(&left)
        || (895..=1280).contains(&right)
        || (1297..=1305).contains(&left)
        || (1297..=1305).contains(&right)
        || word_type(left) == 1
        || word_type(right) == 1
}

pub(crate) fn prediction_usable(right_id: u16) -> bool {
    PREDICTION_UNUSABLE_RIGHT_IDS
        .binary_search(&right_id)
        .is_err()
}

fn enforces_constraint(constraint: &PrefixConstraint, entry: &DictionaryEntry) -> bool {
    !constraint.ignore_memory_and_user_dictionary
        && !entry.metadata.contains(DictionaryMetadata::LEARNED)
        && !entry.metadata.contains(DictionaryMetadata::USER_DICTIONARY)
}

fn path_bytes(path: &Arc<RegisteredPath>) -> Vec<u8> {
    let mut paths = Vec::new();
    let mut cursor = Some(Arc::clone(path));
    while let Some(current) = cursor {
        paths.push(Arc::clone(&current));
        cursor = current.previous.clone();
    }
    paths.reverse();
    paths
        .iter()
        .flat_map(|path| path.entry.word.bytes())
        .collect()
}

fn path_can_continue(path: &Arc<RegisteredPath>, constraint: &PrefixConstraint) -> bool {
    constraint.can_continue(&path_bytes(path))
}

fn path_matches_fixed_prefix(path: &Arc<RegisteredPath>, constraint: &PrefixConstraint) -> bool {
    if constraint.fixed_prefix.is_none() {
        return true;
    }
    bytes_match_fixed_prefix(&path_bytes(path), constraint)
}

fn path_and_word_can_continue(
    path: &Arc<RegisteredPath>,
    word: &str,
    constraint: &PrefixConstraint,
) -> bool {
    let mut bytes = path_bytes(path);
    bytes.extend_from_slice(word.as_bytes());
    constraint.can_continue(&bytes)
}

fn path_and_word_matches_fixed_prefix(
    path: &Arc<RegisteredPath>,
    word: &str,
    constraint: &PrefixConstraint,
) -> bool {
    if constraint.fixed_prefix.is_none() {
        return true;
    }
    let mut bytes = path_bytes(path);
    bytes.extend_from_slice(word.as_bytes());
    bytes_match_fixed_prefix(&bytes, constraint)
}

fn bytes_match_fixed_prefix(candidate: &[u8], constraint: &PrefixConstraint) -> bool {
    let Some(fixed_prefix) = constraint.fixed_prefix else {
        return true;
    };
    let Some(expected) = constraint.bytes.get(..fixed_prefix.byte_count) else {
        return false;
    };
    let compared = candidate.len().min(fixed_prefix.byte_count);
    expected.get(..compared) == candidate.get(..compared)
}

fn path_satisfies(path: &Arc<RegisteredPath>, constraint: &PrefixConstraint) -> bool {
    constraint.is_satisfied_by(&path_bytes(path))
}

fn katakana_score(value: &str) -> f32 {
    value.chars().fold(1.0, |score, character| {
        score
            * if "プヴペィフ".contains(character) {
                0.5
            } else if "ュピポ".contains(character) {
                0.6
            } else if "パォグーム".contains(character) {
                0.7
            } else {
                1.0
            }
    })
}

fn representation_candidate(
    word: String,
    ruby: String,
    value: f32,
    composing_count: ComposingCount,
) -> Candidate {
    let entry = DictionaryEntry {
        word: word.clone(),
        ruby,
        left_id: 1288,
        right_id: 1288,
        meaning_id: 501,
        base_value: value,
        adjustment: 0.0,
        metadata: Default::default(),
    };
    Candidate::single(word, value, composing_count, 501, vec![entry])
}

fn single_character_entries(katakana: &str) -> Vec<DictionaryEntry> {
    let hiragana = to_hiragana(katakana);
    let words = if hiragana == katakana {
        vec![(katakana.to_owned(), -14.0)]
    } else {
        vec![(hiragana, -13.0), (katakana.to_owned(), -14.0)]
    };
    words
        .into_iter()
        .map(|(word, value)| DictionaryEntry {
            word,
            ruby: katakana.to_owned(),
            left_id: 1288,
            right_id: 1288,
            meaning_id: 501,
            base_value: value,
            adjustment: 0.0,
            metadata: DictionaryMetadata::default(),
        })
        .collect()
}

pub(crate) fn to_full_width(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ' ' => '\u{3000}',
            '¥' => '￥',
            '!'..='~' => {
                char::from_u32(u32::from(character) + 0xfee0).expect("full-width scalar is valid")
            }
            _ => character,
        })
        .collect()
}

pub(crate) fn to_half_width(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let mapped = match character {
            '。' => "｡",
            '「' => "｢",
            '」' => "｣",
            '、' => "､",
            '・' => "･",
            'ヲ' => "ｦ",
            'ァ' => "ｧ",
            'ィ' => "ｨ",
            'ゥ' => "ｩ",
            'ェ' => "ｪ",
            'ォ' => "ｫ",
            'ャ' => "ｬ",
            'ュ' => "ｭ",
            'ョ' => "ｮ",
            'ッ' => "ｯ",
            'ー' => "ｰ",
            'ア' => "ｱ",
            'イ' => "ｲ",
            'ウ' => "ｳ",
            'エ' => "ｴ",
            'オ' => "ｵ",
            'カ' => "ｶ",
            'キ' => "ｷ",
            'ク' => "ｸ",
            'ケ' => "ｹ",
            'コ' => "ｺ",
            'サ' => "ｻ",
            'シ' => "ｼ",
            'ス' => "ｽ",
            'セ' => "ｾ",
            'ソ' => "ｿ",
            'タ' => "ﾀ",
            'チ' => "ﾁ",
            'ツ' => "ﾂ",
            'テ' => "ﾃ",
            'ト' => "ﾄ",
            'ナ' => "ﾅ",
            'ニ' => "ﾆ",
            'ヌ' => "ﾇ",
            'ネ' => "ﾈ",
            'ノ' => "ﾉ",
            'ハ' => "ﾊ",
            'ヒ' => "ﾋ",
            'フ' => "ﾌ",
            'ヘ' => "ﾍ",
            'ホ' => "ﾎ",
            'マ' => "ﾏ",
            'ミ' => "ﾐ",
            'ム' => "ﾑ",
            'メ' => "ﾒ",
            'モ' => "ﾓ",
            'ヤ' => "ﾔ",
            'ユ' => "ﾕ",
            'ヨ' => "ﾖ",
            'ラ' => "ﾗ",
            'リ' => "ﾘ",
            'ル' => "ﾙ",
            'レ' => "ﾚ",
            'ロ' => "ﾛ",
            'ワ' => "ﾜ",
            'ン' => "ﾝ",
            'ヴ' => "ｳﾞ",
            'ガ' => "ｶﾞ",
            'ギ' => "ｷﾞ",
            'グ' => "ｸﾞ",
            'ゲ' => "ｹﾞ",
            'ゴ' => "ｺﾞ",
            'ザ' => "ｻﾞ",
            'ジ' => "ｼﾞ",
            'ズ' => "ｽﾞ",
            'ゼ' => "ｾﾞ",
            'ゾ' => "ｿﾞ",
            'ダ' => "ﾀﾞ",
            'ヂ' => "ﾁﾞ",
            'ヅ' => "ﾂﾞ",
            'デ' => "ﾃﾞ",
            'ド' => "ﾄﾞ",
            'バ' => "ﾊﾞ",
            'ビ' => "ﾋﾞ",
            'ブ' => "ﾌﾞ",
            'ベ' => "ﾍﾞ",
            'ボ' => "ﾎﾞ",
            'パ' => "ﾊﾟ",
            'ピ' => "ﾋﾟ",
            'プ' => "ﾌﾟ",
            'ペ' => "ﾍﾟ",
            'ポ' => "ﾎﾟ",
            _ => {
                output.push(character);
                continue;
            }
        };
        output.push_str(mapped);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputStyle;

    fn entry(word: &str, value: f32) -> DictionaryEntry {
        DictionaryEntry {
            word: word.into(),
            ruby: "ルビ".into(),
            left_id: 1285,
            right_id: 1285,
            meaning_id: 501,
            base_value: value,
            adjustment: 0.0,
            metadata: Default::default(),
        }
    }

    struct PersonalFixture {
        root: std::path::PathBuf,
    }

    impl PersonalFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "beankey-lattice-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            std::fs::create_dir_all(root.join("louds")).unwrap();
            std::fs::create_dir_all(root.join("cb")).unwrap();
            std::fs::write(
                root.join("louds/charID.chid"),
                "\0アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲンガギグゲゴッャュョー",
            )
            .unwrap();
            let mut meanings = vec![0.0_f32; 502 * 502];
            meanings[502 + 2] = -7.25;
            let binary: Vec<_> = meanings.into_iter().flat_map(f32::to_le_bytes).collect();
            std::fs::write(root.join("mm.binary"), binary).unwrap();
            for (right, default, overrides) in [
                (0, -0.5_f32, vec![]),
                (7, -0.5, vec![]),
                (100, -2.5, vec![]),
                (1285, -0.75, vec![(100_i32, -1.25_f32)]),
                (1288, -1.0, vec![]),
            ] {
                let mut binary = Vec::new();
                for (left, cost) in std::iter::once((-1_i32, default)).chain(overrides) {
                    binary.extend(left.to_le_bytes());
                    binary.extend(cost.to_le_bytes());
                }
                std::fs::write(root.join(format!("cb/{right}.binary")), binary).unwrap();
            }
            Self { root }
        }

        fn dictionary(&self) -> DictionaryStore {
            DictionaryStore::open(&self.root).unwrap()
        }

        fn memory(&self, dictionary: &DictionaryStore) -> crate::LearningMemory {
            crate::LearningMemory::open(
                self.root.join("memory"),
                crate::LearningMode::InputAndOutput,
                128,
                dictionary.character_ids().clone(),
            )
            .unwrap()
        }
    }

    impl Drop for PersonalFixture {
        fn drop(&mut self) {
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            assert_eq!(self.root.parent(), Some(temporary.as_path()));
            assert!(
                self.root
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("beankey-lattice-")
            );
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn recalled_entry(word: &str, ruby: &str, value: f32) -> DictionaryEntry {
        DictionaryEntry {
            ruby: ruby.into(),
            ..entry(word, value)
        }
    }

    fn composing(value: &str, tables: &InputTableRegistry) -> ComposingText {
        let mut composing = ComposingText::new();
        composing.insert_str(value, InputStyle::Direct, tables);
        composing
    }

    fn remember(memory: &crate::LearningMemory, entries: &[DictionaryEntry]) {
        memory
            .learn(&Candidate::single(
                entries.iter().map(|entry| entry.word.as_str()).collect(),
                1234.0, // A ranking carrier is not a lexical cost.
                ComposingCount::Surface(
                    entries
                        .iter()
                        .map(|entry| {
                            UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count()
                        })
                        .sum(),
                ),
                entries.last().unwrap().meaning_id,
                entries.to_vec(),
            ))
            .unwrap();
    }

    #[test]
    fn personal_multiword_paths_keep_lexical_connection_and_clause_meaning_costs() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let entries = vec![
            DictionaryEntry {
                meaning_id: 1,
                ..recalled_entry("甲", "カ", -3.0)
            },
            DictionaryEntry {
                left_id: 100,
                right_id: 100,
                ..recalled_entry("助", "ナ", -4.0)
            },
            DictionaryEntry {
                meaning_id: 2,
                ..recalled_entry("乙", "ニ", -5.0)
            },
        ];
        let context = ConversionContext {
            right_id: 7,
            meaning_id: 3,
        };
        let composing = composing("かなに", &tables);
        let ordinary = converter
            .convert_with_entries(&composing, &tables, 8, context, &entries)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "甲助乙")
            .unwrap();
        remember(&memory, &entries);
        let recalled = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                context,
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: false,
                },
            )
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == ordinary.text)
            .unwrap();

        assert_eq!(ordinary.value, -23.5);
        assert_eq!(recalled.value, ordinary.value);
        assert_eq!(recalled.entries.len(), 3);
        for (original, recalled) in entries.iter().zip(&recalled.entries) {
            assert_eq!(recalled.word, original.word);
            assert_eq!(recalled.ruby, original.ruby);
            assert_eq!(recalled.base_value, original.base_value);
            assert_eq!(recalled.adjustment, original.adjustment);
        }
        assert_eq!(recalled.composing_count, ComposingCount::Surface(3));
        assert_eq!(recalled.first_clause_candidate().unwrap().text, "甲助");
        assert_eq!(
            recalled.first_clause_candidate().unwrap().composing_count,
            ComposingCount::Surface(2)
        );
        assert_eq!(
            recalled.first_clause_candidate().unwrap().value,
            ordinary.first_clause_candidate().unwrap().value
        );
        let mut reranked = recalled;
        reranked.value = 9999.0;
        assert_eq!(
            converter.candidate_path_value(&reranked, context).unwrap(),
            ordinary.value
        );
    }

    #[test]
    fn personal_paths_bypass_dictionary_threshold_and_twenty_character_limit() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let entries = vec![
            recalled_entry("個人句", &"カ".repeat(24), -30.0),
            recalled_entry("名前", "ナニ", -6.0),
        ];
        let composing = composing(&format!("{}なに", "か".repeat(24)), &tables);
        assert!(should_remove(&entries[0]));
        assert!(
            converter
                .convert_with_entries(
                    &composing,
                    &tables,
                    1,
                    ConversionContext::default(),
                    &entries
                )
                .unwrap()
                .iter()
                .all(|candidate| candidate.text != "個人句名前")
        );
        remember(&memory, &entries);
        let recalled = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                ConversionContext::default(),
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: false,
                },
            )
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "個人句名前")
            .unwrap();
        assert_eq!(recalled.entries.len(), 2);
        assert_eq!(recalled.entries[0].base_value, -30.0);
        assert_eq!(recalled.value, -37.25);
        assert_eq!(recalled.composing_count, ComposingCount::Surface(26));
    }

    #[test]
    fn personal_phrase_remains_accessible_when_the_normal_beam_is_one() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let entries = vec![
            recalled_entry("通常", "カ", -1.0),
            recalled_entry("個人", "カ", -8.0),
            recalled_entry("名", "ナ", -2.0),
        ];
        let composing = composing("かな", &tables);
        assert!(
            converter
                .convert_with_entries(
                    &composing,
                    &tables,
                    1,
                    ConversionContext::default(),
                    &entries
                )
                .unwrap()
                .iter()
                .all(|candidate| candidate.text != "個人名")
        );
        remember(&memory, &entries[1..]);
        for index in 0..6 {
            remember(
                &memory,
                &[recalled_entry(&format!("短語{index}"), "カ", -2.0)],
            );
        }
        let recalled = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                ConversionContext::default(),
                &memory,
                PersonalRecallOptions {
                    maximum: 1,
                    prediction: false,
                },
            )
            .unwrap();
        assert_eq!(recalled.len(), 1);
        assert_eq!(recalled[0].text, "個人名");
    }

    #[test]
    fn personal_memory_cannot_change_normal_conversion_typo_or_prediction() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let composing = composing("かな", &tables);
        let context = ConversionContext::default();
        let before = converter.convert(&composing, &tables, 1).unwrap();
        let typo_before = converter
            .convert_with_typo_correction(&composing, &tables, 1, context)
            .unwrap();
        let prediction_before = converter.predict(&composing, &tables, 4, context).unwrap();
        let user_word = DictionaryEntry {
            metadata: DictionaryMetadata::USER_DICTIONARY,
            ..recalled_entry("手入力", "カナ", -3.0)
        };
        let additional_before = converter
            .convert_with_entries(
                &composing,
                &tables,
                1,
                context,
                std::slice::from_ref(&user_word),
            )
            .unwrap();
        remember(&memory, &[recalled_entry("私有語", "カナ", -8.0)]);
        remember(&memory, &[recalled_entry("私有予測", "カナニ", -8.0)]);

        assert_eq!(converter.convert(&composing, &tables, 1).unwrap(), before);
        assert_eq!(
            converter
                .convert_with_typo_correction(&composing, &tables, 1, context)
                .unwrap(),
            typo_before
        );
        assert_eq!(
            converter.predict(&composing, &tables, 4, context).unwrap(),
            prediction_before
        );
        assert_eq!(
            converter
                .convert_with_entries(&composing, &tables, 1, context, &[user_word])
                .unwrap(),
            additional_before
        );
        assert!(
            additional_before
                .iter()
                .any(|candidate| candidate.text == "手入力")
        );
        assert!(
            converter
                .personal_candidates(
                    &composing,
                    &tables,
                    &[],
                    context,
                    &memory,
                    PersonalRecallOptions {
                        maximum: 4,
                        prediction: false
                    }
                )
                .unwrap()
                .iter()
                .any(|candidate| candidate.text == "私有語")
        );
    }

    #[test]
    fn personal_typo_recall_keeps_real_path_costs_without_mutating_memory() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let context = ConversionContext::default();
        let composing = composing("かくせい", &tables);
        let entries = vec![
            DictionaryEntry {
                adjustment: -0.5,
                ..recalled_entry("個人", "ガク", -30.0)
            },
            recalled_entry("氏名", "セイ", -4.0),
        ];
        let before = converter
            .convert_with_typo_correction(&composing, &tables, 1, context)
            .unwrap();
        remember(&memory, &entries);
        let candidate = converter
            .personal_typo_candidates(&composing, &tables, context, &memory, 4)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "個人氏名")
            .unwrap();
        assert_eq!(candidate.entries.len(), 2);
        assert_eq!(candidate.entries[0].base_value, -30.0);
        assert_eq!(candidate.entries[1].base_value, -4.0);
        assert_eq!(candidate.entries[0].adjustment, -4.0);
        assert_eq!(candidate.entries[1].adjustment, -3.5);
        assert_eq!(candidate.value, -34.0 - 7.5 - 0.5 - 0.75);
        assert_eq!(
            converter.candidate_path_value(&candidate, context).unwrap(),
            candidate.value
        );
        assert!(candidate.is_typo_correction);
        assert!(candidate.first_clause_candidate().is_none());
        assert_eq!(candidate.composing_count, ComposingCount::Input(4));
        let mut remaining = composing.clone();
        remaining.complete_prefix(candidate.composing_count, &tables);
        assert!(remaining.is_empty());
        let stored = memory
            .paths_from_start("ガクセイ", 128)
            .unwrap()
            .into_iter()
            .find(|path| path.len() == 2)
            .unwrap();
        assert_eq!(stored[0].adjustment, -0.5);
        assert_eq!(stored[1].adjustment, 0.0);
        assert_eq!(
            converter
                .convert_with_typo_correction(&composing, &tables, 1, context)
                .unwrap(),
            before
        );
    }

    #[test]
    fn personal_typo_recall_consumes_original_roman_input_and_requires_a_whole_reading() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let context = ConversionContext::default();
        remember(&memory, &[recalled_entry("記名", "キ", -20.0)]);
        let mut composing = ComposingText::new();
        composing.insert_str("li", InputStyle::RomanToKana, &tables);
        assert_eq!(composing.surface(), "ぃ");
        let candidates = converter
            .personal_typo_candidates(&composing, &tables, context, &memory, 1)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "記名");
        assert_eq!(candidates[0].value, -22.25);
        assert_eq!(candidates[0].ruby_count, 1);
        assert_eq!(candidates[0].composing_count, ComposingCount::Input(2));
        let mut remaining = composing.clone();
        remaining.complete_prefix(candidates[0].composing_count.clone(), &tables);
        assert!(remaining.is_empty());
        assert!(remaining.input().is_empty());
        assert!(
            converter
                .personal_typo_candidates(&composing, &tables, context, &memory, 0)
                .unwrap()
                .is_empty()
        );
        composing.insert_str("a", InputStyle::RomanToKana, &tables);
        assert!(
            converter
                .personal_typo_candidates(&composing, &tables, context, &memory, 4)
                .unwrap()
                .is_empty()
        );
        let long_input = self::composing(&"か".repeat(21), &tables);
        assert!(
            converter
                .personal_typo_candidates(&long_input, &tables, context, &memory, 4)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn personal_prediction_consumes_only_the_observed_roman_surface() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        remember(
            &memory,
            &[
                recalled_entry("東京", "トウキョウ", -4.0),
                recalled_entry("駅", "エキ", -3.0),
            ],
        );
        let mut composing = ComposingText::new();
        composing.insert_str("tou", InputStyle::RomanToKana, &tables);
        assert_eq!(composing.surface(), "とう");
        let candidates = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                ConversionContext::default(),
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: true,
                },
            )
            .unwrap();
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.text == "東京駅")
            .unwrap();
        assert_eq!(candidate.ruby_count, 7);
        assert_eq!(candidate.composing_count, ComposingCount::Surface(2));
        assert_eq!(candidate.entries.len(), 2);
        let first = candidate.first_clause_candidate().unwrap();
        assert_eq!(first.text, "東京");
        assert_eq!(first.composing_count, ComposingCount::Surface(2));
        for count in [candidate.composing_count.clone(), first.composing_count] {
            let mut remaining = composing.clone();
            remaining.complete_prefix(count, &tables);
            assert!(remaining.is_empty());
            assert!(remaining.input().is_empty());
        }
    }

    #[test]
    fn personal_occurrences_rebuild_memory_free_prefix_and_suffix_ranges() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let entries = vec![
            recalled_entry("個人", "カナ", -6.0),
            recalled_entry("句", "ニ", -4.0),
        ];
        remember(&memory, &entries);
        let composing = composing("あかなにい", &tables);
        let context = ConversionContext::default();
        let ordinary = converter
            .convert_with_entries(&composing, &tables, 8, context, &entries)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "あ個人句い")
            .unwrap();
        let recalled = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                context,
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: false,
                },
            )
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "あ個人句い")
            .unwrap();
        assert_eq!(recalled.value, ordinary.value);
        assert_eq!(recalled.composing_count, ComposingCount::Surface(5));
        assert_eq!(recalled.entries.len(), 4);
        assert_eq!(recalled.entries[1].ruby, "カナ");
        assert_eq!(
            recalled.first_clause_candidate().unwrap().composing_count,
            ComposingCount::Surface(1)
        );
    }

    #[test]
    fn personal_surroundings_preserve_explicit_dictionary_prefix_and_suffix() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let context = ConversionContext::default();
        let composing = composing("あかなにい", &tables);
        let additional = vec![
            DictionaryEntry {
                metadata: DictionaryMetadata::USER_DICTIONARY,
                ..recalled_entry("明示前", "ア", -1.0)
            },
            DictionaryEntry {
                metadata: DictionaryMetadata::USER_DICTIONARY,
                ..recalled_entry("明示後", "イ", -1.0)
            },
        ];
        let original = vec![
            recalled_entry("個人", "カナ", -30.0),
            recalled_entry("句", "ニ", -4.0),
        ];
        assert!(should_remove(&original[0]));
        let baseline = converter
            .convert_with_entries(&composing, &tables, 1, context, &additional)
            .unwrap();
        remember(&memory, &original);
        let recalled = converter
            .personal_candidates(
                &composing,
                &tables,
                &additional,
                context,
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: false,
                },
            )
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.text == "明示前個人句明示後")
            .unwrap();
        assert_eq!(recalled.entries.len(), 4);
        assert_eq!(recalled.entries[0], additional[0]);
        assert_eq!(recalled.entries[3], additional[1]);
        assert_eq!(recalled.entries[1].base_value, -30.0);
        assert_eq!(recalled.entries[1].adjustment, 0.0);
        assert_eq!(recalled.entries[2].base_value, -4.0);
        assert_eq!(recalled.value, -38.75);
        assert_eq!(
            recalled.value,
            converter.candidate_path_value(&recalled, context).unwrap()
        );
        assert_eq!(recalled.composing_count, ComposingCount::Surface(5));
        let first = recalled.first_clause_candidate().unwrap();
        assert_eq!(first.text, "明示前");
        assert_eq!(first.composing_count, ComposingCount::Surface(1));
        assert!(
            converter
                .personal_candidates(
                    &composing,
                    &tables,
                    &[],
                    context,
                    &memory,
                    PersonalRecallOptions {
                        maximum: 4,
                        prediction: false
                    }
                )
                .unwrap()
                .iter()
                .all(|candidate| candidate.text != recalled.text)
        );
        assert_eq!(
            converter
                .convert_with_entries(&composing, &tables, 1, context, &additional)
                .unwrap(),
            baseline
        );
    }

    #[test]
    fn personal_recall_skips_persisted_invalid_boundary_ids() {
        let fixture = PersonalFixture::new();
        let dictionary = fixture.dictionary();
        let converter = NormalConverter::new(&dictionary);
        let memory = fixture.memory(&dictionary);
        let tables = InputTableRegistry::new();
        let composing = composing("かな", &tables);
        remember(&memory, &[recalled_entry("有効", "カナ", -3.0)]);
        remember(
            &memory,
            &[DictionaryEntry {
                left_id: u16::MAX,
                ..recalled_entry("破損", "カナ", -3.0)
            }],
        );
        memory.commit().unwrap();
        let memory = fixture.memory(&dictionary);
        let before = converter.convert(&composing, &tables, 1).unwrap();
        let candidates = converter
            .personal_candidates(
                &composing,
                &tables,
                &[],
                ConversionContext::default(),
                &memory,
                PersonalRecallOptions {
                    maximum: 4,
                    prediction: false,
                },
            )
            .unwrap();
        assert!(candidates.iter().any(|candidate| candidate.text == "有効"));
        assert!(candidates.iter().all(|candidate| candidate.text != "破損"));
        assert_eq!(converter.convert(&composing, &tables, 1).unwrap(), before);
    }

    #[test]
    fn personal_dedup_retains_different_consumed_spans_and_rejects_invalid_boundaries() {
        let candidate =
            Candidate::single("同じ".into(), -2.0, ComposingCount::Surface(1), 501, vec![]);
        let mut different_span = candidate.clone();
        different_span.composing_count = ComposingCount::Surface(2);
        let mut candidates = Vec::new();
        push_personal_candidate(&mut candidates, candidate.clone(), 4);
        push_personal_candidate(&mut candidates, different_span, 4);
        push_personal_candidate(&mut candidates, candidate, 4);
        assert_eq!(candidates.len(), 2);
        for invalid in [
            DictionaryEntry {
                left_id: 1319,
                ..entry("不正", -3.0)
            },
            DictionaryEntry {
                right_id: u16::MAX,
                ..entry("不正", -3.0)
            },
            DictionaryEntry {
                meaning_id: 502,
                ..entry("不正", -3.0)
            },
            DictionaryEntry {
                base_value: f32::NAN,
                ..entry("不正", -3.0)
            },
        ] {
            assert_eq!(personal_path_length(&[invalid]), None);
        }
    }

    #[test]
    fn katakana_conversion_matches_the_fixed_upstream_range() {
        assert_eq!(to_katakana("ぁあゖー漢"), "ァアヶー漢");
    }

    #[test]
    fn predecessor_order_is_descending_and_stable_for_ties() {
        let mut values = Vec::new();
        for total in [-2.0, -1.0, -1.0, -3.0, 0.0] {
            insert_predecessor(&mut values, Predecessor { path: None, total }, 4);
        }
        assert_eq!(
            values.iter().map(|value| value.total).collect::<Vec<_>>(),
            vec![0.0, -1.0, -1.0, -2.0]
        );
    }

    #[test]
    fn dictionary_threshold_uses_the_upstream_word_length_penalty() {
        assert!(!should_remove(&entry("長い語", -16.0)));
        assert!(should_remove(&entry("語", -16.0)));
        assert!(should_remove(&entry("語", -18.0)));
    }

    #[test]
    fn dual_indices_keep_roman_input_and_surface_positions_in_order() {
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str("kya", InputStyle::RomanToKana, &tables);
        let map = DualIndexMap::new(&composing, &tables);
        let indices = map.indices(composing.input().len(), composing.surface_graphemes().len());
        assert_eq!(indices.first().and_then(|index| index.input()), Some(0));
        assert_eq!(indices.first().and_then(|index| index.surface()), Some(0));
        assert!(indices.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn converts_optional_width_representations() {
        assert_eq!(to_full_width("ABC 123 ¥\\"), "ＡＢＣ　１２３　￥＼");
        assert_eq!(to_half_width("ガッツポーズ"), "ｶﾞｯﾂﾎﾟｰｽﾞ");
    }
}
