use std::path::PathBuf;
use std::sync::Arc;

use beankey_converter::{
    ConversionSession, DictionaryStore, ForeignCompletionProvider, ForeignLanguage,
    HunspellCompleter, InputStyle, InputTableRegistry, KeyboardLanguage, NormalConverter,
    PredictionMode, RequestOptions,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dictionary/azookeyKanaKanji/Dictionary")
}

#[test]
fn preserves_case_and_filters_suggestions_without_ngram_expansion() {
    let provider = HunspellCompleter::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dictionary/hunspellEnglish/en_US"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dictionary/hunspellGreek/el_GR"),
    )
    .unwrap();
    assert_eq!(
        provider.completions(ForeignLanguage::EnglishUs, "Hel"),
        ["Hell", "Held", "Helm", "Help"]
    );
    assert_eq!(
        provider.completions(ForeignLanguage::EnglishUs, "comput"),
        ["compute"]
    );
    let greek = provider.completions(ForeignLanguage::Greek, "Καλ");
    assert!(greek.iter().any(|word| word == "Καλά"));
    assert!(
        greek
            .iter()
            .all(|word| word.to_lowercase().starts_with("καλ"))
    );
    assert_eq!(
        provider.completions(ForeignLanguage::EnglishUs, "hel"),
        ["hell", "held", "helm", "help"]
    );
}

#[test]
fn completes_english_and_greek_with_the_pinned_hunspell_dictionaries() {
    let provider = Arc::new(
        HunspellCompleter::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/dictionary/hunspellEnglish/en_US"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dictionary/hunspellGreek/el_GR"),
        )
        .unwrap(),
    );
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();

    let mut english = ConversionSession::new();
    english.set_foreign_completion_provider(provider.clone());
    english.insert_str("hel", InputStyle::Direct, &tables);
    let english_result = english
        .request(
            &converter,
            &tables,
            RequestOptions {
                foreign_prediction: PredictionMode::Manual,
                keyboard_language: KeyboardLanguage::EnglishUs,
                ..RequestOptions::default()
            },
        )
        .unwrap();
    assert_eq!(
        english_result
            .english_prediction_results
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>(),
        ["hel", "hell", "held", "helm", "help"]
    );

    let mut greek = ConversionSession::new();
    greek.set_foreign_completion_provider(provider);
    greek.insert_str("καλ", InputStyle::Direct, &tables);
    let greek_result = greek
        .request(
            &converter,
            &tables,
            RequestOptions {
                foreign_prediction: PredictionMode::Manual,
                keyboard_language: KeyboardLanguage::Greek,
                ..RequestOptions::default()
            },
        )
        .unwrap();
    assert_eq!(greek_result.english_prediction_results[0].text, "καλ");
    assert!(
        greek_result
            .english_prediction_results
            .iter()
            .any(|candidate| candidate.text == "καλά")
    );
}
