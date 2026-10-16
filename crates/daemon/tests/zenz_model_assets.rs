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
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
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
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be provided by the Nix test environment"))
}

#[test]
fn converts_with_the_fixed_zenz_model_and_llama_backend() {
    let model_path = required_environment("BEANKEY_TEST_MODEL");
    let backend_directory = required_environment("BEANKEY_TEST_LLAMA_BACKEND");
    let mut engine = Engine::open_with_llama(dictionary_root(), model_path, backend_directory)
        .expect("the fixed model and pinned llama.cpp backend must load");
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

    let mut model = LlamaModel::load(
        required_environment("BEANKEY_TEST_MODEL"),
        required_environment("BEANKEY_TEST_LLAMA_BACKEND"),
    )
    .unwrap();
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
    let backend_directory = required_environment("BEANKEY_TEST_LLAMA_BACKEND");
    let tokenizer_path = required_environment("BEANKEY_TEST_ZENZ_TOKENIZER");
    let context = LlamaContext::load(model_path, backend_directory).unwrap();
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

#[test]
fn compares_dictionary_candidates_with_the_fixed_llama_backend() {
    use beankey_converter::{
        ConversionSession, DictionaryStore, NormalConverter, PrefixConstraint,
        ZenzComparisonRequest, ZenzEvaluator, ZenzLanguageModel, ZenzPromptBuilder,
        ZenzVersionConfig,
    };

    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("はし", InputStyle::Direct, &tables);
    let drafts = session.request_candidates(&converter, &tables, 64).unwrap();
    let candidates = ["橋", "箸"].map(|text| {
        drafts
            .iter()
            .find(|candidate| candidate.text == text)
            .unwrap_or_else(|| panic!("the fixed dictionary must supply {text:?} for はし"))
            .clone()
    });
    let mut model = LlamaModel::load(
        required_environment("BEANKEY_TEST_MODEL"),
        required_environment("BEANKEY_TEST_LLAMA_BACKEND"),
    )
    .expect("the fixed model and pinned llama.cpp backend must load");
    let continuations: Vec<_> = candidates
        .iter()
        .map(|candidate| model.tokenize(&candidate.text, false).unwrap())
        .collect();
    assert!(continuations.iter().all(|tokens| !tokens.is_empty()));
    // Every full alternative pays for its own common EOS terminal as well as
    // its chosen spelling. One fewer row must skip the entire group.
    let token_budget: usize = continuations.iter().map(|tokens| tokens.len() + 1).sum();
    let version = ZenzVersionConfig::default();
    let constraint = PrefixConstraint::default();
    let request = |token_budget| ZenzComparisonRequest {
        input: "ハシ",
        input_cursor_position: None,
        candidates: &candidates,
        prefix_constraint: &constraint,
        version: &version,
        token_budget,
    };
    let mut evaluator = ZenzEvaluator::default();
    assert!(
        evaluator
            .compare_candidates(&mut model, request(token_budget - 1))
            .expect("budget preflight must not fail inference")
            .is_none()
    );
    // Reuse both the evaluator and real backend after preflight skipped them.
    let scores = evaluator
        .compare_candidates(&mut model, request(token_budget))
        .expect("the real backend must support teacher-forced comparison")
        .expect("ordinary dictionary alternatives must not silently skip comparison");
    assert_eq!(scores.len(), candidates.len());
    assert!(scores.iter().all(|score| score.is_finite() && *score < 0.0));

    // Independently teacher-force one chosen token at a time, including the
    // same EOS after each complete spelling. This checks the batched suffix
    // path against actual backend logits, not a greedy score or lexical cost.
    let prompt = ZenzPromptBuilder::candidate_evaluation("ハシ", None, "", &version);
    let prompt_tokens = model.tokenize(&prompt, true).unwrap();
    let eos = model.eos_token();
    for ((candidate, continuation), &actual) in candidates.iter().zip(&continuations).zip(&scores) {
        let mut prefix = prompt_tokens.clone();
        let mut expected = 0.0;
        for token in continuation.iter().copied().chain(std::iter::once(eos)) {
            let row = model.next_logits(&prefix).unwrap();
            assert_eq!(row.len(), model.vocabulary_size());
            let maximum = row
                .iter()
                .copied()
                .map(f64::from)
                .fold(f64::NEG_INFINITY, f64::max);
            let log_partition = maximum
                + row
                    .iter()
                    .map(|&value| (f64::from(value) - maximum).exp())
                    .sum::<f64>()
                    .ln();
            expected += f64::from(row[usize::try_from(token).unwrap()]) - log_partition;
            prefix.push(token);
        }
        // Different decode batch shapes can incur small floating-point drift.
        assert!(
            (actual - expected).abs() < 0.05,
            "{}: batched score {actual} differs from EOS-terminated teacher forcing {expected}",
            candidate.text,
        );
    }
}

fn fixed_model_engine() -> Engine {
    Engine::open_with_llama(
        dictionary_root(),
        required_environment("BEANKEY_TEST_MODEL"),
        required_environment("BEANKEY_TEST_LLAMA_BACKEND"),
    )
    .expect("the fixed model and pinned llama.cpp backend must load")
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
    let mut model = LlamaModel::load(
        required_environment("BEANKEY_TEST_MODEL"),
        required_environment("BEANKEY_TEST_LLAMA_BACKEND"),
    )
    .unwrap();
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
fn runs_the_server_executable_with_the_fixed_nixos_assets() {
    let model = required_environment("BEANKEY_TEST_MODEL");
    let backend = required_environment("BEANKEY_TEST_LLAMA_BACKEND");
    let english = required_environment("BEANKEY_TEST_EN_US_DICTIONARY");
    let greek = required_environment("BEANKEY_TEST_EL_GR_DICTIONARY");
    let runtime = TempDir::new().unwrap();
    let config_path = runtime.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            r#"
dictionary = "{}"
model = "{model}"
emoji_dictionary = "{}"
llama_backend_directory = "{backend}"
runtime_socket = "beankey/daemon.sock"

[hunspell]
english_dictionary = "{english}"
greek_dictionary = "{greek}"

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
custom_input_tables = {{}}

[zenz]
inference_limit = 10
rich_candidates = false
predictive_input = false
enable_alignment_separator = false

[lm_typo]
enabled = false
language_model = "zenz"
beam_size = 32
top_k = 64
n_best = 5
substitution_cost = 2.0
deletion_cost = 3.0
transposition_cost = 2.0

[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#,
            dictionary_root().display(),
            PathBuf::from(
                std::env::var_os("BEANKEY_TEST_EMOJI_DICTIONARY").expect(
                    "BEANKEY_TEST_EMOJI_DICTIONARY must be set by the Nix test environment"
                )
            )
            .display()
        ),
    )
    .unwrap();
    let learning_directory = runtime.path().join("state/beankey/learning");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
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
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .unwrap();

    for request in [
        envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Direct as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::EnglishUs as i32,
                custom_input_table: String::new(),
            }),
        ),
        envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "hel".into(),
                ..Default::default()
            }),
        ),
    ] {
        write_envelope(&mut stream, &request).unwrap();
        let response = read_envelope(&mut stream).unwrap();
        let Some(Payload::StateResponse(state)) = response.payload else {
            panic!("server returned a protocol error");
        };
        if request.request_id == 2 {
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
