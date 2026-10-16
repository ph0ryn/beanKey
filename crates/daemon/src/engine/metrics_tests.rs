//! Observe public protocol responses and persisted records; private setup only selects config.
use std::fs;

use serde_json::Value;
use tempfile::TempDir;

use super::*;

fn engine(directory: &Path, live: bool, mode: LearningModeConfig) -> Engine {
    let dictionary = PathBuf::from(std::env::var_os("BEANKEY_TEST_DICTIONARY").unwrap());
    let mut engine = Engine::open(dictionary).unwrap();
    engine
        .apply_conversion_options(&ConversionConfig {
            live_conversion: live,
            ..Default::default()
        })
        .unwrap();
    engine
        .load_learning(
            directory,
            &LearningConfig {
                mode,
                ..Default::default()
            },
        )
        .unwrap();
    engine
}

fn state(engine: &mut Engine, payload: Payload) -> protocol::StateResponse {
    static NEXT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let response = engine.handle(protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: NEXT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        session_id: "private-public-session-id".into(),
        payload: Some(payload),
        trace: Vec::new(),
    });
    let Some(Payload::StateResponse(state)) = response.payload else {
        panic!("unexpected protocol response: {response:?}")
    };
    state
}

fn start(engine: &mut Engine) {
    state(
        engine,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            ..Default::default()
        }),
    );
}

fn key(engine: &mut Engine, action: protocol::UserAction, text: &str) -> protocol::StateResponse {
    state(
        engine,
        Payload::KeyEvent(protocol::KeyEvent {
            action: action as i32,
            text: text.into(),
            input: text.into(),
            intention: text.into(),
            ..Default::default()
        }),
    )
}

fn events(directory: &Path) -> Vec<Value> {
    let mut paths: Vec<_> = fs::read_dir(directory.join("metrics-v1"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| {
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| {
                    let record: Value = serde_json::from_str(line).unwrap();
                    record["record"].clone()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn commits(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter_map(|record| {
            let event = &record["event"];
            (event["kind"] == "commit").then_some(event)
        })
        .collect()
}

#[test]
fn metrics_separate_live_acceptance_from_raw_nonlive_enter_and_hidden_live_preedit() {
    for (live, hide, expected_origin) in [
        (true, None, "candidate"),
        (false, None, "raw"),
        (true, Some(protocol::UserAction::Backspace), "raw"),
        (true, Some(protocol::UserAction::Escape), "raw"),
    ] {
        let directory = TempDir::new().unwrap();
        let mut engine = engine(directory.path(), live, LearningModeConfig::InputAndOutput);
        start(&mut engine);
        let shown = key(&mut engine, protocol::UserAction::Input, "shikai");
        assert!(!shown.preedit.is_empty());
        if let Some(action) = hide {
            if action == protocol::UserAction::Escape {
                key(&mut engine, protocol::UserAction::Down, "");
            }
            key(&mut engine, action, "");
        }
        let accepted = key(&mut engine, protocol::UserAction::Enter, "");
        assert!(!accepted.commit.is_empty());
        drop(engine);
        let records = events(directory.path());
        let commits = commits(&records);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["origin"], expected_origin);
        assert_eq!(
            commits[0]["accepted_chars"],
            accepted.commit.chars().count()
        );
        let comparison = &commits[0]["comparison"];
        if expected_origin == "candidate" {
            assert_eq!(comparison["status"], "comparable");
            assert_eq!(comparison["default_matches_selection"], true);
        } else {
            assert_eq!(comparison["status"], "no_presentation");
            assert!(comparison["default_matches_selection"].is_null());
        }
    }
}

#[test]
fn metrics_measure_live_remainder_preview_and_forced_partial_commit_once() {
    for forced in [false, true] {
        let directory = TempDir::new().unwrap();
        let mut engine = engine(directory.path(), true, LearningModeConfig::InputAndOutput);
        start(&mut engine);
        key(&mut engine, protocol::UserAction::Input, "kyouhaiitenki");
        let selecting = key(&mut engine, protocol::UserAction::Down, "");
        assert_eq!(
            selecting.input_state,
            protocol::InputState::Selecting as i32
        );
        if forced {
            let accepted = state(
                &mut engine,
                Payload::CommitComposition(protocol::CommitComposition {}),
            );
            assert!(!accepted.commit.is_empty());
        } else {
            let prefix = key(&mut engine, protocol::UserAction::Enter, "");
            assert!(!prefix.commit.is_empty());
            assert_eq!(prefix.input_state, protocol::InputState::Previewing as i32);
            assert!(!prefix.preedit.is_empty());
            let remainder = key(&mut engine, protocol::UserAction::Enter, "");
            assert!(!remainder.commit.is_empty());
        }
        drop(engine);
        let records = events(directory.path());
        let commits = commits(&records);
        assert_eq!(commits.len(), if forced { 1 } else { 2 });
        for commit in &commits {
            assert_eq!(
                commit["comparison"]["default_matches_selection"], true,
                "{commit}"
            );
            assert_eq!(commit["comparison"]["status"], "comparable", "{commit}");
        }
        if forced {
            assert_eq!(commits[0]["origin"], "candidate_and_raw");
            assert!(
                commits[0]["accepted_chars"].as_u64().unwrap()
                    > commits[0]["candidate_chars"].as_u64().unwrap()
            );
        }
    }
}

#[test]
fn metrics_reanchor_the_actual_preview_after_reveal_and_escape() {
    let directory = TempDir::new().unwrap();
    let mut engine = engine(directory.path(), false, LearningModeConfig::InputAndOutput);
    start(&mut engine);
    key(&mut engine, protocol::UserAction::Input, "shikai");
    key(&mut engine, protocol::UserAction::Down, "");
    for _ in 0..5 {
        key(&mut engine, protocol::UserAction::Up, "");
    }
    let preview = key(&mut engine, protocol::UserAction::Escape, "");
    assert_eq!(preview.input_state, protocol::InputState::Previewing as i32);
    let accepted = key(&mut engine, protocol::UserAction::Enter, "");
    assert_eq!(accepted.commit, preview.preedit);
    drop(engine);
    let records = events(directory.path());
    let commits = commits(&records);
    assert_eq!(commits.len(), 1);
    assert_eq!(
        commits[0]["comparison"]["status"],
        "baseline_different_scope"
    );
    assert_eq!(commits[0]["comparison"]["default_matches_selection"], true);
    assert!(commits[0]["comparison"]["baseline_matches_selection"].is_null());
}

#[test]
fn metrics_distinguish_unicode_direct_input_and_raw_switching() {
    let directory = TempDir::new().unwrap();
    let mut engine = engine(directory.path(), false, LearningModeConfig::InputAndOutput);
    start(&mut engine);
    key(&mut engine, protocol::UserAction::Input, "shikai");
    assert_eq!(
        key(&mut engine, protocol::UserAction::Enter, "").commit,
        "しかい"
    );
    key(&mut engine, protocol::UserAction::Eisu, "");
    assert_eq!(
        key(&mut engine, protocol::UserAction::Input, "a").commit,
        "a"
    );
    key(&mut engine, protocol::UserAction::Kana, "");
    key(&mut engine, protocol::UserAction::StartUnicodeInput, "");
    key(&mut engine, protocol::UserAction::Input, "1f600");
    assert_eq!(
        key(&mut engine, protocol::UserAction::Enter, "").commit,
        "😀"
    );
    drop(engine);
    let records = events(directory.path());
    let commits = commits(&records);
    assert_eq!(commits.len(), 3);
    assert_eq!(
        commits
            .iter()
            .map(|event| event["origin"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["raw", "direct", "unicode"]
    );
    assert!(
        commits
            .iter()
            .all(|event| event["comparison"]["default_matches_selection"].is_null())
    );
}

#[test]
fn metrics_never_write_in_read_only_or_disabled_learning() {
    for mode in [LearningModeConfig::OnlyOutput, LearningModeConfig::Nothing] {
        let directory = TempDir::new().unwrap();
        let mut engine = engine(directory.path(), true, mode);
        start(&mut engine);
        key(&mut engine, protocol::UserAction::Input, "shikai");
        assert!(
            !key(&mut engine, protocol::UserAction::Enter, "")
                .commit
                .is_empty()
        );
        drop(engine);
        assert!(!directory.path().join("metrics-v1").exists());
    }
}

#[test]
fn metrics_open_failure_never_changes_protocol_or_conversion() {
    let healthy = TempDir::new().unwrap();
    let blocked = TempDir::new().unwrap();
    fs::write(blocked.path().join("metrics-v1"), b"must remain untouched").unwrap();
    let mut good = engine(healthy.path(), true, LearningModeConfig::InputAndOutput);
    let mut bad = engine(blocked.path(), true, LearningModeConfig::InputAndOutput);
    start(&mut good);
    start(&mut bad);
    for (action, text) in [
        (protocol::UserAction::Input, "shikai"),
        (protocol::UserAction::Down, ""),
        (protocol::UserAction::Enter, ""),
        (protocol::UserAction::Enter, ""),
    ] {
        assert_eq!(key(&mut good, action, text), key(&mut bad, action, text));
    }
    drop((good, bad));
    assert_eq!(
        fs::read(blocked.path().join("metrics-v1")).unwrap(),
        b"must remain untouched"
    );
}

#[test]
fn metrics_share_one_run_and_allocate_private_ids_across_connections() {
    let directory = TempDir::new().unwrap();
    let mut first = engine(directory.path(), true, LearningModeConfig::InputAndOutput);
    let mut second = first.for_connection();
    start(&mut first);
    start(&mut second);
    drop(first);
    key(&mut second, protocol::UserAction::Input, "shikai");
    assert!(
        !key(&mut second, protocol::UserAction::Enter, "")
            .commit
            .is_empty()
    );
    drop(second);
    let records = events(directory.path());
    assert!(
        records
            .iter()
            .all(|record| record["run"] == records[0]["run"])
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["event"]["kind"] == "run_started")
            .count(),
        1
    );
    assert_eq!(records.last().unwrap()["event"]["kind"], "run_stopped");
    let ids: Vec<_> = records
        .iter()
        .filter(|record| record["event"]["kind"] == "session_started")
        .map(|record| &record["event"]["session"])
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["event"]["kind"] == "session_ended")
            .count(),
        2
    );
    let serialized = serde_json::to_string(&records).unwrap();
    for private in ["private-public-session-id", "shikai", "しかい"] {
        assert!(!serialized.contains(private));
    }
    let summary =
        serde_json::to_value(metrics::summarize_metrics(directory.path()).unwrap()).unwrap();
    assert_eq!(summary["coverage"]["open_runs"], 0);
    assert_eq!(summary["coverage"]["sequence_gaps"], 0);
    assert_eq!(summary["coverage"]["journal_dropped_records"], 0);
}

#[test]
fn metrics_keep_actual_same_text_candidate_on_forced_commit_after_reveal() {
    let directory = TempDir::new().unwrap();
    let mut engine = engine(directory.path(), false, LearningModeConfig::InputAndOutput);
    start(&mut engine);
    key(&mut engine, protocol::UserAction::Input, "aru");
    // Shift-Right wraps to one kana, then expands to the full two-kana span.
    let expand_segment = protocol::KeyEvent {
        action: protocol::UserAction::Right as i32,
        shift: true,
        ..Default::default()
    };
    state(&mut engine, Payload::KeyEvent(expand_segment.clone()));
    let selecting = state(&mut engine, Payload::KeyEvent(expand_segment));
    assert_eq!(
        selecting.input_state,
        protocol::InputState::Selecting as i32
    );
    let default = &selecting.candidates[selecting.selected_candidate as usize];
    assert_eq!(
        selecting.preedit, default.text,
        "the default must consume the full reading"
    );
    let ordinary_index = selecting
        .candidates
        .iter()
        .position(|candidate| candidate.text == "ある")
        .expect("the ordinary dictionary must offer the full kana candidate ある");
    let ordinary = &selecting.candidates[ordinary_index];
    assert_ne!(ordinary.annotation, "ひらがな");
    let default_matches = default.text == ordinary.text;

    let revealed = key(&mut engine, protocol::UserAction::Up, "");
    assert_eq!(revealed.candidates.len(), selecting.candidates.len() + 1);
    assert_eq!(revealed.selected_candidate, 0);
    assert_eq!(revealed.candidates[0].text, ordinary.text);
    assert_eq!(revealed.candidates[0].annotation, "ひらがな");
    assert_eq!(revealed.candidates[ordinary_index + 1].text, ordinary.text);
    assert_eq!(
        revealed.candidates[ordinary_index + 1].annotation,
        ordinary.annotation
    );
    let candidate_count = revealed.candidates.len();
    let mut selected = revealed;
    for _ in 0..=ordinary_index {
        selected = key(&mut engine, protocol::UserAction::Down, "");
    }
    assert_eq!(selected.selected_candidate as usize, ordinary_index + 1);
    assert_eq!(selected.preedit, ordinary.text);

    let accepted = state(
        &mut engine,
        Payload::CommitComposition(protocol::CommitComposition {}),
    );
    assert!(accepted.consumed);
    assert!(accepted.reset);
    assert_eq!(accepted.commit, ordinary.text);
    assert_eq!(accepted.input_state, protocol::InputState::None as i32);
    assert!(accepted.preedit.is_empty());
    drop(engine);

    let records = events(directory.path());
    let commits = commits(&records);
    assert_eq!(commits.len(), 1);
    let commit = commits[0];
    assert_eq!(commit["operation"], "commit_composition");
    assert_eq!(commit["origin"], "candidate");
    assert_eq!(commit["candidate_chars"], accepted.commit.chars().count());
    let comparison = &commit["comparison"];
    // The earlier same-text representation is not a learning target. A text-only
    // learning lookup must not overwrite the actual ordinary candidate's measurement.
    assert_eq!(comparison["status"], "comparable", "{commit}");
    assert_eq!(comparison["task"], "Conversion");
    assert_eq!(comparison["selected_task"], "Conversion");
    assert_eq!(comparison["default_matches_selection"], default_matches);
    assert_eq!(comparison["baseline_matches_selection"], default_matches);
    assert_eq!(comparison["default_changed"], false);
    assert_eq!(comparison["selected_rank"], ordinary_index + 1);
    assert_eq!(comparison["candidate_count"], candidate_count);
}
