use std::path::PathBuf;

use beankey_converter::{
    ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
    PredictionMode, PrefixConstraint, RequestOptions, TypoCorrectionMode,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn assembles_a_zenz_predictive_input_override() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut prediction_source = ConversionSession::new();
    prediction_source.insert_str("かな", InputStyle::Direct, &tables);
    let prediction_override = prediction_source
        .request_predictions(&converter, &tables, 3)
        .unwrap();
    assert!(!prediction_override.is_empty());

    let mut session = ConversionSession::new();
    session.insert_str("kyo", InputStyle::RomanToKana, &tables);
    session
        .request_zenz_draft(&converter, &tables, 2, &PrefixConstraint::default())
        .unwrap();
    let result = session
        .finalize_zenz_request_with_prediction_override(
            &converter,
            &tables,
            RequestOptions {
                japanese_prediction: PredictionMode::Manual,
                ..RequestOptions::default()
            },
            Some(prediction_override.clone()),
        )
        .unwrap();

    assert_eq!(result.prediction_results, prediction_override);
    assert!(
        result
            .main_results
            .iter()
            .all(|candidate| !result.prediction_results.contains(candidate))
    );
}

#[test]
fn corrects_fixed_upstream_direct_and_roman_typo_rules() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let options = RequestOptions {
        japanese_prediction: PredictionMode::Disabled,
        typo_correction: TypoCorrectionMode::Enabled,
        ..RequestOptions::default()
    };

    for (input, ruby, expected) in [
        ("たいかくせい", "タイカクセイ", "大学生"),
        ("きみのことかすき", "キミノコトカスキ", "君のことが好き"),
        (
            "おへんとうをもつていく",
            "オヘントウヲモツテイク",
            "お弁当を持っていく",
        ),
    ] {
        let mut session = ConversionSession::new();
        session.insert_str(input, InputStyle::Direct, &tables);
        let result = session
            .request(&converter, &tables, options.clone())
            .unwrap();
        assert_eq!(
            result.main_results.first().map(|item| item.text.as_str()),
            Some(expected)
        );
        assert!(result.main_results[0].is_typo_correction);
        assert!(result.main_results.iter().take(3).any(|candidate| {
            !candidate.is_typo_correction
                && candidate
                    .entries
                    .iter()
                    .map(|entry| entry.ruby.as_str())
                    .collect::<String>()
                    == ruby
        }));
    }

    let mut roman = ConversionSession::new();
    roman.insert_str("li", InputStyle::RomanToKana, &tables);
    let result = roman.request(&converter, &tables, options).unwrap();
    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "木" && candidate.is_typo_correction)
    );
}

#[test]
fn assembles_full_clause_word_representation_and_prediction_groups() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("kyo", InputStyle::RomanToKana, &tables);

    let result = session
        .request(
            &converter,
            &tables,
            RequestOptions {
                japanese_prediction: PredictionMode::Manual,
                half_width_kana: true,
                ..RequestOptions::default()
            },
        )
        .unwrap();

    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "巨")
    );
    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "キョ")
    );
    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "きょ")
    );
    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "ｷｮ")
    );
    assert!(
        result
            .prediction_results
            .iter()
            .any(|candidate| candidate.text == "今日")
    );
    assert!(result.english_prediction_results.is_empty());
    assert_eq!(session.candidates(), result.main_results);
}
