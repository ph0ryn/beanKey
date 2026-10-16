use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use super::persistence::{STORE_FILE, TEMPORARY_FILE};
use super::*;
use crate::ComposingCount;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "beankey-personalization-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        assert!(self.0.is_absolute());
        assert!(self.0.starts_with(std::env::temp_dir()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("beankey-personalization-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct WriteGate {
    pub(super) captured: Barrier,
    pub(super) proceed: Barrier,
}

fn characters() -> CharacterIdMap {
    CharacterIdMap::parse("\0アイウエオカキクケコガギグゲゴサシスセソザジズゼゾタチツテトダヂヅデドナニヌネノハヒフヘホバビブベボパピプペポマミムメモヤユヨラリルレロワヲンァィゥェォャュョッヴーABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789。 ").unwrap()
}

fn open(directory: &Directory, mode: LearningMode, count: usize) -> LearningMemory {
    LearningMemory::open(&directory.0, mode, count, characters()).unwrap()
}

fn entry(word: &str, ruby: &str) -> DictionaryEntry {
    DictionaryEntry {
        word: word.into(),
        ruby: ruby.into(),
        left_id: 1285,
        right_id: 1285,
        meaning_id: 501,
        base_value: -12.0,
        adjustment: -0.25,
        metadata: DictionaryMetadata::default(),
    }
}

fn path_candidate(entries: Vec<DictionaryEntry>) -> Candidate {
    let text = entries.iter().map(|entry| entry.word.as_str()).collect();
    let count = entries
        .iter()
        .map(|entry| UnicodeSegmentation::graphemes(entry.ruby.as_str(), true).count())
        .sum();
    Candidate::single(text, -40.0, ComposingCount::Surface(count), 501, entries)
}

fn candidate(text: &str) -> Candidate {
    path_candidate(vec![entry(text, "カナ")])
}

fn scope() -> LearningScope {
    LearningScope::new("かな", LearningTask::Conversion, "左文脈")
}

fn correct(
    memory: &LearningMemory,
    scope: &LearningScope,
    from: &Candidate,
    to: &Candidate,
    times: usize,
) {
    for _ in 0..times {
        memory.record_correction(scope, from, to).unwrap();
    }
}

fn challenge(
    memory: &LearningMemory,
    from: &Candidate,
    to: &Candidate,
) -> Option<PreferenceChallenge> {
    memory.challenge(&scope(), from, to).unwrap()
}

#[test]
fn ordinary_accepts_only_recall_without_creating_or_refreshing_evidence() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("仮名"), candidate("佳奈"));
    assert_eq!(challenge(&memory, &a, &b), None);
    for _ in 0..100 {
        memory.learn(&b).unwrap();
    }
    assert_eq!(challenge(&memory, &a, &b), None);
    correct(&memory, &scope(), &a, &b, 1);
    let before = challenge(&memory, &a, &b).unwrap();
    for _ in 0..100 {
        memory.learn(&b).unwrap();
        memory.learn(&a).unwrap();
    }
    assert_eq!(challenge(&memory, &a, &b), Some(before));
    let learned = memory.exact_match("かな").unwrap();
    assert_eq!(learned.len(), 2);
    assert!(
        learned
            .iter()
            .all(|entry| entry.base_value == -12.0 && entry.adjustment == -0.25)
    );
    assert!(
        learned
            .iter()
            .all(|entry| entry.metadata.contains(DictionaryMetadata::LEARNED))
    );
}

#[test]
fn typo_query_penalties_never_create_or_overwrite_original_recall_costs() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let original = candidate("通常語");
    memory.learn(&original).unwrap();
    let expected: Vec<_> = original.entries.iter().map(recalled_entry).collect();
    let mut typo = original.clone();
    typo.is_typo_correction = true;
    typo.entries[0].adjustment -= 8.0;
    let mut unknown = candidate("未知語");
    unknown.is_typo_correction = true;
    unknown.entries[0].adjustment -= 11.0;
    for _ in 0..8 {
        memory.learn(&typo).unwrap();
        memory.learn(&unknown).unwrap();
    }
    let prediction = PostCompositionPrediction {
        text: "追加".into(),
        value: -20.0,
        kind: PostPredictionKind::Additional {
            entries: vec![entry("追加", "ツイカ")],
        },
        is_terminal: false,
    };
    memory.learn_post_prediction(&typo, &prediction).unwrap();
    memory.learn_post_prediction(&unknown, &prediction).unwrap();
    correct(&memory, &scope(), &candidate("既定"), &typo, 3);
    assert_eq!(memory.entries().unwrap(), expected);
    assert_eq!(
        memory.paths_from_start("カナツイカ", 5).unwrap(),
        vec![expected.clone()]
    );
    assert!(memory.lock().unwrap().store.comparisons.is_empty());
    memory.commit().unwrap();
    drop(memory);
    let reopened = open(&directory, LearningMode::OnlyOutput, 128);
    assert_eq!(reopened.entries().unwrap(), expected);
}

#[test]
fn opposite_correction_cancels_exactly_and_support_is_capped_but_adapts() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("仮名"), candidate("佳奈"));
    correct(&memory, &scope(), &a, &b, 1);
    correct(&memory, &scope(), &b, &a, 1);
    assert_eq!(challenge(&memory, &a, &b), None);
    assert_eq!(challenge(&memory, &b, &a), None);
    assert!(memory.lock().unwrap().store.comparisons.is_empty());
    correct(&memory, &scope(), &a, &b, 100);
    let capped = challenge(&memory, &a, &b).unwrap();
    assert_eq!(capped.confidence, 8.0 / 10.0);
    correct(&memory, &scope(), &b, &a, 8);
    assert_eq!(challenge(&memory, &a, &b), None);
    correct(&memory, &scope(), &b, &a, 3);
    assert!(challenge(&memory, &b, &a).unwrap().wins_without_model());
}

#[test]
fn three_direct_supports_overcome_arbitrary_base_odds_without_a_score_cap() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("仮名"), candidate("稀な姓名"));
    correct(&memory, &scope(), &a, &b, 1);
    assert!(!challenge(&memory, &a, &b).unwrap().wins_without_model());
    correct(&memory, &scope(), &a, &b, 1);
    assert!(!challenge(&memory, &a, &b).unwrap().wins_without_model());
    correct(&memory, &scope(), &a, &b, 1);
    let preference = challenge(&memory, &a, &b).unwrap();
    assert!(preference.wins_without_model());
    assert!(preference.margin(0.0, 1.0) > 0.0);
    assert_eq!(preference.confidence, 0.6);
}

#[test]
fn unrelated_support_cannot_inflate_confidence_and_absent_mass_is_retained() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b, c, d) = (
        candidate("既定"),
        candidate("修正"),
        candidate("別既定"),
        candidate("別修正"),
    );
    correct(&memory, &scope(), &a, &b, 3);
    let before = challenge(&memory, &a, &b).unwrap();
    correct(&memory, &scope(), &c, &d, 100);
    let after = challenge(&memory, &a, &b).unwrap();
    assert_eq!(before.confidence, after.confidence);
    assert_eq!(after.preferred_probability, 3.0 / 11.0);
    assert_eq!(after.anchor_probability, 0.0);
    assert!(!after.wins_without_model());
    // Querying a different shortlist/anchor does not renormalize away D's mass.
    assert_eq!(
        memory
            .challenge(&scope(), &c, &b)
            .unwrap()
            .unwrap()
            .preferred_probability,
        3.0 / 11.0
    );
}

#[test]
fn balanced_cycles_and_ties_fall_back_conservatively() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b, c, d) = (
        candidate("甲"),
        candidate("乙"),
        candidate("丙"),
        candidate("丁"),
    );
    correct(&memory, &scope(), &a, &b, 3);
    correct(&memory, &scope(), &b, &c, 3);
    correct(&memory, &scope(), &c, &a, 3);
    for (from, to) in [(&a, &b), (&b, &c), (&c, &a)] {
        assert_eq!(challenge(&memory, from, to), None);
    }
    correct(&memory, &scope(), &a, &b, 1);
    correct(&memory, &scope(), &c, &d, 1);
    assert_eq!(challenge(&memory, &b, &d), None);
}

#[test]
fn context_task_reading_and_action_are_isolated_without_narrow_to_broad_leakage() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("仮名"), candidate("佳奈"));
    let narrow = scope().with_action("span=かな;fixed=甲;segment=全体");
    correct(&memory, &narrow, &a, &b, 8);
    assert!(
        memory
            .challenge(&narrow, &a, &b)
            .unwrap()
            .unwrap()
            .wins_without_model()
    );
    let others = [
        scope(),
        scope().with_action("span=かな;fixed=乙;segment=全体"),
        LearningScope::new("カナ", LearningTask::Conversion, "別の文脈")
            .with_action(narrow.action.clone()),
        LearningScope::new("カナ", LearningTask::Conversion, "").with_action(narrow.action.clone()),
        LearningScope::new("カナ", LearningTask::Prediction, "左文脈")
            .with_action(narrow.action.clone()),
        LearningScope::new("カ", LearningTask::Conversion, "左文脈")
            .with_action(narrow.action.clone()),
    ];
    for other in others {
        assert_eq!(memory.challenge(&other, &a, &b).unwrap(), None);
    }
    // Scope reading is observed input: it need not equal completed entries.
    let observed = LearningScope::new("か", LearningTask::Prediction, "");
    correct(&memory, &observed, &a, &b, 3);
    assert!(
        memory
            .challenge(&observed, &a, &b)
            .unwrap()
            .unwrap()
            .wins_without_model()
    );
}

#[test]
fn scope_normalizes_kana_bounds_graphemes_and_does_not_truncate_action_into_collision() {
    let context = format!("{}か\u{3099}", "前".repeat(100));
    let value = LearningScope::new("かな", LearningTask::PartialConversion, &context);
    assert_eq!(value.reading, "カナ");
    assert_eq!(
        UnicodeSegmentation::graphemes(value.context.as_str(), true).count(),
        32
    );
    assert!(value.context.ends_with("か\u{3099}"));
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let oversized = value.with_action("a".repeat(MAX_ACTION_BYTES + 1));
    correct(&memory, &oversized, &candidate("甲"), &candidate("乙"), 8);
    assert!(memory.lock().unwrap().store.comparisons.is_empty());
}

#[test]
fn multiword_recall_preserves_original_entries_internal_costs_and_metadata() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let mut first = entry("学", "ガク");
    first.base_value = -18.75;
    first.adjustment = 1.25;
    first.metadata.insert(DictionaryMetadata::USER_DICTIONARY);
    let mut second = entry("生", "セイ");
    second.left_id = 501;
    second.right_id = 502;
    second.base_value = -30.125;
    let candidate = path_candidate(vec![first, second]);
    memory.learn(&candidate).unwrap();
    for persisted in [false, true] {
        if persisted {
            memory.commit().unwrap();
        }
        let paths = memory.paths_from_start("がくせいは", 4).unwrap();
        let full = paths.iter().find(|path| path.len() == 2).unwrap();
        let expected: Vec<_> = candidate.entries.iter().map(recalled_entry).collect();
        assert_eq!(*full, expected);
        assert!(
            !memory
                .entries()
                .unwrap()
                .iter()
                .any(|entry| entry.word == "学生")
        );
        assert!(
            !memory
                .exact_match("ガクセイ")
                .unwrap()
                .iter()
                .any(|entry| entry.word == "学生")
        );
        assert!(
            !memory
                .matches_from_start("ガクセイ", 4)
                .unwrap()
                .iter()
                .any(|entry| entry.word == "学生")
        );
        assert!(
            !memory
                .predict("ガク")
                .unwrap()
                .iter()
                .any(|entry| entry.word == "学生")
        );
        assert!(
            memory
                .predictive_paths("がく", 4)
                .unwrap()
                .iter()
                .any(|path| path.len() == 2)
        );
        assert!(
            memory
                .paths_from_start("ガクセイ", 3)
                .unwrap()
                .iter()
                .all(|path| path.len() == 1)
        );
    }
    drop(memory);
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    let expected: Vec<_> = candidate.entries.iter().map(recalled_entry).collect();
    assert!(
        memory
            .paths_from_start("ガクセイ", 4)
            .unwrap()
            .contains(&expected)
    );
}

#[test]
fn post_prediction_recalls_only_new_parts_and_joined_original_path() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let base = path_candidate(vec![entry("今日", "キョウ")]);
    let prediction = PostCompositionPrediction {
        text: "は".into(),
        value: -13.0,
        kind: PostPredictionKind::Additional {
            entries: vec![entry("は", "ハ")],
        },
        is_terminal: false,
    };
    memory.learn_post_prediction(&base, &prediction).unwrap();
    assert!(
        memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "は")
    );
    assert!(
        !memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "今日")
    );
    assert!(
        memory
            .paths_from_start("キョウハ", 4)
            .unwrap()
            .iter()
            .any(|path| path.len() == 2)
    );
    assert_eq!(challenge(&memory, &candidate("甲"), &candidate("乙")), None);
}

#[test]
fn recall_is_canonical_bounded_and_never_evicts_or_refreshes_rank_evidence() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 2);
    let (a, b) = (candidate("既定"), candidate("選択"));
    correct(&memory, &scope(), &a, &b, 3);
    let before = challenge(&memory, &a, &b);
    for number in 0..20 {
        memory.learn(&candidate(&format!("語{number}"))).unwrap();
    }
    memory.learn(&candidate("語19")).unwrap();
    let entries = memory.entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].word, "語19");
    assert_eq!(challenge(&memory, &a, &b), before);
    for number in 0..20 {
        correct(
            &memory,
            &scope(),
            &candidate(&format!("既定{number}")),
            &candidate(&format!("選択{number}")),
            1,
        );
    }
    assert_eq!(memory.lock().unwrap().store.comparisons.len(), 2);
    memory.set_max_count(1).unwrap();
    assert_eq!(memory.entries().unwrap().len(), 1);
    assert_eq!(memory.lock().unwrap().store.comparisons.len(), 1);
    memory.set_max_count(0).unwrap();
    memory.learn(&a).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    assert!(memory.entries().unwrap().is_empty());
    assert_eq!(challenge(&memory, &a, &b), None);
    memory.commit().unwrap();
    drop(memory);
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(memory.entries().unwrap().is_empty());
}

#[test]
fn query_path_output_is_bounded_and_exact_predictions_are_consistent() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 512);
    for number in 0..100 {
        memory.learn(&candidate(&format!("語{number}"))).unwrap();
    }
    assert_eq!(
        memory.paths_from_start("カナ", 2).unwrap().len(),
        MAX_LOOKUP_PATHS
    );
    assert_eq!(
        memory.predictive_paths("カ", 2).unwrap().len(),
        MAX_LOOKUP_PATHS
    );
    assert!(memory.predict("カナ").unwrap().is_empty());
    assert!(memory.predictive_paths("カナ", 2).unwrap().is_empty());
    memory.commit().unwrap();
    assert!(memory.predict("カナ").unwrap().is_empty());
    assert!(memory.predictive_paths("カ", 1).unwrap().is_empty());
}

#[test]
fn persistence_reopens_recall_and_real_corrections_but_never_old_frequency_files() {
    let directory = Directory::new();
    let old_path = directory.0.join("memory.memorymetadata");
    let old_shard = directory.0.join("memory0.loudstxt3");
    fs::write(&old_path, b"old frequency statistics stay untouched").unwrap();
    fs::write(&old_shard, b"old synthetic entries stay untouched").unwrap();
    fs::write(directory.0.join(".pause"), b"old partial-save marker").unwrap();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("既定"), candidate("修正"));
    memory.learn(&b).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    assert!(memory.commit().unwrap());
    assert!(!memory.commit().unwrap());
    let before = challenge(&memory, &a, &b);
    drop(memory);
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    assert_eq!(challenge(&memory, &a, &b), before);
    assert!(
        memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "修正")
    );
    assert_eq!(
        fs::read(old_path).unwrap(),
        b"old frequency statistics stay untouched"
    );
    assert_eq!(
        fs::read(old_shard).unwrap(),
        b"old synthetic entries stay untouched"
    );
}

#[test]
fn readonly_and_disabled_modes_do_not_write_and_nothing_does_not_read() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("既定"), candidate("修正"));
    memory.learn(&b).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    memory.commit().unwrap();
    drop(memory);
    let path = directory.0.join(STORE_FILE);
    let original = fs::read(&path).unwrap();
    for mode in [LearningMode::OnlyOutput, LearningMode::Nothing] {
        let memory = open(&directory, mode, 128);
        if mode == LearningMode::OnlyOutput {
            assert!(!memory.entries().unwrap().is_empty());
            assert!(challenge(&memory, &a, &b).is_some());
        } else {
            assert!(memory.entries().unwrap().is_empty());
            assert_eq!(challenge(&memory, &a, &b), None);
        }
        memory.learn(&a).unwrap();
        correct(&memory, &scope(), &b, &a, 8);
        memory.forget(&b).unwrap();
        memory.reset().unwrap();
        assert!(!memory.commit().unwrap());
        drop(memory);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    fs::write(&path, b"broken new-format data").unwrap();
    let memory = open(&directory, LearningMode::Nothing, 128);
    assert!(memory.entries().unwrap().is_empty());
    drop(memory);
    assert!(
        LearningMemory::open(&directory.0, LearningMode::OnlyOutput, 128, characters()).is_err()
    );
    let absent = directory.0.join("not-created");
    drop(LearningMemory::open(&absent, LearningMode::OnlyOutput, 128, characters()).unwrap());
    assert!(!absent.exists());
}

#[test]
fn changing_modes_loads_lazily_and_drains_old_writes_before_readonly_returns() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::Nothing, 128);
    let (a, b) = (candidate("既定"), candidate("修正"));
    memory.learn(&b).unwrap();
    assert!(!directory.0.join(STORE_FILE).exists());
    memory.set_mode(LearningMode::InputAndOutput).unwrap();
    memory.learn(&b).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    memory.set_mode(LearningMode::OnlyOutput).unwrap();
    let original = fs::read(directory.0.join(STORE_FILE)).unwrap();
    memory.learn(&a).unwrap();
    correct(&memory, &scope(), &b, &a, 8);
    memory.set_mode(LearningMode::Nothing).unwrap();
    assert!(memory.entries().unwrap().is_empty());
    memory.set_mode(LearningMode::OnlyOutput).unwrap();
    assert!(challenge(&memory, &a, &b).unwrap().wins_without_model());
    drop(memory);
    assert_eq!(fs::read(directory.0.join(STORE_FILE)).unwrap(), original);
}

#[test]
fn failed_mode_switch_keeps_dirty_updates_explicitly_writable_for_retry() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    memory.learn(&candidate("保存済み")).unwrap();
    memory.commit().unwrap();
    let path = directory.0.join(STORE_FILE);
    let original = fs::read(&path).unwrap();
    let blocked = directory.0.join(TEMPORARY_FILE);
    fs::create_dir(&blocked).unwrap();
    memory.learn(&candidate("未保存")).unwrap();
    assert!(memory.set_mode(LearningMode::OnlyOutput).is_err());
    assert_eq!(memory.lock().unwrap().mode, LearningMode::InputAndOutput);
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::remove_dir(&blocked).unwrap();
    memory.set_mode(LearningMode::OnlyOutput).unwrap();
    assert_eq!(memory.lock().unwrap().mode, LearningMode::OnlyOutput);
    let saved = fs::read(&path).unwrap();
    drop(memory);
    assert_eq!(fs::read(&path).unwrap(), saved);
    let reopened = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(
        reopened
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "未保存")
    );
}

#[test]
fn forget_removes_winner_loser_and_phrase_projections_across_contexts_and_readings() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b, c, d, e) = (
        candidate("既定"),
        candidate("対象"),
        candidate("他修正"),
        candidate("残既定"),
        candidate("残修正"),
    );
    let other_reading = path_candidate(vec![entry("対象", "カニ")]);
    let phrase = path_candidate(vec![
        entry("前", "マエ"),
        entry("対", "カ"),
        entry("象", "ナ"),
    ]);
    memory.learn(&b).unwrap();
    memory.learn(&other_reading).unwrap();
    memory.learn(&phrase).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    correct(
        &memory,
        &LearningScope::new("カニ", LearningTask::Prediction, "別"),
        &other_reading,
        &c,
        3,
    );
    correct(&memory, &scope(), &phrase, &c, 3);
    correct(&memory, &scope(), &d, &e, 3);
    memory.forget(&b).unwrap();
    assert!(
        memory
            .paths_from_start("マエカナ", 5)
            .unwrap()
            .iter()
            .all(|path| {
                !path
                    .iter()
                    .map(|entry| entry.word.as_str())
                    .collect::<String>()
                    .contains("対象")
            })
    );
    assert!(
        !memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "対象")
    );
    assert_eq!(challenge(&memory, &a, &b), None);
    assert_eq!(challenge(&memory, &a, &c), None);
    assert!(challenge(&memory, &d, &e).unwrap().wins_without_model());
    assert_eq!(memory.lock().unwrap().store.comparisons.len(), 1);
    drop(memory);
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    assert_eq!(memory.lock().unwrap().store.comparisons.len(), 1);
    assert!(
        !memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "対象")
    );
}

#[test]
fn queued_and_inflight_snapshots_cannot_resurrect_forget_or_reset() {
    for reset in [false, true] {
        let directory = Directory::new();
        let memory = open(&directory, LearningMode::InputAndOutput, 128);
        let (a, b) = (candidate("既定"), candidate("修正"));
        let gate = Arc::new(WriteGate {
            captured: Barrier::new(2),
            proceed: Barrier::new(2),
        });
        *memory.inner.runtime.write_gate.lock().unwrap() = Some(Arc::clone(&gate));
        memory.learn(&b).unwrap();
        correct(&memory, &scope(), &a, &b, 3);
        // Hold the writer after taking an old snapshot but before publication.
        gate.captured.wait();
        let previous = memory.lock().unwrap().revision;
        let deletion = memory.clone();
        let deleted = b.clone();
        let thread = std::thread::spawn(move || {
            if reset {
                deletion.reset()
            } else {
                deletion.forget(&deleted)
            }
        });
        {
            let runtime = &memory.inner.runtime;
            let mut state = runtime.state.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while state.revision == previous {
                let timeout = deadline.saturating_duration_since(Instant::now());
                assert!(!timeout.is_zero(), "deletion did not reach its barrier");
                (state, _) = runtime.changed.wait_timeout(state, timeout).unwrap();
            }
        }
        gate.proceed.wait();
        thread.join().unwrap().unwrap();
        assert!(memory.entries().unwrap().is_empty());
        assert_eq!(challenge(&memory, &a, &b), None);
        drop(memory);
        let memory = open(&directory, LearningMode::OnlyOutput, 128);
        assert!(memory.entries().unwrap().is_empty());
        assert_eq!(challenge(&memory, &a, &b), None);
    }
}

#[test]
fn background_save_and_last_shared_drop_flush_without_explicit_commit() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let other = memory.clone();
    memory.learn(&candidate("背景保存")).unwrap();
    {
        let runtime = &memory.inner.runtime;
        let mut state = runtime.state.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.durable_revision < state.revision {
            let timeout = deadline.saturating_duration_since(Instant::now());
            assert!(!timeout.is_zero(), "background writer did not save");
            (state, _) = runtime.changed.wait_timeout(state, timeout).unwrap();
        }
    }
    assert!(directory.0.join(STORE_FILE).is_file());
    drop(memory);
    other.learn(&candidate("終了保存")).unwrap();
    drop(other);
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(
        memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "終了保存")
    );
}

#[test]
fn write_failure_does_not_corrupt_last_good_file_and_input_updates_stay_nonfatal() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("既定"), candidate("修正"));
    memory.learn(&a).unwrap();
    memory.commit().unwrap();
    let path = directory.0.join(STORE_FILE);
    let previous = fs::read(&path).unwrap();
    let blocked = directory.0.join(TEMPORARY_FILE);
    fs::create_dir(&blocked).unwrap();
    memory.learn(&b).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    assert!(
        memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "修正")
    );
    assert!(memory.commit().is_err());
    assert_eq!(fs::read(&path).unwrap(), previous);
    let reopened = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(
        reopened
            .entries()
            .unwrap()
            .iter()
            .all(|entry| entry.word != "修正")
    );
    assert_eq!(challenge(&reopened, &a, &b), None);
    drop(reopened);
    fs::remove_dir(&blocked).unwrap();
    assert!(memory.commit().unwrap());
    drop(memory);
    let reopened = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(challenge(&reopened, &a, &b).unwrap().wins_without_model());
}

#[test]
fn reset_does_not_recover_stale_temporary_or_old_files() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    let (a, b) = (candidate("既定"), candidate("修正"));
    memory.learn(&b).unwrap();
    correct(&memory, &scope(), &a, &b, 3);
    memory.commit().unwrap();
    let stale = fs::read(directory.0.join(STORE_FILE)).unwrap();
    memory.reset().unwrap();
    drop(memory);
    fs::write(directory.0.join(TEMPORARY_FILE), stale).unwrap();
    fs::write(directory.0.join("memory0.loudstxt3"), b"old recall").unwrap();
    let memory = open(&directory, LearningMode::OnlyOutput, 128);
    assert!(memory.entries().unwrap().is_empty());
    assert_eq!(challenge(&memory, &a, &b), None);
}

#[test]
fn invalid_and_oversized_paths_are_not_learned_or_serialized() {
    let directory = Directory::new();
    let memory = open(&directory, LearningMode::InputAndOutput, 128);
    for bad in [
        path_candidate(vec![entry("x\0y", "カナ")]),
        path_candidate(vec![entry(&"x".repeat(MAX_TEXT_BYTES + 1), "カナ")]),
        path_candidate(vec![entry("語", "☃")]),
        path_candidate(vec![entry("語", "カ"); MAX_PATH_ENTRIES + 1]),
    ] {
        memory.learn(&bad).unwrap();
        correct(&memory, &scope(), &candidate("既定"), &bad, 3);
    }
    let mut nonfinite = candidate("非有限");
    nonfinite.entries[0].base_value = f32::NAN;
    memory.learn(&nonfinite).unwrap();
    assert!(memory.entries().unwrap().is_empty());
    assert!(memory.lock().unwrap().store.comparisons.is_empty());
}
