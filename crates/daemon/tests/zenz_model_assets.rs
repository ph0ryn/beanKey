use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use bean_key_converter::{
    ComposingText, InputStyle, InputTableRegistry, LmTypoConfig, ZenzTokenizer,
    experimental_typo_correction,
};
use bean_key_daemon::protocol::envelope::Payload;
use bean_key_daemon::{
    Engine, LlamaModel, PROTOCOL_VERSION, protocol, read_envelope, write_envelope,
};
use bean_key_llama::{LlamaContext, LlamaSequence};
use tempfile::TempDir;

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
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
    let model_path = required_environment("BEAN_KEY_TEST_MODEL");
    let backend_directory = required_environment("BEAN_KEY_TEST_LLAMA_BACKEND");
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
fn cached_and_batched_logits_match_a_single_full_evaluation() {
    let model_path = required_environment("BEAN_KEY_TEST_MODEL");
    let backend_directory = required_environment("BEAN_KEY_TEST_LLAMA_BACKEND");
    let mut context = LlamaContext::load(model_path, backend_directory).unwrap();
    let tokens = context
        .tokenize("\u{ee00}テスト\u{ee01}候補", true)
        .unwrap();
    assert!(tokens.len() >= 2);

    let batched = context
        .logits(&tokens, 0, LlamaSequence::Evaluation)
        .unwrap();
    let single = context.next_logits(&tokens).unwrap();
    let final_row = &batched[batched.len() - single.len()..];
    let maximum_difference = final_row
        .iter()
        .zip(&single)
        .map(|(batched, single)| (batched - single).abs())
        .fold(0.0_f32, f32::max);

    assert!(
        maximum_difference < 1e-5,
        "cached logits differed by {maximum_difference}"
    );
}

#[test]
fn fixed_llama_tokenizer_matches_the_upstream_tokenizer_asset() {
    let model_path = required_environment("BEAN_KEY_TEST_MODEL");
    let backend_directory = required_environment("BEAN_KEY_TEST_LLAMA_BACKEND");
    let tokenizer_path = required_environment("BEAN_KEY_TEST_ZENZ_TOKENIZER");
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

fn fixed_model_engine() -> Engine {
    Engine::open_with_llama(
        dictionary_root(),
        required_environment("BEAN_KEY_TEST_MODEL"),
        required_environment("BEAN_KEY_TEST_LLAMA_BACKEND"),
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

fn input_state(engine: &mut Engine, request_id: u64, text: &str) -> protocol::StateResponse {
    let response = engine.handle(envelope(
        request_id,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: text.into(),
            ..Default::default()
        }),
    ));
    let Some(Payload::StateResponse(state)) = response.payload else {
        panic!("fixed-model conversion returned a protocol error");
    };
    state
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
        required_environment("BEAN_KEY_TEST_MODEL"),
        required_environment("BEAN_KEY_TEST_LLAMA_BACKEND"),
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
    let model = required_environment("BEAN_KEY_TEST_MODEL");
    let backend = required_environment("BEAN_KEY_TEST_LLAMA_BACKEND");
    let english = required_environment("BEAN_KEY_TEST_EN_US_DICTIONARY");
    let greek = required_environment("BEAN_KEY_TEST_EL_GR_DICTIONARY");
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
runtime_socket = "bean-key/daemon.sock"

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
                std::env::var_os("BEAN_KEY_TEST_EMOJI_DICTIONARY").expect(
                    "BEAN_KEY_TEST_EMOJI_DICTIONARY must be set by the Nix test environment"
                )
            )
            .display()
        ),
    )
    .unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_bean-key-daemon"))
        .args(["--config", config_path.to_str().unwrap()])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("XDG_STATE_HOME", runtime.path().join("state"))
        .spawn()
        .unwrap();
    let socket = runtime.path().join("bean-key/daemon.sock");
    let mut stream = connect_before(&socket, Duration::from_secs(5));

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
        if let Some(status) = daemon.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            daemon.kill().unwrap();
            panic!("daemon did not exit after its last client disconnected");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn connect_before(path: &Path, timeout: Duration) -> UnixStream {
    let deadline = Instant::now() + timeout;
    loop {
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
