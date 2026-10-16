use std::path::PathBuf;

use beankey_converter::{
    Candidate, ConversionSession, DictionaryEntry, DictionaryMetadata, DictionaryStore, InputStyle,
    InputTableRegistry, NormalConverter, PredictionMode, PrefixConstraint, RequestOptions,
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

fn conversion_candidate(
    converter: &NormalConverter<'_>,
    tables: &InputTableRegistry,
    reading: &str,
    text: &str,
) -> Candidate {
    let mut source = ConversionSession::new();
    source.insert_str(reading, InputStyle::Direct, tables);
    source
        .request(converter, tables, RequestOptions::default())
        .unwrap()
        .main_results
        .into_iter()
        .find(|candidate| candidate.text == text)
        .unwrap_or_else(|| panic!("{text} was not a candidate for {reading}"))
}

#[test]
fn accepts_a_prediction_reading_and_continues_roman_input() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    for (input, reading, text, continuation, expected) in [
        ("hida", "ひだり", "←", "nimagaru", "ひだりにまがる"),
        (
            "arigat",
            "ありがとう",
            "有難う",
            "gozaimasu",
            "ありがとうございます",
        ),
    ] {
        let candidate = conversion_candidate(&converter, &tables, reading, text);
        let mut session = ConversionSession::new();
        session.insert_str(input, InputStyle::RomanToKana, &tables);
        assert!(session.accept_prediction_candidate(&candidate, &tables));
        assert_eq!(session.composing().surface(), reading);
        session.insert_str(continuation, InputStyle::RomanToKana, &tables);
        assert_eq!(session.composing().surface(), expected);
    }
}

#[test]
fn rejects_stale_predictions_and_acceptance_away_from_the_end_without_editing() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let candidate = conversion_candidate(&converter, &tables, "ひだり", "←");
    for (input, cursor_delta) in [("みぎ", 0), ("ひだ", -1), ("ひだり", 0), ("", 0)] {
        let mut session = ConversionSession::new();
        session.insert_str(input, InputStyle::Direct, &tables);
        session.move_cursor(cursor_delta);
        let original = session.composing().clone();
        assert!(!session.accept_prediction_candidate(&candidate, &tables));
        assert_eq!(session.composing().surface(), original.surface());
        assert_eq!(session.composing().cursor(), original.cursor());
        assert_eq!(session.composing().input(), original.input());
    }
}

#[test]
fn keeps_accepted_zenz_spelling_and_releases_it_after_editing_or_committing() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let accepted = conversion_candidate(&converter, &tables, "ひだり", "←");
    let options = RequestOptions {
        japanese_prediction: PredictionMode::Manual,
        ..Default::default()
    };
    let mut session = ConversionSession::new();
    session.insert_str("ひだ", InputStyle::Direct, &tables);
    assert!(session.accept_prediction_candidate(&accepted, &tables));
    for continuation in ["", "にまがる"] {
        session.insert_str(continuation, InputStyle::Direct, &tables);
        session
            .request_zenz_draft(&converter, &tables, 3, &PrefixConstraint::default())
            .unwrap();
        let result = session
            .finalize_zenz_request(&converter, &tables, options.clone())
            .unwrap();
        assert!(!result.main_results.is_empty());
        assert!(
            result
                .main_results
                .iter()
                .all(|candidate| candidate.text.starts_with('←'))
        );
        assert!(
            result
                .prediction_results
                .iter()
                .all(|candidate| candidate.text.starts_with('←'))
        );
    }
    session
        .select_candidate_value(accepted.clone(), &tables)
        .unwrap();
    assert!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .is_empty()
    );
    session.reset();
    session.insert_str("ひだ", InputStyle::Direct, &tables);
    assert!(session.accept_prediction_candidate(&accepted, &tables));
    session.delete_backward(1, &tables);
    session.insert_str("り", InputStyle::Direct, &tables);
    assert!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .is_empty()
    );
    // 表記の固定は本家と同じくZenzaiの変換にだけ適用する。
    let result = session.request(&converter, &tables, options).unwrap();
    assert!(
        result
            .main_results
            .iter()
            .any(|candidate| candidate.text == "左")
    );
}

#[test]
fn retains_an_expanded_template_prefix_in_following_predictions() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let template = "<random type=\"int\" value=\"7,7\">";
    let entry = |word: String, ruby: &str| DictionaryEntry {
        word,
        ruby: ruby.into(),
        left_id: 1288,
        right_id: 1288,
        meaning_id: 501,
        base_value: 0.0,
        adjustment: 0.0,
        metadata: DictionaryMetadata::default(),
    };
    let mut session = ConversionSession::new();
    session.import_dynamic_user_dictionary(
        vec![
            entry(template.into(), "キョウ"),
            entry(format!("{template}は晴れ"), "キョウハハレ"),
        ],
        vec![],
    );
    session.insert_str("きょ", InputStyle::Direct, &tables);
    let options = RequestOptions {
        japanese_prediction: PredictionMode::Manual,
        ..Default::default()
    };
    let initial = session
        .request(&converter, &tables, options.clone())
        .unwrap();
    let accepted = initial
        .prediction_results
        .into_iter()
        .find(|candidate| candidate.text == "7")
        .unwrap();
    assert!(session.accept_prediction_candidate(&accepted, &tables));
    session
        .request_zenz_draft(&converter, &tables, 3, &PrefixConstraint::default())
        .unwrap();
    let result = session
        .finalize_zenz_request(&converter, &tables, options)
        .unwrap();
    assert_eq!(result.main_results[0].text, "7");
    assert!(!result.main_results[0].is_learning_target);
    assert!(
        result
            .prediction_results
            .iter()
            .any(|candidate| candidate.text == "7は晴れ")
    );
    assert!(
        result
            .prediction_results
            .iter()
            .all(|candidate| candidate.text.starts_with('7'))
    );
}

#[test]
fn temporarily_shorter_segment_preserves_accepted_spelling_until_commit() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let accepted = conversion_candidate(&converter, &tables, "ひだり", "←");
    let mut session = ConversionSession::new();
    session.insert_str("ひだ", InputStyle::Direct, &tables);
    assert!(session.accept_prediction_candidate(&accepted, &tables));
    session.insert_str("にまがる", InputStyle::Direct, &tables);
    session.move_cursor(-5);
    session.begin_segment_request(&tables);
    assert_eq!(session.composing().surface(), "ひだ");
    session
        .request_zenz_draft(&converter, &tables, 3, &PrefixConstraint::default())
        .unwrap();
    session.end_segment_request();
    session.move_cursor(5);
    session
        .request_zenz_draft(&converter, &tables, 3, &PrefixConstraint::default())
        .unwrap();
    let result = session
        .finalize_zenz_request(&converter, &tables, RequestOptions::default())
        .unwrap();
    assert!(!result.main_results.is_empty());
    assert!(
        result
            .main_results
            .iter()
            .all(|candidate| candidate.text.starts_with('←'))
    );
    assert_eq!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .bytes,
        "←".as_bytes()
    );

    // Actually committing a shorter, ordinary segment removes the accepted reading.
    session.move_cursor(-5);
    session.begin_segment_request(&tables);
    let partial = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap()
        .main_results[0]
        .clone();
    session.end_segment_request();
    session.select_candidate_value(partial, &tables).unwrap();
    assert!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .is_empty()
    );
}
