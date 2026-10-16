//! Public Engine/protocol acceptance regressions; no private daemon test hooks.
use std::fs;
use std::path::{Path, PathBuf};

use beankey_daemon::protocol::envelope::Payload;
use beankey_daemon::{Engine, PROTOCOL_VERSION, protocol};
use serde_json::Value;
use tempfile::TempDir;

const STORE: &str = "personalization-v1.json";
const ROMAN: &str = "shikai";
const KANA: &str = "しかい";
const WEATHER_ROMAN: &str = "kyouhaiitenki";
const WEATHER_KANA: &str = "きょうはいいてんき";

fn dictionary_root() -> PathBuf {
    std::env::var_os("BEANKEY_TEST_DICTIONARY")
        .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment")
        .into()
}

struct Client {
    engine: Engine,
    request_id: u64,
    session_id: String,
}

impl Client {
    fn open(directory: &Path) -> Self {
        Self {
            engine: Engine::open_with_learning(dictionary_root(), directory).unwrap(),
            request_id: 0,
            session_id: String::new(),
        }
    }

    fn send(&mut self, payload: Payload) -> protocol::StateResponse {
        self.request_id += 1;
        let reply = self.engine.handle(protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.request_id,
            session_id: self.session_id.clone(),
            payload: Some(payload),
            trace: Vec::new(),
        });
        match reply.payload.expect("Engine response payload") {
            Payload::StateResponse(state) => state,
            Payload::ProtocolError(error) => panic!("protocol error: {}", error.message),
            _ => panic!("unexpected Engine response"),
        }
    }

    fn start(&mut self, context: &str) {
        self.session_id = format!("personalization-{}", self.request_id + 1);
        let cursor = context.chars().count() as u32;
        let state = self.send(Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: Some(protocol::SurroundingText {
                available: true,
                text: context.into(),
                cursor,
                anchor: cursor,
            }),
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }));
        assert!(state.learning_available && state.learning_writable);
    }

    fn key(&mut self, action: protocol::UserAction, text: &str) -> protocol::StateResponse {
        self.send(Payload::KeyEvent(protocol::KeyEvent {
            action: action as i32,
            text: text.into(),
            input: text.into(),
            intention: text.into(),
            ..Default::default()
        }))
    }

    fn shifted(&mut self, action: protocol::UserAction) -> protocol::StateResponse {
        self.send(Payload::KeyEvent(protocol::KeyEvent {
            action: action as i32,
            shift: true,
            ..Default::default()
        }))
    }

    fn selecting(&mut self, context: &str) -> protocol::StateResponse {
        self.start(context);
        self.key(protocol::UserAction::Input, ROMAN);
        // Desktop's ordinary Down list starts with a first-clause candidate.
        // Explicitly grow the selected segment back to the whole reading so
        // the homograph fixtures all have the same actual consumed span.
        self.shifted(protocol::UserAction::Left);
        let state = self.shifted(protocol::UserAction::Right);
        assert_eq!(state.input_state, protocol::InputState::Selecting as i32);
        assert_eq!(state.selected_candidate, 0);
        state
    }

    fn navigate(&mut self, candidate: &protocol::Candidate) {
        // Every caller starts from the initial selected index zero. Navigation
        // must not replace the observation of that initially displayed default.
        for _ in 0..candidate.index {
            let state = self.key(protocol::UserAction::Down, "");
            assert_eq!(state.input_state, protocol::InputState::Selecting as i32);
        }
    }

    fn select(&mut self, candidate: &protocol::Candidate) -> protocol::StateResponse {
        self.send(Payload::SelectCandidate(protocol::SelectCandidate {
            index: candidate.index,
        }))
    }

    fn end(&mut self) {
        let state = self.send(Payload::EndSession(protocol::EndSession {}));
        assert!(state.preedit.is_empty());
        assert!(state.commit.is_empty());
    }
}

fn selected(state: &protocol::StateResponse) -> &protocol::Candidate {
    state
        .candidates
        .iter()
        .find(|candidate| candidate.index as i32 == state.selected_candidate)
        .expect("initial selected candidate is included in public response")
}

fn candidate_named(state: &protocol::StateResponse, text: &str) -> protocol::Candidate {
    state
        .candidates
        .iter()
        .find(|candidate| candidate.text == text)
        .unwrap_or_else(|| panic!("missing {text:?} among {:?}", state.candidates))
        .clone()
}

fn full_fixture_candidate(candidate: &protocol::Candidate, roman: &str, kana: &str) -> bool {
    // Fixture discovery only: accept either public count representation. The
    // implementation must compare actual consumed spans, not enum equality.
    match candidate
        .composing_count
        .as_ref()
        .and_then(|count| count.count.as_ref())
    {
        Some(protocol::composing_count::Count::Surface(count)) => {
            *count as usize == kana.chars().count()
        }
        Some(protocol::composing_count::Count::Input(count)) => {
            let count = *count as usize;
            count == roman.chars().count() || count == roman.chars().count() + 1
        }
        _ => false,
    }
}

fn alternative(state: &protocol::StateResponse) -> protocol::Candidate {
    let default = selected(state);
    assert!(full_fixture_candidate(default, ROMAN, KANA));
    ["司会", "視界", "歯科医", "市会"]
        .into_iter()
        .filter(|text| *text != default.text)
        .find_map(|text| {
            state.candidates.iter().find(|candidate| {
                candidate.text == text && full_fixture_candidate(candidate, ROMAN, KANA)
            })
        })
        .expect("pinned dictionary provides different full-reading shikai homographs")
        .clone()
}

fn stored(directory: &Path) -> Value {
    let value: Value = serde_json::from_slice(&fs::read(directory.join(STORE)).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert!(value["recall"].is_array());
    assert!(value["comparisons"].is_array());
    for legacy in [
        "memory.louds",
        "memory.loudschars2",
        "memory.memorymetadata",
        "memory0.loudstxt3",
    ] {
        assert!(!directory.join(legacy).exists());
    }
    value
}

fn recalled(value: &Value, text: &str) -> bool {
    value["recall"].as_array().unwrap().iter().any(|path| {
        path.as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["word"].as_str().unwrap())
            .collect::<String>()
            == text
    })
}

fn directed_support(value: &Value, default: &str, chosen: &str, context: &str) -> i64 {
    let pairs = value["comparisons"].as_array().unwrap();
    assert_eq!(
        pairs.len(),
        1,
        "exactly one canonical comparison, not an accept log"
    );
    let pair = &pairs[0];
    assert_eq!(pair["scope"]["context"], context);
    assert_eq!(pair["scope"]["reading"], "シカイ");
    assert_eq!(pair["scope"]["task"], "Conversion");
    assert!(
        pair["scope"]["action"]
            .as_str()
            .unwrap()
            .starts_with("span=")
    );
    match (
        pair["first"]["key"]["text"].as_str(),
        pair["second"]["key"]["text"].as_str(),
    ) {
        (Some(from), Some(to)) if from == default && to == chosen => {
            pair["support"].as_i64().unwrap()
        }
        (Some(to), Some(from)) if from == default && to == chosen => {
            -pair["support"].as_i64().unwrap()
        }
        endpoints => panic!("unexpected correction endpoints {endpoints:?}"),
    }
}

fn metrics_records(directory: &Path) -> (String, Vec<Value>) {
    let mut paths = fs::read_dir(directory.join("metrics-v1"))
        .expect("writable learning records local efficacy metrics")
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    paths.sort();
    let text = paths
        .into_iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<String>();
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["record"].clone())
        .collect();
    (text, records)
}

#[test]
fn efficacy_metrics_preserve_actual_defaults_and_never_record_text() {
    let directory = TempDir::new().unwrap();
    let mut client = Client::open(directory.path());
    let context = "絶対に診断記録へ出さない機密の左文脈";
    let first = client.selecting(context);
    let baseline = selected(&first).text.clone();
    let target = alternative(&first).text;
    let mut observations = vec![(client.session_id.clone(), candidate_named(&first, &target))];
    for _ in 1..3 {
        let state = client.selecting(context);
        assert_eq!(selected(&state).text, baseline);
        observations.push((client.session_id.clone(), candidate_named(&state, &target)));
    }
    for (session, candidate) in observations {
        client.session_id = session;
        client.navigate(&candidate);
        assert_eq!(client.key(protocol::UserAction::Enter, "").commit, target);
        client.end();
    }
    let personalized = client.selecting(context);
    assert_eq!(selected(&personalized).text, target);
    assert_eq!(client.key(protocol::UserAction::Enter, "").commit, target);
    client.end();
    let personalized = client.selecting(context);
    assert_eq!(selected(&personalized).text, target);
    client.select(&candidate_named(&personalized, &baseline));
    client.end();
    drop(client);

    let (text, records) = metrics_records(directory.path());
    for secret in [
        context,
        ROMAN,
        KANA,
        "シカイ",
        &baseline,
        &target,
        "personalization-",
    ] {
        assert!(!text.contains(secret), "metrics leaked {secret:?}");
    }
    let commits = records
        .iter()
        .filter(|record| record["event"]["kind"] == "commit")
        .map(|record| &record["event"]["comparison"])
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 5, "every acceptance supplies a denominator");
    for correction in &commits[..3] {
        assert_eq!(correction["status"], "comparable");
        assert_eq!(correction["default_matches_selection"], false);
        assert_eq!(correction["baseline_matches_selection"], false);
    }
    assert_eq!(commits[3]["status"], "comparable");
    assert_eq!(commits[3]["default_matches_selection"], true);
    assert_eq!(commits[3]["baseline_matches_selection"], false);
    assert_eq!(commits[4]["default_matches_selection"], false);
    assert_eq!(commits[4]["baseline_matches_selection"], true);
    assert!(
        records
            .iter()
            .any(|record| record["event"]["kind"] == "candidate_request")
    );
    assert!(
        records
            .iter()
            .any(|record| record["event"]["kind"] == "run_started")
    );
    assert!(
        records
            .iter()
            .any(|record| record["event"]["kind"] == "run_stopped")
    );
}

#[test]
fn actual_personalized_defaults_record_corrections_and_reverse_snapshots_cancel() {
    let directory = TempDir::new().unwrap();
    let mut client = Client::open(directory.path());
    let context = "文書甲";
    let first = client.selecting(context);
    let baseline = selected(&first).text.clone();
    let target = alternative(&first).text;
    // Observe all three A defaults before recording any correction. Even weak
    // evidence can legitimately flip a close dictionary tie; subsequent queries
    // must not be mistaken for additional observations of the old default.
    let mut observations = vec![(client.session_id.clone(), candidate_named(&first, &target))];
    for _ in 1..3 {
        let state = client.selecting(context);
        assert_eq!(selected(&state).text, baseline);
        observations.push((client.session_id.clone(), candidate_named(&state, &target)));
    }
    for (method, (session_id, candidate)) in observations.into_iter().enumerate() {
        client.session_id = session_id;
        let accepted = match method {
            0 => client.select(&candidate),
            1 => {
                client.navigate(&candidate);
                client.key(protocol::UserAction::Enter, "")
            }
            _ => {
                client.navigate(&candidate);
                client.key(protocol::UserAction::Input, "ka")
            }
        };
        assert!(accepted.consumed);
        assert_eq!(accepted.commit, target);
        if method == 2 {
            assert_eq!(
                accepted.preedit, "か",
                "typing commits before starting the next composition"
            );
        } else {
            assert!(accepted.preedit.is_empty());
        }
        client.end();
        let memory = stored(directory.path());
        assert_eq!(
            directed_support(&memory, &baseline, &target, context),
            method as i64 + 1
        );
        assert!(recalled(&memory, &target));
    }

    drop(client);
    let mut client = Client::open(directory.path());
    let before_passive = stored(directory.path())["comparisons"].clone();
    let personalized = client.selecting(context);
    assert_eq!(selected(&personalized).text, target);
    let accepted = client.key(protocol::UserAction::Enter, "");
    assert_eq!(accepted.commit, target);
    client.end();
    assert_eq!(
        stored(directory.path())["comparisons"],
        before_passive,
        "passive acceptance of a personalized default must not refresh its evidence"
    );

    let isolated = client.selecting("文書乙");
    assert_eq!(
        selected(&isolated).text,
        baseline,
        "no evidence backoff into another left context"
    );
    client.end();

    // Capturing several actually displayed B defaults before correcting any of
    // them makes all three legitimate B->A reversals, even if earlier reversals
    // would change a freshly generated default.
    let mut snapshots = Vec::new();
    for _ in 0..3 {
        let state = client.selecting(context);
        assert_eq!(
            selected(&state).text,
            target,
            "three direct corrections must change the displayed default"
        );
        snapshots.push((
            client.session_id.clone(),
            candidate_named(&state, &baseline),
        ));
    }
    for (index, (session_id, candidate)) in snapshots.into_iter().enumerate() {
        client.session_id = session_id;
        let accepted = client.select(&candidate);
        assert_eq!(accepted.commit, baseline);
        client.end();
        let memory = stored(directory.path());
        if index < 2 {
            assert_eq!(
                directed_support(&memory, &baseline, &target, context),
                2 - index as i64
            );
        } else {
            assert!(
                memory["comparisons"].as_array().unwrap().is_empty(),
                "opposite actual-default corrections cancel, rather than becoming new absolute wins"
            );
        }
    }
    drop(client);
    let mut reopened = Client::open(directory.path());
    let state = reopened.selecting(context);
    assert_eq!(selected(&state).text, baseline);
    reopened.end();
}

#[test]
fn passive_preview_acceptance_and_type_after_preview_only_populate_recall() {
    let directory = TempDir::new().unwrap();
    let mut client = Client::open(directory.path());
    for method in 0..4 {
        client.start("受理文脈");
        client.key(protocol::UserAction::Input, ROMAN);
        let preview = client.key(protocol::UserAction::Space, "");
        assert_eq!(preview.input_state, protocol::InputState::Previewing as i32);
        let default = preview.preedit.clone();
        assert_ne!(default, KANA);
        let accepted = match method {
            0 => client.key(protocol::UserAction::Enter, ""),
            1 => client.send(Payload::CommitComposition(protocol::CommitComposition {})),
            2 => client.key(protocol::UserAction::Input, "ka"),
            _ => client.select(&candidate_named(&preview, &default)),
        };
        assert_eq!(accepted.commit, default);
        assert!(accepted.consumed);
        if method == 2 {
            assert_eq!(accepted.preedit, "か");
        }
        client.end();
        let memory = stored(directory.path());
        assert!(recalled(&memory, &default));
        assert!(
            memory["comparisons"].as_array().unwrap().is_empty(),
            "passively accepting the displayed default must never accumulate ranking evidence"
        );
    }
}

#[test]
fn non_live_whole_preview_is_the_correction_default_not_merged_partial_zero() {
    let directory = TempDir::new().unwrap();
    let mut client = Client::open(directory.path());
    client.start("全体文脈");
    let roman = WEATHER_ROMAN;
    let kana = WEATHER_KANA;
    client.key(protocol::UserAction::Input, roman);
    let preview = client.key(protocol::UserAction::Space, "");
    assert_eq!(preview.input_state, protocol::InputState::Previewing as i32);
    let whole = candidate_named(&preview, &preview.preedit);
    let merged_zero = preview
        .candidates
        .iter()
        .find(|candidate| candidate.index == 0)
        .unwrap();
    assert_ne!(
        whole.index, 0,
        "fixture needs a whole preview distinct from merged partial candidate zero"
    );
    assert!(!full_fixture_candidate(merged_zero, roman, kana));
    assert!(full_fixture_candidate(&whole, roman, kana));
    let target = preview
        .candidates
        .iter()
        .find(|candidate| {
            candidate.text != whole.text
                && candidate.text.contains("天気")
                && full_fixture_candidate(candidate, roman, kana)
        })
        .expect("pinned dictionary provides another full weather-phrase conversion")
        .clone();
    let accepted = client.select(&target);
    assert_eq!(accepted.commit, target.text);
    assert!(accepted.preedit.is_empty());
    client.end();
    let memory = stored(directory.path());
    let pairs = memory["comparisons"].as_array().unwrap();
    assert_eq!(
        pairs.len(),
        1,
        "a whole->whole correction must not be lost against a mismatched partial-zero anchor"
    );
    let pair = &pairs[0];
    let endpoints = [
        pair["first"]["key"]["text"].as_str().unwrap(),
        pair["second"]["key"]["text"].as_str().unwrap(),
    ];
    assert!(endpoints.contains(&whole.text.as_str()));
    assert!(endpoints.contains(&target.text.as_str()));
    assert_eq!(pair["scope"]["reading"], "キョウハイイテンキ");
    assert_eq!(pair["scope"]["task"], "Conversion");
    assert_eq!(pair["scope"]["context"], "全体文脈");
    assert!(
        memory["recall"].as_array().unwrap().iter().any(|path| {
            let entries = path.as_array().unwrap();
            entries.len() > 1
                && entries
                    .iter()
                    .map(|entry| entry["word"].as_str().unwrap())
                    .collect::<String>()
                    == target.text
        }),
        "whole phrase recall retains its original multiword dictionary path"
    );
}

#[test]
fn escape_from_shortened_selection_preserves_the_raw_remaining_preview_and_commit() {
    let directory = TempDir::new().unwrap();
    let mut client = Client::open(directory.path());
    client.start("縮小文脈");
    client.key(protocol::UserAction::Input, WEATHER_ROMAN);
    let shortened = client.shifted(protocol::UserAction::Left);
    assert_eq!(
        shortened.input_state,
        protocol::InputState::Selecting as i32
    );
    let candidate = selected(&shortened);
    let remaining = shortened.preedit.strip_prefix(&candidate.text).unwrap();
    assert!(
        !remaining.is_empty(),
        "fixture must retain an unconverted suffix"
    );
    assert!(WEATHER_KANA.ends_with(remaining));
    let expected = shortened.preedit.clone();
    let preview = client.key(protocol::UserAction::Escape, "");
    assert_eq!(preview.input_state, protocol::InputState::Previewing as i32);
    assert_eq!(
        preview.preedit, expected,
        "leaving selection must not lose the raw remaining suffix"
    );
    let accepted = client.key(protocol::UserAction::Enter, "");
    assert_eq!(accepted.commit, expected);
    assert!(accepted.preedit.is_empty());
    client.end();
}

#[test]
fn failed_personalization_saves_never_discard_committed_protocol_text() {
    for method in 0..3 {
        let directory = TempDir::new().unwrap();
        let mut client = Client::open(directory.path());
        // Only sabotage the publication target after open: session input and
        // acceptance must remain usable, while EndSession's barrier reports its
        // persistence warning without replacing the response with ProtocolError.
        let blocked = directory.path().join(STORE);
        fs::create_dir(&blocked).unwrap();
        let selecting = client.selecting("失敗文脈");
        let target = alternative(&selecting);
        let accepted = match method {
            0 => {
                client.navigate(&target);
                client.key(protocol::UserAction::Enter, "")
            }
            1 => client.select(&target),
            _ => {
                client.navigate(&target);
                client.send(Payload::CommitComposition(protocol::CommitComposition {}))
            }
        };
        assert!(accepted.consumed);
        assert_eq!(accepted.commit, target.text);
        assert!(accepted.preedit.is_empty());
        client.end();
        assert!(blocked.is_dir());

        // Verify the exact resolved empty directory before removing the injected
        // failure. A fresh EndSession is then an explicit retry of dirty state.
        let resolved = blocked.canonicalize().unwrap();
        assert_eq!(
            resolved,
            directory.path().canonicalize().unwrap().join(STORE)
        );
        fs::remove_dir(&resolved).unwrap();
        client.start("失敗文脈");
        client.end();
        let memory = stored(directory.path());
        assert!(recalled(&memory, &target.text));
        assert_eq!(memory["comparisons"].as_array().unwrap().len(), 1);
    }
}
