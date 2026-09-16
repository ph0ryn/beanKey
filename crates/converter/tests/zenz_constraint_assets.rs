use std::path::PathBuf;

use bean_key_converter::{
    ComposingText, ConversionContext, DictionaryStore, InputStyle, InputTableRegistry,
    NormalConverter, PrefixConstraint,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
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
