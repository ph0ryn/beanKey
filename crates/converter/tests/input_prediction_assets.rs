use std::path::PathBuf;

use beankey_converter::{
    ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn predicts_longer_words_from_a_direct_kana_prefix() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("きょ", InputStyle::Direct, &tables);

    let predictions = session
        .request_predictions(&converter, &tables, 10)
        .unwrap();

    assert!(predictions.iter().any(|candidate| candidate.text == "今日"));
    assert!(predictions.iter().all(|candidate| candidate.ruby_count > 2));
    assert!(
        predictions
            .windows(2)
            .all(|pair| pair[0].value >= pair[1].value)
    );
}

#[test]
fn does_not_predict_from_only_an_unresolved_roman_prefix() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("ky", InputStyle::RomanToKana, &tables);

    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn keeps_compatible_predictions_while_a_roman_suffix_is_unresolved() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("kyou", InputStyle::RomanToKana, &tables);

    let initial = session
        .request_predictions(&converter, &tables, 20)
        .unwrap();
    let expected = initial
        .iter()
        .find(|candidate| {
            candidate
                .entries
                .iter()
                .any(|entry| entry.ruby.starts_with("キョウハ"))
        })
        .map(|candidate| candidate.text.clone())
        .unwrap();

    session.insert_str("h", InputStyle::RomanToKana, &tables);
    assert_eq!(session.composing().surface(), "きょうh");
    let unresolved = session
        .request_predictions(&converter, &tables, 20)
        .unwrap();

    assert_eq!(unresolved[0].text, expected);
    assert!(
        unresolved
            .iter()
            .any(|candidate| candidate.text == expected)
    );
    assert!(unresolved.iter().all(|candidate| {
        candidate.composing_count == beankey_converter::ComposingCount::Surface(4)
    }));
}

#[test]
fn predicts_from_the_last_clause_while_preserving_the_preceding_conversion() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("これはきょう", InputStyle::Direct, &tables);

    let predictions = session
        .request_predictions(&converter, &tables, 20)
        .unwrap();

    assert!(predictions.iter().any(|candidate| {
        candidate.text.starts_with("これは今日")
            && candidate
                .entries
                .iter()
                .map(|entry| entry.ruby.as_str())
                .collect::<String>()
                .starts_with("コレハキョウ")
            && candidate.ruby_count > 6
    }));
}
