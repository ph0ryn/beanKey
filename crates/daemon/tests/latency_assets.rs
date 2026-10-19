//! Opt-in public Engine benchmark using the pinned real model and synthetic input.
use std::time::Instant;

use beankey_daemon::protocol::envelope::Payload;
use beankey_daemon::{DaemonConfig, Engine, PROTOCOL_VERSION, protocol};
use tempfile::TempDir;

fn asset(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be provided by Nix"))
}

#[test]
#[ignore = "performance probe; run alone with --release --ignored --nocapture"]
fn live_roman_input_latency() {
    let config = DaemonConfig::parse(&format!(
        r#"
        dictionary = "{}"
        model = "{}"
        emoji_dictionary = "{}"
        llama_backend_directory = "{}"
        runtime_socket = "beankey/daemon.sock"
        [hunspell]
        english_dictionary = "{}"
        greek_dictionary = "{}"
        [conversion]
        live_conversion = true
        japanese_prediction = "disabled"
        foreign_prediction = "disabled"
        [zenz]
        inference_limit = 3
        enable_alignment_separator = true
        [inference]
        context_size = 512
        batch_size = 512
        micro_batch_size = 64
        flash_attention = true
        "#,
        asset("BEANKEY_TEST_DICTIONARY"),
        asset("BEANKEY_TEST_MODEL"),
        asset("BEANKEY_TEST_EMOJI_DICTIONARY"),
        asset("BEANKEY_TEST_LLAMA_BACKEND"),
        asset("BEANKEY_TEST_EN_US_DICTIONARY"),
        asset("BEANKEY_TEST_EL_GR_DICTIONARY"),
    ))
    .unwrap();
    let directory = TempDir::new().unwrap();
    let mut engine = Engine::open_with_config(&config, directory.path()).unwrap();
    let mut request_id = 0;
    let mut samples = Vec::new();
    let mut response_fingerprint = 0xcbf29ce484222325_u64;
    for round in 0..5 {
        let mut send = |payload| {
            request_id += 1;
            let started = Instant::now();
            let response = engine.handle(protocol::Envelope {
                protocol_version: PROTOCOL_VERSION,
                request_id,
                session_id: format!("latency-probe-{round}"),
                payload: Some(payload),
                trace: Vec::new(),
            });
            let duration = started.elapsed().as_micros();
            let Some(Payload::StateResponse(state)) = response.payload else {
                panic!("latency probe returned a protocol error");
            };
            for byte in format!("{state:?}").bytes() {
                response_fingerprint =
                    (response_fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
            (state, duration)
        };
        send(Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            ..Default::default()
        }));
        for character in "kyouhaiitenkinanodegakkoumadearuiteikimasu".chars() {
            let (state, duration) = send(Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: character.to_string(),
                ..Default::default()
            }));
            assert!(state.consumed);
            samples.push(duration);
        }
        let (state, _) = send(Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }));
        assert!(!state.commit.is_empty());
        assert!(state.preedit.is_empty());
        send(Payload::EndSession(protocol::EndSession {}));
    }
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100) - 1];
    eprintln!(
        "engine_probe n={} mean_us={} p50_us={} p95_us={} p99_us={} max_us={} response_fingerprint={response_fingerprint:016x}",
        samples.len(),
        samples.iter().sum::<u128>() / samples.len() as u128,
        percentile(50),
        percentile(95),
        percentile(99),
        samples.last().unwrap(),
    );
}
