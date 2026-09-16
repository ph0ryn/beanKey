use std::path::PathBuf;

use bean_key_converter::{
    ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, NormalConverter,
    RequestOptions,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn exposes_typography_only_when_the_optional_provider_is_enabled() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("beanKey", InputStyle::Direct, &tables);

    let defaults = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap();
    assert!(
        !defaults
            .main_results
            .iter()
            .any(|candidate| candidate.text == "𝐁𝐞𝐚𝐧𝐊𝐞𝐲")
    );

    let enabled = session
        .request(
            &converter,
            &tables,
            RequestOptions {
                typography: true,
                ..RequestOptions::default()
            },
        )
        .unwrap();
    assert!(enabled.main_results.iter().any(|candidate| {
        candidate.text == "𝐛𝐞𝐚𝐧𝐊𝐞𝐲"
            && candidate.value == -15.0
            && candidate.entries[0].ruby == "beanKey"
    }));
}
