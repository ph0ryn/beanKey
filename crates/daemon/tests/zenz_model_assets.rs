use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use beankey_converter::{
    ComposingText, InputStyle, InputTableRegistry, LmTypoConfig, ZenzTokenizer,
    experimental_typo_correction,
};
use beankey_daemon::protocol::envelope::Payload;
use beankey_daemon::{
    Engine, LlamaModel, PROTOCOL_VERSION, protocol, read_envelope, write_envelope,
};
use beankey_llama::LlamaContext;
use tempfile::TempDir;

fn dictionary_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../converter/assets/dictionary/azookeyKanaKanji/Dictionary")
}

fn envelope(request_id: u64, payload: Payload) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id: "fixed-model".into(),
        payload: Some(payload),
        trace: Vec::new(),
    }
}

fn required_environment(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must identify the fixed test asset"))
}

#[test]
fn converts_with_the_fixed_zenz_model_and_bundled_llama() {
    let model_path = required_environment("BEANKEY_TEST_MODEL");
    let mut engine = Engine::open_with_llama(dictionary_root(), model_path)
        .expect("the fixed model and bundled llama.cpp must load");
    engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    ));

    let first = engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "は".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    ));
    let Some(Payload::StateResponse(first)) = first.payload else {
        panic!("fixed-model conversion returned a protocol error");
    };
    assert_eq!(first.preedit, "は");

    let converted = engine.handle(envelope(
        3,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "し".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    ));

    let Some(Payload::StateResponse(state)) = converted.payload else {
        panic!("fixed-model conversion returned a protocol error");
    };
    assert_eq!(state.preedit, "はし");
    assert_eq!(
        state
            .candidates
            .first()
            .map(|candidate| candidate.text.as_str()),
        Some("はし")
    );
    assert!(
        state
            .candidates
            .iter()
            .any(|candidate| candidate.text == "箸")
    );
    assert!(state.candidates.iter().any(|candidate| {
        matches!(
            candidate
                .composing_count
                .as_ref()
                .and_then(|count| count.count.as_ref()),
            Some(protocol::composing_count::Count::Surface(1))
        )
    }));
}

#[test]
fn fixed_model_capacity_falls_back_without_losing_input_and_recovers() {
    use beankey_converter::ZenzLanguageModel;

    let mut model = LlamaModel::load(required_environment("BEANKEY_TEST_MODEL")).unwrap();
    let text = "ゐ".repeat(beankey_llama::CONTEXT_TOKEN_LIMIT + 1);
    assert!(model.tokenize(&text, false).unwrap().len() > beankey_llama::CONTEXT_TOKEN_LIMIT);
    let mut engine = Engine::open_with_zenz_model(dictionary_root(), Box::new(model)).unwrap();
    start_input_session(&mut engine, protocol::InputStyle::Direct);
    let state = engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: text.clone(),
            ..Default::default()
        }),
    ));
    let Some(Payload::StateResponse(state)) = state.payload else {
        panic!("context capacity must fall back instead of resetting the composition");
    };
    assert_eq!(state.preedit, text);
    assert!(!state.candidates.is_empty());
    assert!(
        state
            .candidates
            .iter()
            .all(|candidate| candidate.annotation != "Zenzai")
    );
    let committed = engine.handle(envelope(
        3,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }),
    ));
    let Some(Payload::StateResponse(committed)) = committed.payload else {
        panic!("dictionary fallback must remain committable");
    };
    assert_eq!(committed.commit, text);
    assert!(committed.preedit.is_empty());
    let resumed = engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "はし".into(),
            surrounding_text: Some(protocol::SurroundingText {
                available: true,
                ..Default::default()
            }),
            ..Default::default()
        }),
    ));
    let Some(Payload::StateResponse(resumed)) = resumed.payload else {
        panic!("a shorter request must resume normal inference");
    };
    assert_eq!(resumed.preedit, "はし");
    assert!(
        resumed
            .candidates
            .iter()
            .any(|candidate| candidate.annotation == "Zenzai")
    );
}

#[test]
fn fixed_llama_tokenizer_matches_the_upstream_tokenizer_asset() {
    let model_path = required_environment("BEANKEY_TEST_MODEL");
    let tokenizer_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../converter/assets/tokenizer/tokenizer.json");
    let context = LlamaContext::load(model_path).unwrap();
    let tokenizer = ZenzTokenizer::open(tokenizer_path).unwrap();

    for input in [
        "\u{ee00}ハシ\u{ee01}箸",
        "\u{ee02}前文脈\u{ee00}カンジ\u{ee01}漢字",
        "ASCII\u{3000}SPACE",
        "御社を第一に志望しています",
    ] {
        assert_eq!(
            context.tokenize(input, false).unwrap(),
            tokenizer.encode(input).unwrap(),
            "token IDs diverged for {input:?}"
        );
    }
}

fn fixed_model_engine() -> Engine {
    Engine::open_with_llama(
        dictionary_root(),
        required_environment("BEANKEY_TEST_MODEL"),
    )
    .expect("the fixed model and bundled llama.cpp must load")
}

fn start_input_session(engine: &mut Engine, style: protocol::InputStyle) {
    engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: style as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    ));
}

fn state_response(
    engine: &mut Engine,
    request_id: u64,
    payload: Payload,
) -> protocol::StateResponse {
    let response = engine.handle(envelope(request_id, payload));
    let Some(Payload::StateResponse(state)) = response.payload else {
        panic!("fixed-model conversion returned a protocol error");
    };
    state
}

fn input_state(engine: &mut Engine, request_id: u64, text: &str) -> protocol::StateResponse {
    state_response(
        engine,
        request_id,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: text.into(),
            ..Default::default()
        }),
    )
}

#[test]
fn inference_history_preserves_rich_candidates_and_commits() {
    let inputs = [("はし", "箸"), ("てすと", "テスト"), ("かんじ", "漢字")];
    let mut reversed = inputs;
    reversed.reverse();
    for inputs in [inputs, reversed] {
        let mut engine = fixed_model_engine();
        start_input_session(&mut engine, protocol::InputStyle::Direct);
        for (step, (input, expected_commit)) in inputs.into_iter().enumerate() {
            let request_id = 2 + u64::try_from(step).unwrap() * 5;
            let mut reference = fixed_model_engine();
            start_input_session(&mut reference, protocol::InputStyle::Direct);
            let mut states = Vec::new();
            for context in [&mut engine, &mut reference] {
                input_state(context, request_id, input);
                // Down requests neural rich candidates through the same public
                // protocol action used by both frontends.
                let state = state_response(
                    context,
                    request_id + 1,
                    Payload::KeyEvent(protocol::KeyEvent {
                        action: protocol::UserAction::Down as i32,
                        ..Default::default()
                    }),
                );
                assert_eq!(state.input_state, protocol::InputState::Selecting as i32);
                assert!(!state.candidates.is_empty());
                assert!(
                    state
                        .candidates
                        .iter()
                        .all(|candidate| candidate.value.is_finite())
                );
                states.push(state);
            }
            let visible_candidates = |state: &protocol::StateResponse| {
                state
                    .candidates
                    .iter()
                    .map(|candidate| {
                        (
                            candidate.text.clone(),
                            candidate.annotation.clone(),
                            candidate.composing_count.clone(),
                            candidate.index,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            // Scores may drift with KV allocation; the ordered choices and
            // their selection semantics must still match a fresh engine.
            assert_eq!(
                visible_candidates(&states[0]),
                visible_candidates(&states[1]),
                "candidate history changed {input:?}"
            );
            assert_eq!(states[0].preedit, states[1].preedit);
            assert_eq!(states[0].selected_candidate, states[1].selected_candidate);
            assert_eq!(states[0].candidate_window, states[1].candidate_window);
            let index = states[0]
                .candidates
                .iter()
                .find(|candidate| candidate.text == expected_commit)
                .unwrap_or_else(|| panic!("missing full candidate {expected_commit:?}"))
                .index;
            for context in [&mut engine, &mut reference] {
                let committed = state_response(
                    context,
                    request_id + 2,
                    Payload::SelectCandidate(protocol::SelectCandidate { index }),
                );
                assert_eq!(committed.commit, expected_commit);
                assert!(committed.reset);
                assert!(committed.preedit.is_empty());
                assert!(committed.candidates.is_empty());
                let composing = input_state(context, request_id + 3, input);
                assert!(!composing.preedit.is_empty());
                assert_eq!(
                    composing.input_state,
                    protocol::InputState::Composing as i32
                );
                let reset = state_response(
                    context,
                    request_id + 4,
                    Payload::ResetSession(protocol::ResetSession {}),
                );
                assert!(reset.reset);
                assert!(reset.preedit.is_empty());
                assert!(reset.candidates.is_empty());
                assert_eq!(reset.input_state, protocol::InputState::None as i32);
                assert_eq!(
                    reset.candidate_window,
                    protocol::CandidateWindow::Hidden as i32
                );
            }
        }
    }
}

#[test]
fn matches_the_fixed_upstream_full_conversion_regressions() {
    for (input, expected) in [
        (
            "はがいたいのでしかいにみてもらった",
            "歯が痛いので歯科医に診てもらった",
        ),
        (
            "おんしゃをだいいちにしぼうしています",
            "御社を第一に志望しています",
        ),
        (
            "ふくをきて、きをきって、うみにきた",
            "服を着て、木を切って、海に来た",
        ),
        (
            "このぶんしょうはかんじへんかんがせいかくということでわだいのにほんごにゅうりょくしすてむをつかってうちこんでいます",
            "この文章は漢字変換が正確ということで話題の日本語入力システムを使って打ち込んでいます",
        ),
    ] {
        let mut engine = fixed_model_engine();
        start_input_session(&mut engine, protocol::InputStyle::Direct);
        let state = input_state(&mut engine, 2, input);
        assert_eq!(
            state
                .candidates
                .first()
                .map(|candidate| candidate.text.as_str()),
            Some(expected),
            "fixed upstream top candidate diverged for {input}"
        );
    }
}

#[test]
fn matches_the_pending_roman_upstream_regression() {
    let mut engine = fixed_model_engine();
    start_input_session(&mut engine, protocol::InputStyle::RomanToKana);
    let mut final_state = None;
    for (index, character) in "mizuwonomunda".chars().enumerate() {
        final_state = Some(input_state(
            &mut engine,
            u64::try_from(index).unwrap() + 2,
            &character.to_string(),
        ));
    }
    let state = final_state.unwrap();
    assert_eq!(state.preedit, "みずをのむんだ");
    assert_eq!(
        state
            .candidates
            .first()
            .map(|candidate| candidate.text.as_str()),
        Some("水を飲むんだ")
    );
}

#[test]
fn recovers_the_fixed_upstream_roman_typo_regression() {
    let tables = InputTableRegistry::new();
    let mut composing = ComposingText::new();
    composing.insert_str("ojsyougozainasu", InputStyle::RomanToKana, &tables);
    let mut model = LlamaModel::load(required_environment("BEANKEY_TEST_MODEL")).unwrap();
    let candidates = experimental_typo_correction(
        &mut model,
        "やあ、",
        &composing,
        &InputStyle::RomanToKana,
        &tables,
        &LmTypoConfig {
            beam_size: 10,
            top_k: 100,
            n_best: 20,
            ..LmTypoConfig::default()
        },
    )
    .unwrap();

    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.corrected_input == "ohayougozaimasu"),
        "expected upstream correction, got {:?}",
        candidates
            .iter()
            .map(|candidate| &candidate.corrected_input)
            .collect::<Vec<_>>()
    );
}

#[test]
fn runs_the_server_executable_with_the_fixed_assets() {
    let package = TempDir::new().unwrap();
    let root = package.path().join("relocated 日本語");
    let executable = root.join("bin/beankey-daemon");
    if let Some(source) = std::env::var_os("BEANKEY_TEST_PACKAGE") {
        assert!(
            Command::new("cp")
                .arg("-R")
                .arg(source)
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
    } else {
        let model = required_environment("BEANKEY_TEST_MODEL");
        let dictionaries =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../converter/assets/dictionary");
        let assets = root.join("share/beankey");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(&assets).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_beankey-daemon"), &executable).unwrap();
        for (name, path) in [
            ("dictionary", dictionary_root()),
            ("model", PathBuf::from(model)),
            (
                "input-ngram",
                PathBuf::from(required_environment("BEANKEY_TEST_INPUT_NGRAM")),
            ),
            (
                "tokenizer.json",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../converter/assets/tokenizer/tokenizer.json"),
            ),
            (
                "emoji-dictionary",
                dictionaries.join("azookeyEmoji/EmojiDictionary/emoji_all_E17.0.txt"),
            ),
            ("hunspell-english", dictionaries.join("hunspellEnglish")),
            ("hunspell-greek", dictionaries.join("hunspellGreek")),
        ] {
            std::os::unix::fs::symlink(path, assets.join(name)).unwrap();
        }
    }
    let runtime = TempDir::new().unwrap();
    let config_path = runtime.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
[conversion]
input_style = "roman_to_kana"
n_best = 10
japanese_prediction = "automatic"
foreign_prediction = "automatic"
full_width_roman = false
half_width_kana = false
typography = false
typo_correction = "automatic"
live_conversion = false
custom_input_tables = {}

[zenz]
inference_limit = 10
rich_candidates = false
predictive_input = false
enable_alignment_separator = false

[lm_typo]
enabled = true
substitution_cost = 2.0
deletion_cost = 3.0
transposition_cost = 2.0

[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#,
    )
    .unwrap();
    let learning_directory = runtime.path().join("state/beankey/learning");
    let mut daemon = Command::new(&executable)
        .arg("--config")
        .arg(&config_path)
        .arg("--runtime-root")
        .arg(runtime.path())
        .arg("--learning-directory")
        .arg(&learning_directory)
        .spawn()
        .unwrap();
    let daemon = ChildGuard(&mut daemon);
    let socket = runtime.path().join("beankey/daemon.sock");
    // Metal initialization took 24 seconds on a cold macOS CI runner.
    let mut stream = connect_before(daemon.0, &socket, Duration::from_secs(60));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Match the native clients' request timeout and one-character key events.
    let mut request_id = 0;
    let mut exchange = |session_id: &str, payload| match exchange_payload(
        &mut stream,
        &mut request_id,
        session_id,
        payload,
    ) {
        Payload::StateResponse(state) => state,
        other => panic!("server returned {other:?}"),
    };
    exchange(
        "english",
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            keyboard_language: protocol::KeyboardLanguage::EnglishUs as i32,
            ..Default::default()
        }),
    );
    for character in "hel".chars() {
        let state = exchange(
            "english",
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: character.to_string(),
                ..Default::default()
            }),
        );
        assert!(state.consumed && state.commit.is_empty());
        if character == 'l' {
            assert_eq!(
                state
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.text.len() > 3)
                    .map(|candidate| candidate.text.as_str())
                    .collect::<Vec<_>>(),
                ["hell", "held", "helm", "help"]
            );
        }
    }
    exchange(
        "backspace-typo",
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: Some(protocol::SurroundingText {
                available: true,
                text: "やあ、".into(),
                cursor: 3,
                anchor: 3,
            }),
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            ..Default::default()
        }),
    );
    // Keep the upstream typo and context, but replay actual typing rather than
    // inserting the whole composition in one request.
    let tables = InputTableRegistry::new();
    let mut raw = ComposingText::new();
    for character in "ojsyougozainasu".chars() {
        raw.insert_str(&character.to_string(), InputStyle::RomanToKana, &tables);
        let typed = exchange(
            "backspace-typo",
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: character.to_string(),
                ..Default::default()
            }),
        );
        assert!(typed.consumed && typed.commit.is_empty());
        assert_eq!(typed.preedit, raw.surface());
    }
    let before_backspace = Instant::now();
    let deleted = exchange(
        "backspace-typo",
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Backspace as i32,
            ..Default::default()
        }),
    );
    let elapsed = before_backspace.elapsed();
    eprintln!(
        "fixed-model automatic Backspace: {elapsed:?}, prediction={:?}",
        deleted.prediction
    );
    assert!(elapsed < Duration::from_secs(5));
    raw.delete_backward(1, &tables);
    assert_eq!(deleted.preedit, raw.surface());
    assert!(deleted.consumed && deleted.commit.is_empty());
    let prediction = deleted
        .prediction
        .expect("fixed model must offer its first converted correction");
    let prefix_len = deleted.preedit.chars().count() - prediction.delete_count as usize;
    let expected =
        deleted.preedit.chars().take(prefix_len).collect::<String>() + &prediction.append_text;
    assert_eq!(expected, "おはようございます");
    assert!(!prediction.display_text.is_empty());
    let accepted = exchange(
        "backspace-typo",
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Tab as i32,
            ..Default::default()
        }),
    );
    assert!(accepted.consumed && accepted.commit.is_empty());
    assert_eq!(accepted.preedit, expected);
    let committed = exchange(
        "backspace-typo",
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }),
    );
    assert_eq!(committed.commit, accepted.preedit);
    assert!(committed.preedit.is_empty() && committed.prediction.is_none());
    for (key, expected) in [("k", "k"), ("a", "か"), ("n", "かn"), ("a", "かな")] {
        let resumed = exchange(
            "backspace-typo",
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: key.into(),
                ..Default::default()
            }),
        );
        assert!(resumed.consumed && resumed.commit.is_empty());
        assert_eq!(resumed.preedit, expected);
    }
    exercise_manual_ngram_correction(&mut stream, &mut request_id);
    drop(stream);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            panic!("daemon did not exit after its last client disconnected");
        }
        thread::sleep(Duration::from_millis(20));
    }
    verify_ngram_asset_failures(&root.join("share/beankey"));
}

fn verify_ngram_asset_failures(assets: &Path) {
    let missing = TempDir::new().unwrap();
    for entry in fs::read_dir(assets).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "input-ngram" {
            std::os::unix::fs::symlink(entry.path(), missing.path().join(entry.file_name()))
                .unwrap();
        }
    }
    let learning = TempDir::new().unwrap();
    let mut config = beankey_daemon::DaemonConfig::parse("[lm_typo]\nenabled = true\n").unwrap();
    assert!(
        matches!(
            Engine::open_with_config(&config, missing.path(), learning.path()),
            Err(beankey_daemon::EngineOpenError::NGram(_))
        ),
        "an enabled correction API must fail startup without its bundled ngram"
    );
    config.lm_typo.enabled = false;
    Engine::open_with_config(&config, missing.path(), learning.path()).unwrap();

    config.lm_typo.enabled = true;
    config.lm_typo.ngram = Some(beankey_daemon::TypoNGramConfig {
        prefix: missing.path().join("nonexistent/lm_"),
        tokenizer: assets.join("tokenizer.json"),
        n: 5,
        discount: 0.75,
    });
    assert!(
        matches!(
            Engine::open_with_config(&config, assets, learning.path()),
            Err(beankey_daemon::EngineOpenError::NGram(_))
        ),
        "an invalid explicit model must not fall back to the available bundled ngram"
    );
}

fn exchange_payload(
    stream: &mut UnixStream,
    request_id: &mut u64,
    session_id: &str,
    payload: Payload,
) -> Payload {
    *request_id += 1;
    let mut request = envelope(*request_id, payload);
    request.session_id = session_id.into();
    write_envelope(stream, &request).unwrap();
    let response = read_envelope(stream).unwrap();
    assert_eq!(response.request_id, *request_id);
    assert_eq!(response.session_id, session_id);
    response
        .payload
        .expect("server response must have a payload")
}

fn exercise_manual_ngram_correction(stream: &mut UnixStream, request_id: &mut u64) {
    let mut exchange = |payload| exchange_payload(stream, request_id, "manual-typo", payload);
    exchange(Payload::StartSession(protocol::StartSession {
        input_style: protocol::InputStyle::RomanToKana as i32,
        keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
        surrounding_text: Some(protocol::SurroundingText {
            available: true,
            text: "やあ、".into(),
            cursor: 3,
            anchor: 3,
        }),
        ..Default::default()
    }));
    let tables = InputTableRegistry::new();
    for select in [false, true] {
        let mut raw = ComposingText::new();
        for character in "ojsyougozainasu".chars() {
            let text = character.to_string();
            raw.insert_str(&text, InputStyle::RomanToKana, &tables);
            let typed = exchange(Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text,
                ..Default::default()
            }));
            assert!(matches!(typed, Payload::StateResponse(state)
                if state.consumed && state.commit.is_empty() && state.preedit == raw.surface()));
        }
        let response = exchange(Payload::RequestTypoCorrections(
            protocol::RequestTypoCorrections {},
        ));
        let Payload::TypoCorrectionResponse(correction) = response else {
            panic!("expected manual corrections, got {response:?}");
        };
        assert!(!correction.candidates.is_empty() && correction.candidates.len() <= 3);
        let first = &correction.candidates[0];
        let mut corrected = ComposingText::new();
        corrected.insert_str(&first.corrected_input, InputStyle::RomanToKana, &tables);
        assert_eq!(corrected.surface(), "おはようございます");
        assert!(!first.converted_text.is_empty());
        let (payload, expected) = if select {
            (
                Payload::SelectTypoCorrection(protocol::SelectTypoCorrection { index: 0 }),
                first.converted_text.clone(),
            )
        } else {
            (
                Payload::KeyEvent(protocol::KeyEvent {
                    action: protocol::UserAction::Enter as i32,
                    ..Default::default()
                }),
                raw.surface(),
            )
        };
        let committed = exchange(payload);
        assert!(matches!(committed, Payload::StateResponse(state)
            if state.consumed && state.commit == expected && state.preedit.is_empty() && state.reset));
        assert!(
            matches!(
                exchange(Payload::SelectTypoCorrection(
                    protocol::SelectTypoCorrection { index: 0 }
                )),
                Payload::ProtocolError(_)
            ),
            "committing must invalidate correction candidates"
        );
    }
}

struct ChildGuard<'a>(&'a mut Child);

impl Drop for ChildGuard<'_> {
    fn drop(&mut self) {
        // kill can fail if the child already exited. Always wait to reap it,
        // including while unwinding a failed assertion in the parent test.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn connect_before(daemon: &mut Child, path: &Path, timeout: Duration) -> UnixStream {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            panic!("daemon exited before accepting connections: {status}");
        }
        match UnixStream::connect(path) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("could not connect to daemon: {error}"),
        }
    }
}

#[test]
fn reaps_child_when_startup_times_out() {
    let runtime = TempDir::new().unwrap();
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let child = ChildGuard(&mut child);
        connect_before(
            child.0,
            &runtime.path().join("missing.sock"),
            Duration::ZERO,
        );
    }));
    let reaped = child.try_wait().unwrap().is_some();
    // Also clean up when this regression test fails.
    if !reaped {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert!(result.is_err());
    assert!(reaped, "a failed startup left its child running");
}

#[test]
#[should_panic(expected = "daemon exited before accepting connections")]
fn reports_daemon_exit_during_startup() {
    let runtime = TempDir::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
        .arg("--invalid-option")
        .spawn()
        .unwrap();
    let child = ChildGuard(&mut child);
    connect_before(
        child.0,
        &runtime.path().join("missing.sock"),
        Duration::from_secs(1),
    );
}
