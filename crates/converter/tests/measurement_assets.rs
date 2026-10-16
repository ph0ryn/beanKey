use std::path::PathBuf;

use beankey_converter::{
    ComposingCount, ConversionSession, DictionaryStore, InputStyle, InputTableRegistry,
    MeasurementStatus, NormalConverter, RequestOptions,
};

#[test]
fn presentation_comparison_uses_actual_default_and_same_consumed_scope_without_memory() {
    let root = PathBuf::from(std::env::var_os("BEANKEY_TEST_DICTIONARY").unwrap());
    let dictionary = DictionaryStore::open(root).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("shikai", InputStyle::RomanToKana, &tables);
    let candidates = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap()
        .main_results;
    let baseline = candidates
        .iter()
        .find(|candidate| candidate.text == "視界")
        .unwrap();
    let preferred = candidates
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap();
    let snapshot = session.measure_presentation(
        Some(baseline),
        preferred,
        &candidates,
        &tables,
        "機密の文脈",
    );
    let helped = session.measure_selection(&snapshot, preferred, &tables);
    assert_eq!(helped.status, MeasurementStatus::Comparable);
    assert_eq!(helped.default_matches_selection, Some(true));
    assert_eq!(helped.baseline_matches_selection, Some(false));
    let harmed = session.measure_selection(&snapshot, baseline, &tables);
    assert_eq!(harmed.default_matches_selection, Some(false));
    assert_eq!(harmed.baseline_matches_selection, Some(true));

    // Surface/Input count representations may differ while the consumed action is identical.
    let mut equivalent = preferred.clone();
    equivalent.composing_count = ComposingCount::Surface(3);
    assert_eq!(
        session
            .measure_selection(&snapshot, &equivalent, &tables)
            .status,
        MeasurementStatus::Comparable
    );

    // A different consumed action must never become a default correction or a paired vote.
    let mut partial = preferred.clone();
    partial.composing_count = ComposingCount::Surface(1);
    let choices = vec![baseline.clone(), preferred.clone(), partial.clone()];
    let snapshot =
        session.measure_presentation(Some(baseline), preferred, &choices, &tables, "機密の文脈");
    let mismatch = session.measure_selection(&snapshot, &partial, &tables);
    assert_eq!(mismatch.status, MeasurementStatus::DifferentScope);
    assert_eq!(mismatch.default_matches_selection, None);
    assert_eq!(mismatch.baseline_matches_selection, None);

    session.insert_str("a", InputStyle::RomanToKana, &tables);
    assert_eq!(
        session
            .measure_selection(&snapshot, preferred, &tables)
            .status,
        MeasurementStatus::StalePresentation
    );
}
