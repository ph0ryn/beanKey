use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use beankey_converter::{ZenzInferenceError, ZenzLanguageModel};
use beankey_daemon::protocol::envelope::Payload;
use beankey_daemon::{
    ConversionConfig, ConversionResourceError, Engine, EngineOpenError, PROTOCOL_VERSION,
    PunctuationStyleConfig, protocol,
};
use tempfile::TempDir;

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

fn emoji_dictionary_path() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_EMOJI_DICTIONARY")
            .expect("BEANKEY_TEST_EMOJI_DICTIONARY must be set by the Nix test environment"),
    )
}

fn envelope(request_id: u64, payload: Payload) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id: "session".into(),
        payload: Some(payload),
        trace: Vec::new(),
    }
}

fn response(envelope: protocol::Envelope) -> protocol::StateResponse {
    match envelope.payload.unwrap() {
        Payload::StateResponse(response) => response,
        Payload::ProtocolError(error) => panic!("protocol error: {}", error.message),
        _ => panic!("unexpected response"),
    }
}

fn start_roman_session(engine: &mut Engine, request_id: u64) {
    response(engine.handle(envelope(
        request_id,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
}

#[test]
fn advertises_only_configured_addon_capabilities() {
    let mut stateless = Engine::open(dictionary_root()).unwrap();
    let stateless_start = response(stateless.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    assert!(!stateless_start.lm_typo_available);
    assert!(!stateless_start.learning_available);
    assert!(!stateless_start.learning_writable);

    let state = TempDir::new().unwrap();
    let mut learning = Engine::open_with_learning(dictionary_root(), state.path()).unwrap();
    let learning_start = response(learning.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    assert!(!learning_start.lm_typo_available);
    assert!(learning_start.learning_available);
    assert!(learning_start.learning_writable);
}

fn key_event(key_sym: u32, text: &str) -> protocol::KeyEvent {
    protocol::KeyEvent {
        action: match key_sym {
            0 => protocol::UserAction::Input,
            0xff08 => protocol::UserAction::Backspace,
            0xff0d | 0xff8d => protocol::UserAction::Enter,
            0xff1b => protocol::UserAction::Escape,
            0xff09 | 0xfe20 => protocol::UserAction::Tab,
            0xff51 => protocol::UserAction::Left,
            0xff52 => protocol::UserAction::Up,
            0xff53 => protocol::UserAction::Right,
            0xff54 => protocol::UserAction::Down,
            0x20 => protocol::UserAction::Space,
            _ => protocol::UserAction::Unspecified,
        } as i32,
        text: text.into(),
        input: text.into(),
        intention: text.into(),
        ..Default::default()
    }
}

fn configured_engine(config: ConversionConfig) -> Engine {
    Engine::open_with_conversion_resources(dictionary_root(), &config).unwrap()
}

#[test]
fn applies_the_desktop_backslash_preference_and_option_inversion() {
    for (type_backslash, option, shift, expected) in [
        (false, false, false, "￥"),
        (true, false, false, "＼"),
        (false, true, false, "＼"),
        (true, true, false, "￥"),
        (false, false, true, "｜"),
        (true, true, true, "｜"),
    ] {
        let mut engine = configured_engine(ConversionConfig {
            type_backslash,
            ..Default::default()
        });
        start_roman_session(&mut engine, 1);
        let mut event = key_event(0, if option { "" } else { "\\" });
        event.input = "\\".into();
        event.intention = "\\".into();
        event.option = option;
        event.shift = shift;
        let state = response(engine.handle(envelope(2, Payload::KeyEvent(event))));
        assert_eq!(state.preedit, expected);
    }
}

#[test]
fn matches_desktop_japanese_symbol_intentions_in_roman_input() {
    let mut engine = configured_engine(ConversionConfig {
        live_conversion: false,
        ..Default::default()
    });
    start_roman_session(&mut engine, 1);

    for (index, (input, expected)) in [
        ("!", "！"),
        ("\"", "”"),
        ("#", "＃"),
        ("$", "＄"),
        ("%", "％"),
        ("&", "＆"),
        ("'", "’"),
        ("(", "（"),
        (")", "）"),
        ("=", "＝"),
        ("~", "〜"),
        ("|", "｜"),
        ("`", "｀"),
        ("{", "『"),
        ("+", "＋"),
        ("*", "＊"),
        ("}", "』"),
        ("<", "＜"),
        (">", "＞"),
        ("?", "？"),
        ("_", "＿"),
        ("-", "ー"),
        ("^", "＾"),
        ("\\", "￥"),
        ("¥", "￥"),
        ("@", "＠"),
        ("[", "「"),
        (";", "；"),
        (":", "："),
        ("]", "」"),
        (",", "、"),
        (".", "。"),
        ("/", "・"),
    ]
    .into_iter()
    .enumerate()
    {
        let request_id = 2 + index as u64 * 2;
        let state =
            response(engine.handle(envelope(request_id, Payload::KeyEvent(key_event(0, input)))));
        assert_eq!(state.preedit, expected, "unexpected mapping for {input}");
        response(engine.handle(envelope(
            request_id + 1,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
    }
}

#[test]
fn matches_desktop_option_symbol_intentions_while_composing() {
    let mut engine = configured_engine(ConversionConfig {
        live_conversion: false,
        ..Default::default()
    });
    start_roman_session(&mut engine, 1);

    for (index, (input, shift, expected)) in [
        ("/", false, "／"),
        ("?", true, "…"),
        ("[", false, "［"),
        ("{", true, "｛"),
        ("]", false, "］"),
        ("}", true, "｝"),
    ]
    .into_iter()
    .enumerate()
    {
        let request_id = 2 + index as u64 * 2;
        let mut event = key_event(0, "");
        event.input = input.into();
        event.intention = input.into();
        event.option = true;
        event.shift = shift;
        let state = response(engine.handle(envelope(request_id, Payload::KeyEvent(event))));
        assert_eq!(state.preedit, expected, "unexpected mapping for {input}");
        response(engine.handle(envelope(
            request_id + 1,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
    }
}

#[test]
fn inserts_printable_option_generated_characters() {
    let mut engine = configured_engine(ConversionConfig {
        live_conversion: false,
        ..Default::default()
    });
    start_roman_session(&mut engine, 1);

    for (index, (input, intention)) in [("¯", "<"), ("˘", ">")].into_iter().enumerate() {
        let request_id = 2 + index as u64 * 2;
        let mut event = key_event(0, "");
        event.input = input.into();
        event.intention = intention.into();
        event.option = true;
        event.shift = true;
        let state = response(engine.handle(envelope(request_id, Payload::KeyEvent(event))));
        assert_eq!(state.preedit, input);
        response(engine.handle(envelope(
            request_id + 1,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
    }
}

#[test]
fn consumes_undefined_control_shortcuts_only_during_composition() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    let idle = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Consume as i32,
            ..Default::default()
        }),
    )));
    assert!(!idle.consumed);

    response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0, "kana")))));
    let composing = response(engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Consume as i32,
            ..Default::default()
        }),
    )));
    assert!(composing.consumed);
    assert_eq!(composing.preedit, "かな");
}

#[test]
fn inverts_the_idle_space_width_when_half_space_is_enabled() {
    for (type_half_space, shift, expected) in [
        (false, false, "　"),
        (false, true, " "),
        (true, false, " "),
        (true, true, "　"),
    ] {
        let mut engine = configured_engine(ConversionConfig {
            type_half_space,
            ..Default::default()
        });
        start_roman_session(&mut engine, 1);
        let mut event = key_event(0x20, "");
        event.shift = shift;
        let state = response(engine.handle(envelope(2, Payload::KeyEvent(event))));
        assert!(state.consumed);
        assert_eq!(state.commit, expected);
    }
}

#[test]
fn directly_commits_full_width_text_for_option_input() {
    let mut engine = configured_engine(ConversionConfig {
        type_backslash: true,
        option_direct_full_width_input: true,
        ..Default::default()
    });
    start_roman_session(&mut engine, 1);

    for (request_id, input, expected) in [(2, "a", "ａ"), (3, "¥", "＼"), (4, "-", "－")] {
        let mut event = key_event(0, "");
        event.input = input.into();
        event.intention = input.into();
        event.option = true;
        let state = response(engine.handle(envelope(request_id, Payload::KeyEvent(event))));
        assert!(state.consumed);
        assert_eq!(state.commit, expected);
        assert!(state.preedit.is_empty());
        assert_eq!(state.input_state, protocol::InputState::None as i32);
    }
}

#[test]
fn applies_all_desktop_punctuation_styles_and_option_inversion() {
    for (style, comma, period) in [
        (PunctuationStyleConfig::KutenAndToten, "、", "。"),
        (PunctuationStyleConfig::KutenAndComma, "，", "。"),
        (PunctuationStyleConfig::PeriodAndToten, "、", "．"),
        (PunctuationStyleConfig::PeriodAndComma, "，", "．"),
    ] {
        let mut engine = configured_engine(ConversionConfig {
            punctuation_style: style,
            ..Default::default()
        });
        start_roman_session(&mut engine, 1);

        let comma_state =
            response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, ",")))));
        assert_eq!(comma_state.preedit, comma);

        response(engine.handle(envelope(
            3,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
        let period_state =
            response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0, ".")))));
        assert_eq!(period_state.preedit, period);

        response(engine.handle(envelope(
            5,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
        let mut option_comma = key_event(0, "");
        option_comma.input = ",".into();
        option_comma.intention = ",".into();
        option_comma.option = true;
        let inverted = response(engine.handle(envelope(6, Payload::KeyEvent(option_comma))));
        assert_eq!(inverted.preedit, if comma == "、" { "，" } else { "、" });

        response(engine.handle(envelope(
            7,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
        let mut option_period = key_event(0, "");
        option_period.input = ".".into();
        option_period.intention = ".".into();
        option_period.option = true;
        let inverted = response(engine.handle(envelope(8, Payload::KeyEvent(option_period))));
        assert_eq!(inverted.preedit, if period == "。" { "．" } else { "。" });
    }
}

#[test]
fn does_not_turn_control_key_text_into_composition() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    for (index, (name, key_sym, text)) in [
        ("BackSpace", 0xff08, "\u{8}"),
        ("Return", 0xff0d, "\r"),
        ("KP_Enter", 0xff8d, "\r"),
        ("Escape", 0xff1b, "\u{1b}"),
        ("Tab", 0xff09, "\t"),
        ("Delete", 0xffff, "\u{7f}"),
    ]
    .into_iter()
    .enumerate()
    {
        let request_id = index as u64 * 2 + 2;
        let state = response(engine.handle(envelope(
            request_id,
            Payload::KeyEvent(key_event(key_sym, text)),
        )));
        assert!(!state.consumed, "{name} must be returned to Fcitx5");
        assert!(state.preedit.is_empty(), "{name} created preedit text");
        assert!(state.commit.is_empty(), "{name} committed control text");

        let reset = response(engine.handle(envelope(
            request_id + 1,
            Payload::ResetSession(protocol::ResetSession {}),
        )));
        assert!(reset.reset);
    }
}

#[test]
fn enters_and_commits_desktop_unicode_input() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    let entered = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::StartUnicodeInput as i32,
            ..Default::default()
        }),
    )));
    assert!(entered.consumed);
    assert_eq!(entered.input_state, protocol::InputState::Unicode as i32);
    assert_eq!(entered.preedit, "U+");

    let typed = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0, "1f豆60")))));
    assert_eq!(typed.preedit, "U+1f60");

    let committed =
        response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0xff0d, "\r")))));
    assert_eq!(committed.commit, "ὠ");
    assert!(committed.reset);
    assert_eq!(committed.input_state, protocol::InputState::None as i32);
}

#[test]
fn commits_composition_before_entering_unicode_input() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));

    let entered = response(engine.handle(envelope(
        3,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::StartUnicodeInput as i32,
            ..Default::default()
        }),
    )));
    assert_eq!(entered.commit, "かな");
    assert_eq!(entered.preedit, "U+");
    assert_eq!(entered.input_state, protocol::InputState::Unicode as i32);
}

#[test]
fn switches_desktop_input_language_without_restarting_the_session() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    let english = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Eisu as i32,
            ..Default::default()
        }),
    )));
    assert!(english.consumed);
    assert_eq!(
        english.input_language,
        protocol::InputLanguage::English as i32
    );

    let direct = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0, "a")))));
    assert_eq!(direct.commit, "a");
    assert!(direct.preedit.is_empty());

    let space = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(space.commit, " ");

    let japanese = response(engine.handle(envelope(
        5,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Kana as i32,
            ..Default::default()
        }),
    )));
    assert_eq!(
        japanese.input_language,
        protocol::InputLanguage::Japanese as i32
    );
    let composing = response(engine.handle(envelope(6, Payload::KeyEvent(key_event(0, "a")))));
    assert_eq!(composing.preedit, "あ");
}

#[test]
fn keeps_composition_when_switching_to_english() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));

    let switched = response(engine.handle(envelope(
        3,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Eisu as i32,
            ..Default::default()
        }),
    )));
    assert_eq!(switched.preedit, composing.preedit);
    assert!(switched.commit.is_empty());
    assert_eq!(
        switched.input_language,
        protocol::InputLanguage::English as i32
    );
}

#[test]
fn appends_space_to_english_composition_with_and_without_live_conversion() {
    for live_conversion in [false, true] {
        let mut engine = Engine::open_with_conversion_resources(
            dictionary_root(),
            &ConversionConfig {
                live_conversion,
                ..Default::default()
            },
        )
        .unwrap();
        start_roman_session(&mut engine, 1);
        response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));
        response(engine.handle(envelope(
            3,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Eisu as i32,
                ..Default::default()
            }),
        )));

        let spaced = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0x20, "")))));

        assert!(spaced.consumed);
        assert_eq!(spaced.preedit, "かな ");
        assert!(spaced.commit.is_empty());
        assert_eq!(spaced.input_state, protocol::InputState::Composing as i32);
        assert_eq!(
            spaced.candidate_window,
            protocol::CandidateWindow::Hidden as i32
        );
    }
}

#[test]
fn edits_active_composition_with_backspace_and_consumes_navigation() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));
    assert_eq!(composing.preedit, "かな");

    let backspace =
        response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff08, "\u{8}")))));
    assert!(backspace.consumed);
    assert_eq!(backspace.preedit, "か");
    assert!(backspace.commit.is_empty());

    response(engine.handle(envelope(
        4,
        Payload::ResetSession(protocol::ResetSession {}),
    )));
    response(engine.handle(envelope(5, Payload::KeyEvent(key_event(0, "kana")))));
    let moved = response(engine.handle(envelope(6, Payload::KeyEvent(key_event(0xff51, "")))));
    assert!(moved.consumed);
    assert_eq!(moved.preedit_cursor, 2);
    assert_eq!(moved.preedit, "かな");
}

#[test]
fn commits_active_composition_with_return_and_keypad_enter() {
    for (name, key_sym) in [("Return", 0xff0d), ("KP_Enter", 0xff8d)] {
        let mut engine = Engine::open(dictionary_root()).unwrap();
        start_roman_session(&mut engine, 1);
        response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "shikai")))));

        let committed =
            response(engine.handle(envelope(3, Payload::KeyEvent(key_event(key_sym, "\r")))));
        assert!(committed.consumed, "{name} did not commit the composition");
        assert!(
            !committed.commit.is_empty(),
            "{name} returned an empty commit"
        );
        assert!(
            !committed.commit.chars().any(char::is_control),
            "{name} committed control text"
        );
        assert!(committed.preedit.is_empty());
    }
}

#[test]
fn escape_cancels_active_composition() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));

    let escaped =
        response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff1b, "\u{1b}")))));
    assert!(escaped.consumed);
    assert!(escaped.reset);
    assert!(escaped.preedit.is_empty());
    assert!(escaped.candidates.is_empty());
    assert!(escaped.commit.is_empty());
}

#[test]
fn consumes_tab_without_mutating_active_composition() {
    for (name, key_sym) in [("Tab", 0xff09), ("ISO_Left_Tab", 0xfe20)] {
        let mut engine = Engine::open(dictionary_root()).unwrap();
        start_roman_session(&mut engine, 1);
        let composing =
            response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kana")))));

        let tab = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(key_sym, "\t")))));
        assert!(tab.consumed, "{name} must be consumed while composing");
        assert_eq!(tab.preedit, composing.preedit, "{name} changed preedit");
        assert_eq!(
            tab.preedit_cursor, composing.preedit_cursor,
            "{name} moved the preedit cursor"
        );
        assert!(tab.commit.is_empty(), "{name} committed text");
    }
}

#[test]
fn follows_the_non_live_desktop_candidate_state_transitions() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "shikai")))));
    assert_eq!(
        composing.input_state,
        protocol::InputState::Composing as i32
    );
    assert_eq!(
        composing.candidate_window,
        protocol::CandidateWindow::Preview as i32
    );
    assert_eq!(composing.preedit, "しかい");

    let previewing = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(
        previewing.input_state,
        protocol::InputState::Previewing as i32
    );
    assert_eq!(
        previewing.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );
    assert_eq!(previewing.selected_candidate, -1);
    assert_ne!(previewing.preedit, "しかい");

    let selecting = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(
        selecting.input_state,
        protocol::InputState::Selecting as i32
    );
    assert_eq!(
        selecting.candidate_window,
        protocol::CandidateWindow::Selecting as i32
    );
    assert_eq!(selecting.selected_candidate, 0);

    let escaped = response(engine.handle(envelope(5, Payload::KeyEvent(key_event(0xff1b, "")))));
    assert_eq!(escaped.input_state, protocol::InputState::Previewing as i32);
    assert_eq!(
        escaped.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );

    let composing_again =
        response(engine.handle(envelope(6, Payload::KeyEvent(key_event(0xff1b, "")))));
    assert_eq!(
        composing_again.input_state,
        protocol::InputState::Composing as i32
    );
    assert_eq!(composing_again.preedit, "しかい");
}

#[test]
fn resolves_pending_roman_input_before_previewing_candidates() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            live_conversion: false,
            ..Default::default()
        },
    )
    .unwrap();
    start_roman_session(&mut engine, 1);

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "kan")))));
    assert_eq!(composing.preedit, "かn");

    let previewing = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(
        previewing.input_state,
        protocol::InputState::Previewing as i32
    );

    let composing = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0xff1b, "")))));
    assert_eq!(composing.preedit, "かん");
}

#[test]
fn follows_live_desktop_selection_and_backspace_behavior() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            live_conversion: true,
            ..Default::default()
        },
    )
    .unwrap();
    start_roman_session(&mut engine, 1);

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "shikai")))));
    assert_eq!(
        composing.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );
    assert_ne!(composing.preedit, "しかい");

    let selecting = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(selecting.selected_candidate, 0);
    assert_eq!(
        selecting.input_state,
        protocol::InputState::Selecting as i32
    );

    let next = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(next.selected_candidate, 1);
    let mut previous_space = key_event(0x20, "");
    previous_space.shift = true;
    let previous = response(engine.handle(envelope(5, Payload::KeyEvent(previous_space))));
    assert_eq!(previous.selected_candidate, 0);

    let raw = response(engine.handle(envelope(6, Payload::KeyEvent(key_event(0xff08, "")))));
    assert_eq!(raw.input_state, protocol::InputState::Composing as i32);
    assert_eq!(raw.preedit, "しか");
    assert_eq!(
        raw.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );
}

#[test]
fn keeps_a_single_character_raw_during_live_conversion() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            input_style: beankey_daemon::InputStyleConfig::Direct,
            live_conversion: true,
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "は")))));

    assert_eq!(composing.preedit, "は");
    assert_eq!(composing.highlighted_preedit_length, 0);
}

#[test]
fn exposes_the_desktop_dynamic_date_and_time_shortcuts() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let yesterday_before_last =
        response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "おととい")))));
    assert!(
        yesterday_before_last.candidates.iter().any(|candidate| {
            candidate.text.matches('/').count() == 1
                && candidate
                    .text
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '/')
        }),
        "candidates: {:?}",
        yesterday_before_last
            .candidates
            .iter()
            .map(|candidate| &candidate.text)
            .collect::<Vec<_>>()
    );
    assert!(
        yesterday_before_last
            .candidates
            .iter()
            .all(|candidate| !candidate.text.contains("<date "))
    );
}

#[test]
fn zero_commits_selected_marked_text_and_starts_a_new_composition() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "shikai")))));
    response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));

    let zero = response(engine.handle(envelope(4, Payload::KeyEvent(key_event(0, "0")))));
    assert!(zero.consumed);
    assert!(!zero.commit.is_empty());
    assert_eq!(zero.preedit, "0");
    assert_eq!(zero.input_state, protocol::InputState::Composing as i32);
}

#[test]
fn reveals_desktop_representation_candidates_progressively_above_the_first_candidate() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "shikai")))));
    response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));

    let expected = [
        vec!["ひらがな"],
        vec!["カタカナ", "ひらがな"],
        vec!["半角カナ", "カタカナ", "ひらがな"],
        vec!["全角英数", "半角カナ", "カタカナ", "ひらがな"],
        vec!["英数", "全角英数", "半角カナ", "カタカナ", "ひらがな"],
    ];
    for (offset, annotations) in expected.into_iter().enumerate() {
        let state = response(engine.handle(envelope(
            4 + offset as u64,
            Payload::KeyEvent(key_event(0xff52, "")),
        )));
        assert_eq!(state.selected_candidate, 0);
        assert_eq!(
            state
                .candidates
                .iter()
                .take(annotations.len())
                .map(|candidate| candidate.annotation.as_str())
                .collect::<Vec<_>>(),
            annotations
        );
    }
}

#[test]
fn exposes_manual_prediction_separately_and_accepts_it_with_tab() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            input_style: beankey_daemon::InputStyleConfig::Direct,
            japanese_prediction: beankey_daemon::PredictionConfig::Manual,
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let composing = response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "きょ")))));
    let prediction = composing
        .prediction
        .expect("manual prediction was not exposed");
    assert!(!prediction.display_text.is_empty());
    assert!(!prediction.append_text.is_empty());
    assert!(
        composing
            .candidates
            .iter()
            .all(|candidate| candidate.text != prediction.display_text)
    );

    let accepted = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff09, "")))));
    assert!(accepted.consumed);
    assert_ne!(accepted.preedit, composing.preedit);
}

#[test]
fn exposes_manual_prediction_from_a_katakana_prefix() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            japanese_prediction: beankey_daemon::PredictionConfig::Manual,
            live_conversion: false,
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let composing = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "キョ".into(),
            input: "キョ".into(),
            intention: "キョ".into(),
            ..Default::default()
        }),
    )));

    let prediction = composing
        .prediction
        .expect("katakana prefix did not expose a prediction");
    assert!(!prediction.append_text.is_empty());
    assert!(prediction.append_text.starts_with('う'));
}

#[test]
fn shift_navigation_edits_the_selected_segment_length() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "きょうはあめ")))));
    let initial = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));
    let initial_remaining =
        initial.preedit.chars().count() - initial.highlighted_preedit_length as usize;

    let mut shift_right = key_event(0xff53, "");
    shift_right.shift = true;
    let one = response(engine.handle(envelope(4, Payload::KeyEvent(shift_right.clone()))));
    assert!(one.highlighted_preedit_length > 0);
    let one_remaining = one.preedit.chars().count() - one.highlighted_preedit_length as usize;
    assert_eq!(one_remaining + 1, initial_remaining);

    let two = response(engine.handle(envelope(5, Payload::KeyEvent(shift_right))));
    let two_remaining = two.preedit.chars().count() - two.highlighted_preedit_length as usize;
    assert_eq!(two_remaining + 1, one_remaining);

    let mut shift_left = key_event(0xff51, "");
    shift_left.shift = true;
    let one_again = response(engine.handle(envelope(6, Payload::KeyEvent(shift_left))));
    let one_again_remaining =
        one_again.preedit.chars().count() - one_again.highlighted_preedit_length as usize;
    assert_eq!(one_again_remaining, one_remaining);
}

#[test]
fn transformed_candidate_commits_only_the_selected_segment() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    response(engine.handle(envelope(2, Payload::KeyEvent(key_event(0, "きょうはあめ")))));
    let selecting = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));
    assert!(selecting.highlighted_preedit_length < selecting.preedit.chars().count() as u32);

    let transformed = response(engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Hiragana as i32,
            ..Default::default()
        }),
    )));
    assert!(transformed.consumed);
    assert!(!transformed.commit.is_empty());
    assert!(!transformed.reset);
    assert!(!transformed.preedit.is_empty());
    assert_eq!(
        transformed.input_state,
        protocol::InputState::Selecting as i32
    );
}

#[test]
fn returns_enter_after_committing_a_candidate() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    start_roman_session(&mut engine, 1);
    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "shikai".into(),
            ..Default::default()
        }),
    )));
    let index = converted
        .candidates
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap()
        .index;
    let committed = response(engine.handle(envelope(
        3,
        Payload::SelectCandidate(protocol::SelectCandidate { index }),
    )));
    assert_eq!(committed.commit, "司会");
    assert!(committed.candidates.is_empty());

    let enter = response(engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }),
    )));
    assert!(
        !enter.consumed,
        "Enter selected a post-composition prediction"
    );
    assert!(enter.commit.is_empty());
    assert!(enter.preedit.is_empty());
}

#[test]
fn learns_a_previewed_candidate_committed_with_enter() {
    let state = TempDir::new().unwrap();
    let mut engine = Engine::open_with_learning(dictionary_root(), state.path()).unwrap();
    start_roman_session(&mut engine, 1);
    response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "shikai".into(),
            ..Default::default()
        }),
    )));
    let previewing = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0x20, "")))));
    assert_eq!(
        previewing.input_state,
        protocol::InputState::Previewing as i32
    );
    assert_ne!(previewing.preedit, "しかい");

    let entered = response(engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }),
    )));

    assert_eq!(entered.commit, previewing.preedit);
    assert!(entered.reset);
    assert!(!state.path().join("memory.louds").exists());
    response(engine.handle(envelope(5, Payload::EndSession(protocol::EndSession {}))));
    for file in [
        "memory.louds",
        "memory.loudschars2",
        "memory.memorymetadata",
        "memory0.loudstxt3",
    ] {
        assert!(state.path().join(file).exists(), "missing {file}");
    }
}

#[test]
fn does_not_persist_control_key_text_in_learning_memory() {
    for (name, key_sym, text) in [
        ("BackSpace", 0xff08, "\u{8}"),
        ("Return", 0xff0d, "\r"),
        ("KP_Enter", 0xff8d, "\r"),
        ("Escape", 0xff1b, "\u{1b}"),
        ("Tab", 0xff09, "\t"),
        ("ISO_Left_Tab", 0xfe20, "\t"),
        ("Delete", 0xffff, "\u{7f}"),
    ] {
        let state = TempDir::new().unwrap();
        let mut engine = Engine::open_with_learning(dictionary_root(), state.path()).unwrap();
        start_roman_session(&mut engine, 1);
        response(engine.handle(envelope(2, Payload::KeyEvent(key_event(key_sym, text)))));
        response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff0d, "")))));
        response(engine.handle(envelope(4, Payload::EndSession(protocol::EndSession {}))));

        assert_eq!(
            std::fs::read_dir(state.path()).unwrap().count(),
            0,
            "{name} created persistent learning state for control text"
        );
    }
}

#[test]
fn converts_selects_commits_and_resets_a_session() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "shikai".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    )));
    assert!(converted.consumed);
    assert_eq!(converted.preedit, "しかい");
    assert_eq!(
        converted.candidate_window,
        protocol::CandidateWindow::Preview as i32
    );
    let index = converted
        .candidates
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap()
        .index;

    let selected = response(engine.handle(envelope(
        3,
        Payload::SelectCandidate(protocol::SelectCandidate { index }),
    )));
    assert_eq!(selected.commit, "司会");
    assert!(selected.preedit.is_empty());
    assert!(selected.reset);
    assert!(selected.candidates.is_empty());

    let reset = response(engine.handle(envelope(
        4,
        Payload::ResetSession(protocol::ResetSession {}),
    )));
    assert!(reset.reset);
    assert!(reset.preedit.is_empty());
}

#[test]
fn uses_the_configured_input_style_when_the_addon_does_not_override_it() {
    let mut engine =
        Engine::open_with_conversion_resources(dictionary_root(), &ConversionConfig::default())
            .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Unspecified as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "shikai".into(),
            ..Default::default()
        }),
    )));

    assert_ne!(converted.preedit, "shikai");
    assert_eq!(
        converted.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );
    assert!(
        converted
            .candidates
            .iter()
            .any(|candidate| candidate.text == "司会")
    );
}

#[test]
fn forwards_explicit_kana_key_intention_and_input() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::KanaJis as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "q".into(),
            surrounding_text: None,
            input: "q".into(),
            intention: "q".into(),
            option: false,
        }),
    )));
    assert!(converted.consumed);
    assert_eq!(converted.preedit, "た");
}

#[test]
fn completes_foreign_input_with_the_configured_hunspell_assets() {
    let mut engine = Engine::open_with_hunspell(
        dictionary_root(),
        env!("BEANKEY_TEST_EN_US_DICTIONARY"),
        env!("BEANKEY_TEST_EL_GR_DICTIONARY"),
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::EnglishUs as i32,
            custom_input_table: String::new(),
        }),
    )));
    let english = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "hel".into(),
            ..Default::default()
        }),
    )));
    assert!(
        english
            .candidates
            .iter()
            .any(|candidate| candidate.text.len() > 3 && candidate.text.starts_with("hel"))
    );

    response(engine.handle(envelope(
        3,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Greek as i32,
            custom_input_table: String::new(),
        }),
    )));
    let greek = response(engine.handle(envelope(
        4,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "καλ".into(),
            ..Default::default()
        }),
    )));
    assert!(
        greek
            .candidates
            .iter()
            .any(|candidate| candidate.text == "καλά")
    );
}

#[test]
fn persists_forgets_and_resets_learning_through_session_requests() {
    let state = TempDir::new().unwrap();
    let mut engine = Engine::open_with_learning(dictionary_root(), state.path()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "shikai".into(),
            ..Default::default()
        }),
    )));
    let selected = converted
        .candidates
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap()
        .index;
    response(engine.handle(envelope(
        3,
        Payload::SelectCandidate(protocol::SelectCandidate { index: selected }),
    )));
    let memory_path = state.path().join("memory.memorymetadata");
    assert!(!memory_path.exists());
    response(engine.handle(envelope(4, Payload::EndSession(protocol::EndSession {}))));
    let learned_size = std::fs::metadata(&memory_path).unwrap().len();
    drop(engine);

    let mut restarted = Engine::open_with_learning(dictionary_root(), state.path()).unwrap();
    response(restarted.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let converted = response(restarted.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "shikai".into(),
            ..Default::default()
        }),
    )));
    let learned = converted
        .candidates
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap()
        .index;
    response(restarted.handle(envelope(
        3,
        Payload::ForgetCandidate(protocol::ForgetCandidate { index: learned }),
    )));
    assert!(std::fs::metadata(&memory_path).unwrap().len() < learned_size);

    response(restarted.handle(envelope(
        4,
        Payload::ResetLearning(protocol::ResetLearning {}),
    )));
    assert!(!memory_path.exists());
}

#[test]
fn does_not_offer_post_composition_predictions_in_the_desktop_ux() {
    let mut engine = Engine::open_with_emoji(dictionary_root(), emoji_dictionary_path()).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::RomanToKana as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "egao".into(),
            ..Default::default()
        }),
    )));
    let smile = converted
        .candidates
        .iter()
        .find(|candidate| candidate.text == "笑顔")
        .unwrap()
        .index;
    let committed = response(engine.handle(envelope(
        3,
        Payload::SelectCandidate(protocol::SelectCandidate { index: smile }),
    )));
    assert_eq!(committed.commit, "笑顔");
    assert!(committed.preedit.is_empty());
    assert!(committed.reset);
    assert!(committed.candidates.is_empty());
}

#[test]
fn uses_a_json_user_dictionary_with_a_named_custom_input_table() {
    let resources = TempDir::new().unwrap();
    let user_dictionary = resources.path().join("user.json");
    let input_table = resources.path().join("greeting.tsv");
    std::fs::write(
        &user_dictionary,
        r#"[{"word":"挨拶語","reading":"あいさつ","hint":"test"}]"#,
    )
    .unwrap();
    std::fs::write(&input_table, "qq\tあいさつ\n").unwrap();
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            user_dictionary: Some(user_dictionary),
            user_dictionary_directory: None,
            custom_input_tables: BTreeMap::from([("greeting".into(), input_table)]),
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Custom as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: "greeting".into(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "qq".into(),
            ..Default::default()
        }),
    )));

    assert_ne!(converted.preedit, "qq");
    assert_eq!(
        converted.candidate_window,
        protocol::CandidateWindow::Hidden as i32
    );
    assert!(
        converted
            .candidates
            .iter()
            .any(|candidate| candidate.text == "挨拶語")
    );
}

#[test]
fn provides_desktop_and_beankey_product_name_candidates_by_default() {
    for (reading, expected) in [("あずーきー", "azooKey"), ("びーんきー", "beanKey")] {
        let mut engine = Engine::open(dictionary_root()).unwrap();
        response(engine.handle(envelope(
            1,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Direct as i32,
                surrounding_text: None,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                custom_input_table: String::new(),
            }),
        )));

        let converted = response(engine.handle(envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: reading.into(),
                input: reading.into(),
                intention: reading.into(),
                ..Default::default()
            }),
        )));

        assert!(
            converted
                .candidates
                .iter()
                .any(|candidate| candidate.text == expected),
            "missing {expected} for {reading}"
        );
    }
}

#[test]
fn keeps_product_name_candidates_when_a_json_dictionary_is_loaded() {
    let resources = TempDir::new().unwrap();
    let user_dictionary = resources.path().join("user.json");
    std::fs::write(
        &user_dictionary,
        r#"[{"word":"追加語","reading":"びーんきー","hint":"test"}]"#,
    )
    .unwrap();
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            user_dictionary: Some(user_dictionary),
            live_conversion: false,
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "びーんきー".into(),
            input: "びーんきー".into(),
            intention: "びーんきー".into(),
            ..Default::default()
        }),
    )));
    assert!(["beanKey", "追加語"].into_iter().all(|expected| {
        converted
            .candidates
            .iter()
            .any(|candidate| candidate.text == expected)
    }));
}

#[test]
fn applies_conversion_options_and_displays_live_conversion() {
    let mut engine = Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            n_best: 3,
            live_conversion: true,
            full_width_roman: true,
            typography: true,
            ..Default::default()
        },
    )
    .unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "かなかんじへんかん".into(),
            ..Default::default()
        }),
    )));

    assert_ne!(converted.preedit, "かなかんじへんかん");
    assert_eq!(
        converted.preedit_cursor as usize,
        converted.preedit.chars().count()
    );
    assert!(converted.candidates.len() >= 3);
    let committed = response(engine.handle(envelope(
        3,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Enter as i32,
            ..Default::default()
        }),
    )));
    assert_eq!(committed.commit, converted.preedit);

    response(engine.handle(envelope(
        4,
        Payload::ResetSession(protocol::ResetSession {}),
    )));
    let representations = response(engine.handle(envelope(
        5,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            text: "ABC".into(),
            ..Default::default()
        }),
    )));
    assert!(
        representations
            .candidates
            .iter()
            .any(|candidate| candidate.text == "ＡＢＣ")
    );
}

#[test]
fn rejects_an_invalid_custom_input_table() {
    let resources = TempDir::new().unwrap();
    let input_table = resources.path().join("invalid.tsv");
    std::fs::write(&input_table, "missing separator\n").unwrap();

    let error = match Engine::open_with_conversion_resources(
        dictionary_root(),
        &ConversionConfig {
            custom_input_tables: BTreeMap::from([("invalid".into(), input_table)]),
            ..Default::default()
        },
    ) {
        Ok(_) => panic!("invalid input table was accepted"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        EngineOpenError::ConversionResource(ConversionResourceError::InvalidInputTable {
            name
        }) if name == "invalid"
    ));
}

#[test]
fn isolates_sessions_and_rejects_out_of_order_requests() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    response(engine.handle(envelope(
        2,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    let error = engine.handle(envelope(
        1,
        Payload::ResetSession(protocol::ResetSession {}),
    ));
    assert!(matches!(error.payload, Some(Payload::ProtocolError(_))));

    let mut other = envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    );
    other.session_id = "other".into();
    response(engine.handle(other));
}

#[test]
fn rejects_invalid_surrounding_text_and_resets_the_session() {
    let mut engine = Engine::open(dictionary_root()).unwrap();
    let rejected = engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: Some(protocol::SurroundingText {
                available: true,
                text: "文脈".into(),
                cursor: 3,
                anchor: 0,
            }),
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    ));

    let Some(Payload::ProtocolError(error)) = rejected.payload else {
        panic!("expected a protocol error");
    };
    assert_eq!(
        error.code,
        protocol::protocol_error::Code::InvalidPayload as i32
    );
}

#[derive(Default)]
struct PrefixModel {
    token_piece_calls: Option<Arc<AtomicUsize>>,
}

struct ContextRecordingModel {
    prompts: Arc<Mutex<Vec<String>>>,
}

impl ZenzLanguageModel for ContextRecordingModel {
    fn vocabulary_size(&self) -> usize {
        3
    }

    fn eos_token(&self) -> i32 {
        2
    }

    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        if add_special {
            self.prompts.lock().unwrap().push(text.to_owned());
        }
        Ok(vec![1])
    }

    fn token_to_piece(&mut self, _token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        Ok(b"x".to_vec())
    }

    fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        Ok(vec![0.0, 10.0, 0.0])
    }
}

impl ZenzLanguageModel for PrefixModel {
    fn vocabulary_size(&self) -> usize {
        5
    }

    fn eos_token(&self) -> i32 {
        2
    }

    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        Ok(if add_special {
            vec![1]
        } else if text == "箸" {
            vec![4]
        } else {
            vec![3]
        })
    }

    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        if let Some(calls) = &self.token_piece_calls {
            calls.fetch_add(1, Ordering::Relaxed);
        }
        Ok(if token == 4 {
            "箸".as_bytes().to_vec()
        } else {
            b"x".to_vec()
        })
    }

    fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        Ok(vec![0.0, 0.0, 0.0, 1.0, 10.0])
    }
}

#[test]
fn applies_zenz_prefix_correction_through_the_session_engine() {
    let mut engine =
        Engine::open_with_zenz_model(dictionary_root(), Box::new(PrefixModel::default())).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "はし".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    )));

    assert!(
        converted
            .candidates
            .iter()
            .take(5)
            .any(|candidate| candidate.text == "箸")
    );
}

#[test]
fn requests_rich_zenz_candidates_when_candidate_selection_starts() {
    let token_piece_calls = Arc::new(AtomicUsize::new(0));
    let model = PrefixModel {
        token_piece_calls: Some(Arc::clone(&token_piece_calls)),
    };
    let mut engine = Engine::open_with_zenz_model(dictionary_root(), Box::new(model)).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "はし".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    )));
    let calls_before_selection = token_piece_calls.load(Ordering::Relaxed);

    let selecting = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));

    assert_eq!(
        selecting.input_state,
        protocol::InputState::Selecting as i32
    );
    assert!(
        token_piece_calls.load(Ordering::Relaxed) >= calls_before_selection + 3,
        "candidate selection must request the candidate token and rich alternatives"
    );
}

#[test]
fn appends_a_partial_commit_to_the_immediate_zenz_left_context() {
    let input = "きょうはあめ";
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let model = ContextRecordingModel {
        prompts: Arc::clone(&prompts),
    };
    let mut engine = Engine::open_with_zenz_model(dictionary_root(), Box::new(model)).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: Some(protocol::SurroundingText {
                available: true,
                text: "前".into(),
                cursor: 1,
                anchor: 1,
            }),
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));
    response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: input.into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    )));
    let selecting = response(engine.handle(envelope(3, Payload::KeyEvent(key_event(0xff54, "")))));
    let input_surface_count = input.chars().count() as u32;
    let partial_index = selecting
        .candidates
        .iter()
        .filter_map(|candidate| {
            let protocol::composing_count::Count::Surface(count) =
                candidate.composing_count.as_ref()?.count.as_ref()?
            else {
                return None;
            };
            (*count < input_surface_count).then_some((candidate.index, *count))
        })
        .max_by_key(|(_, count)| *count)
        .map(|(index, _)| index)
        .expect("rich candidates must include a partial first-clause conversion");
    let prompt_count_before_commit = prompts.lock().unwrap().len();

    let committed = response(engine.handle(envelope(
        4,
        Payload::SelectCandidate(protocol::SelectCandidate {
            index: partial_index,
        }),
    )));

    assert!(!committed.reset);
    assert!(!committed.commit.is_empty());
    let expected_left_context = format!("前{}", committed.commit);
    assert!(
        prompts
            .lock()
            .unwrap()
            .iter()
            .skip(prompt_count_before_commit)
            .any(|prompt| prompt.contains(&expected_left_context)),
        "the remainder request must observe the just-committed prefix"
    );
}

#[test]
fn preserves_special_candidates_after_zenz_conversion() {
    let mut engine =
        Engine::open_with_zenz_model(dictionary_root(), Box::new(PrefixModel::default())).unwrap();
    response(engine.handle(envelope(
        1,
        Payload::StartSession(protocol::StartSession {
            input_style: protocol::InputStyle::Direct as i32,
            surrounding_text: None,
            keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
            custom_input_table: String::new(),
        }),
    )));

    let converted = response(engine.handle(envelope(
        2,
        Payload::KeyEvent(protocol::KeyEvent {
            action: protocol::UserAction::Input as i32,
            shift: false,
            text: "U+1F600".into(),
            surrounding_text: None,
            input: String::new(),
            intention: String::new(),
            option: false,
        }),
    )));

    assert!(
        converted
            .candidates
            .iter()
            .any(|candidate| candidate.text == "😀")
    );
}
