use std::path::PathBuf;

use beankey_converter::{
    ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
    PostCompositionPredictor, TextReplacer,
};

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

#[test]
fn reads_fixed_zero_hint_assets_and_predicts_after_commit() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let predictor = PostCompositionPredictor::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("きょう", InputStyle::Direct, &tables);

    let candidates = session.request_candidates(&converter, &tables, 10).unwrap();
    let index = candidates
        .iter()
        .position(|candidate| candidate.text == "今日")
        .unwrap();
    let selected = candidates[index].clone();
    session.select_candidate(index, &tables).unwrap();
    let predictions = session
        .request_post_composition_predictions(&predictor)
        .unwrap();

    assert!(!predictions.is_empty());
    assert!(predictions.len() <= 10);
    assert!(predictions.iter().any(|prediction| {
        let joined = prediction.join(&selected);
        joined.text.starts_with("今日") && joined.text.len() > "今日".len()
    }));
}

#[test]
fn places_up_to_three_base_emojis_before_other_post_composition_predictions() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let replacer = TextReplacer::open(emoji_dictionary_path()).unwrap();
    let predictor = PostCompositionPredictor::with_text_replacer(&dictionary, &replacer);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("えがお", InputStyle::Direct, &tables);

    let candidates = session.request_candidates(&converter, &tables, 10).unwrap();
    let index = candidates
        .iter()
        .position(|candidate| candidate.text == "笑顔")
        .unwrap();
    session.select_candidate(index, &tables).unwrap();
    let predictions = session
        .request_post_composition_predictions(&predictor)
        .unwrap();

    assert_eq!(predictions.len(), 10);
    assert_eq!(
        predictions[..3]
            .iter()
            .filter(|item| item.value == -3.0)
            .count(),
        3
    );
    assert!(predictions[..3].iter().all(|item| {
        matches!(
            &item.kind,
            beankey_converter::PostPredictionKind::Additional { entries }
                if entries.len() == 1 && entries[0].ruby == "エモジ"
        )
    }));
}
