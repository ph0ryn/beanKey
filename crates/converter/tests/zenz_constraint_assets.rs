use std::path::PathBuf;

use beankey_converter::{
    ComposingText, ConversionContext, ConversionSession, DictionaryEntry, DictionaryMetadata,
    DictionaryStore, FixedPrefix, InputStyle, InputTableRegistry, NormalConverter,
    PrefixConstraint,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dictionary/azookey/Dictionary")
}

#[test]
fn searches_the_lattice_with_utf8_prefix_and_eos_constraints() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut composing = ComposingText::new();
    composing.insert_str("はし", InputStyle::Direct, &tables);

    let prefix = PrefixConstraint::new("箸".as_bytes().to_vec());
    let prefixed = converter
        .convert_with_prefix_constraint(
            &composing,
            &tables,
            3,
            ConversionContext::default(),
            &prefix,
        )
        .unwrap();
    assert!(!prefixed.is_empty());
    assert!(
        prefixed
            .iter()
            .all(|candidate| candidate.text.starts_with("箸"))
    );

    let exact = PrefixConstraint {
        bytes: "箸".as_bytes().to_vec(),
        has_eos: true,
        ignore_memory_and_user_dictionary: false,
        fixed_prefix: None,
    };
    let exact = converter
        .convert_with_prefix_constraint(
            &composing,
            &tables,
            3,
            ConversionContext::default(),
            &exact,
        )
        .unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].text, "箸");
}

#[test]
fn fixed_prefix_constrains_user_dictionary_entries() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.import_dynamic_user_dictionary(
        vec![DictionaryEntry {
            word: "橋".into(),
            ruby: "ハシ".into(),
            left_id: 1285,
            right_id: 1285,
            meaning_id: 501,
            base_value: -1.0,
            adjustment: 0.0,
            metadata: DictionaryMetadata::default(),
        }],
        Vec::new(),
    );
    session.insert_str("はし", InputStyle::Direct, &tables);

    let constraint = PrefixConstraint {
        bytes: "箸".as_bytes().to_vec(),
        has_eos: false,
        ignore_memory_and_user_dictionary: false,
        fixed_prefix: Some(FixedPrefix {
            byte_count: "箸".len(),
            ruby_count: 1,
        }),
    };
    let candidates = session
        .request_zenz_draft(&converter, &tables, 10, &constraint)
        .unwrap();

    assert!(candidates.iter().any(|candidate| candidate.text == "箸"));
    assert!(!candidates.iter().any(|candidate| candidate.text == "橋"));
}
