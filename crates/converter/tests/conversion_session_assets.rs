use std::path::PathBuf;

use beankey_converter::{
    ComposingCount, ConversionSession, DictionaryStore, InputStyle, InputTableId,
    InputTableRegistry, NormalConverter, TextTransform,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn observed_consumption_matches_completion_without_changing_composition() {
    let tables = InputTableRegistry::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let mut fixture = ConversionSession::new();
    fixture.insert_str("かな", InputStyle::Direct, &tables);
    let template = fixture.request_candidates(&converter, &tables, 1).unwrap()[0].clone();
    for (input, style) in [
        ("きょうはか\u{3099}👨‍👩‍👧", InputStyle::Direct),
        ("gakkouhakyoun", InputStyle::RomanToKana),
        (
            "kyouhasshi",
            InputStyle::Mapped(InputTableId::DefaultRomanToKana),
        ),
        ("kakt", InputStyle::Mapped(InputTableId::DefaultAzik)),
        ("t@g", InputStyle::Mapped(InputTableId::DefaultKanaJis)),
    ] {
        let mut session = ConversionSession::new();
        session.insert_str(input, style, &tables);
        let original = session.composing().clone();
        let mut counts = (0..=original.surface_graphemes().len() + 2)
            .map(ComposingCount::Surface)
            .chain((0..=original.input().len() + 2).map(ComposingCount::Input))
            .collect::<Vec<_>>();
        counts.extend([
            ComposingCount::Composite(
                Box::new(ComposingCount::Surface(1)),
                Box::new(ComposingCount::Surface(2)),
            ),
            ComposingCount::Composite(
                Box::new(ComposingCount::Input(1)),
                Box::new(ComposingCount::Surface(2)),
            ),
        ]);
        for count in counts {
            let mut candidate = template.clone();
            candidate.composing_count = count.clone();
            let mut completed = original.clone();
            completed.complete_prefix(count.clone(), &tables);
            assert_eq!(
                session.consumed_surface_count(&candidate, &tables),
                original.surface_graphemes().len() - completed.surface_graphemes().len(),
                "input={input:?}, count={count:?}"
            );
            assert_eq!(session.composing(), &original);
        }
    }
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
