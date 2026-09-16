use std::path::PathBuf;

use bean_key_converter::{
    ComposingCount, ComposingText, ConversionSession, DictionaryStore, InputStyle,
    InputTableRegistry, NormalConverter, PredictionMode, RequestOptions,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn converts_direct_kana_with_the_fixed_dictionary() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut composing = ComposingText::new();
    composing.insert_str("しかい", InputStyle::Direct, &tables);

    let candidates = converter.convert(&composing, &tables, 10).unwrap();

    assert!(candidates.iter().any(|candidate| candidate.text == "司会"));
    assert!(candidates.iter().any(|candidate| candidate.text == "視界"));
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.composing_count == ComposingCount::Surface(3))
    );
    assert!(
        candidates
            .windows(2)
            .all(|pair| pair[0].value >= pair[1].value)
    );
}

#[test]
fn converts_roman_history_through_the_surface_lattice() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut composing = ComposingText::new();
    composing.insert_str("shikai", InputStyle::RomanToKana, &tables);
    assert_eq!(composing.surface(), "しかい");

    let candidates = converter.convert(&composing, &tables, 10).unwrap();

    assert!(candidates.iter().any(|candidate| candidate.text == "司会"));
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.composing_count == ComposingCount::Surface(3))
    );
}

#[test]
fn matches_the_fixed_upstream_must_convert_cases_for_full_and_gradual_input() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let options = RequestOptions {
        japanese_prediction: PredictionMode::Disabled,
        ..RequestOptions::default()
    };

    for (style, cases) in [
        (
            InputStyle::Direct,
            &[
                ("つかっている", "使っている"),
                ("しんだどうぶつ", "死んだ動物"),
                ("けいさん", "計算"),
                ("azooKeyをつかう", "azooKeyを使う"),
                ("じどうAIそうじゅう。", "自動AI操縦。"),
                ("1234567890123456789012", "1234567890123456789012"),
            ][..],
        ),
        (
            InputStyle::RomanToKana,
            &[
                ("tukatteiru", "使っている"),
                ("sindadoubutu", "死んだ動物"),
                ("keisann", "計算"),
            ][..],
        ),
    ] {
        for &(input, expected) in cases {
            let mut full = ConversionSession::new();
            full.insert_str(input, style.clone(), &tables);
            assert_eq!(
                full.request(&converter, &tables, options.clone())
                    .unwrap()
                    .main_results
                    .first()
                    .map(|candidate| candidate.text.as_str()),
                Some(expected),
                "full conversion for {input}"
            );

            let mut gradual = ConversionSession::new();
            let mut result = None;
            for character in input.chars() {
                gradual.insert_str(&character.to_string(), style.clone(), &tables);
                result = Some(
                    gradual
                        .request(&converter, &tables, options.clone())
                        .unwrap(),
                );
            }
            assert_eq!(
                result
                    .as_ref()
                    .and_then(|result| result.main_results.first())
                    .map(|candidate| candidate.text.as_str()),
                Some(expected),
                "gradual conversion for {input}"
            );
        }
    }
}

#[test]
fn never_expands_plain_backslash_n_into_a_line_break() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("\\n", InputStyle::Direct, &tables);

    let result = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap();

    assert!(
        result
            .main_results
            .iter()
            .all(|candidate| candidate.text != "\n")
    );
}
