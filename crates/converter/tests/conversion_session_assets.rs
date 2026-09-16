use std::path::PathBuf;

use bean_key_converter::{
    ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
    TextTransform,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn transforms_composition_for_the_desktop_function_keys() {
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("gattu", InputStyle::RomanToKana, &tables);

    assert_eq!(session.transformed_text(TextTransform::Hiragana), "がっつ");
    assert_eq!(session.transformed_text(TextTransform::Katakana), "ガッツ");
    assert_eq!(
        session.transformed_text(TextTransform::HalfWidthKatakana),
        "ｶﾞｯﾂ"
    );
    assert_eq!(
        session.transformed_text(TextTransform::FullWidthRoman),
        "ｇａｔｔｕ"
    );
    assert_eq!(
        session.transformed_text(TextTransform::HalfWidthRoman),
        "gattu"
    );
}

#[test]
fn resolves_pending_roman_input_at_the_desktop_conversion_boundary() {
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("kan", InputStyle::RomanToKana, &tables);
    assert_eq!(session.composing().surface(), "かn");

    session.insert_composition_separator(InputStyle::RomanToKana, &tables);
    assert_eq!(session.composing().surface(), "かん");
    let input_count = session.composing().input().len();

    session.insert_composition_separator(InputStyle::RomanToKana, &tables);
    assert_eq!(session.composing().surface(), "かん");
    assert_eq!(session.composing().input().len(), input_count);
}

#[test]
fn selects_and_commits_a_candidate_without_leaking_between_sessions() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut first = ConversionSession::new();
    let mut second = ConversionSession::new();
    first.insert_str("shikai", InputStyle::RomanToKana, &tables);
    second.insert_str("かな", InputStyle::Direct, &tables);

    let candidates = first.request_candidates(&converter, &tables, 10).unwrap();
    let index = candidates
        .iter()
        .position(|candidate| candidate.text == "司会")
        .unwrap();
    let committed = first.select_candidate(index, &tables).unwrap();

    assert_eq!(committed, "司会");
    assert!(first.composing().is_empty());
    assert_eq!(second.composing().surface(), "かな");
    assert!(second.candidates().is_empty());
}

#[test]
fn partially_commits_the_first_clause_and_converts_the_remainder() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("わがはいはねこ", InputStyle::Direct, &tables);

    let candidates = session.request_candidates(&converter, &tables, 10).unwrap();
    let index = candidates
        .iter()
        .position(|candidate| candidate.text == "吾輩は")
        .unwrap();
    assert_eq!(session.select_candidate(index, &tables).unwrap(), "吾輩は");
    assert_eq!(session.composing().surface(), "ねこ");

    let remaining = session.request_candidates(&converter, &tables, 10).unwrap();
    assert!(remaining.iter().any(|candidate| candidate.text == "猫"));
}
