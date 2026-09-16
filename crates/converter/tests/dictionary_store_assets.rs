use std::path::PathBuf;

use bean_key_converter::DictionaryStore;

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn searches_fixed_entries_and_costs_through_the_store() {
    let store = DictionaryStore::open(dictionary_root()).unwrap();

    let entries = store.exact_match("シカイ").unwrap();
    assert!(entries.iter().any(|entry| entry.word == "司会"));
    assert!(entries.iter().any(|entry| entry.word == "視界"));
    assert!(entries.iter().all(|entry| entry.ruby == "シカイ"));

    let prefixes = store.matches_from_start("シカイシャ", 20).unwrap();
    assert!(prefixes.iter().any(|item| item.surface_end == 4));
    assert!(prefixes.iter().any(|item| item.surface_end > 4));

    assert!((store.connection_cost(1285, 0).unwrap() - -18.43).abs() < 0.0001);
    assert!((store.connection_cost(1318, 10).unwrap() - -7.3947).abs() < 0.0001);
    assert_eq!(store.meaning_cost(500, 20), Some(0.0));
}
