use std::path::PathBuf;

use beankey_converter::TextReplacer;

fn emoji_dictionary_path() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_EMOJI_DICTIONARY")
            .expect("BEANKEY_TEST_EMOJI_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn searches_and_replaces_using_the_fixed_emoji_asset() {
    let replacer = TextReplacer::open(emoji_dictionary_path()).unwrap();

    assert!(
        replacer
            .search("しゃち", false)
            .iter()
            .any(|item| item.text.starts_with('🫍'))
    );

    let smiles = replacer.search("笑顔", true);
    assert!(smiles.len() > 3);
    assert!(smiles.iter().all(|item| item.query == "笑顔"));

    let thumbs_up = replacer.search("イイネ", false);
    let base = thumbs_up
        .iter()
        .find(|item| item.text.starts_with('👍'))
        .unwrap();
    let replacements = replacer.replacements("", &base.text, "");
    assert!(replacements.iter().any(|item| item.text() != base.text));
    assert!(replacements.iter().all(|item| item.base == base.text));
}
