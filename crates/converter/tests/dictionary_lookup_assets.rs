use std::fs;
use std::path::PathBuf;

use beankey_converter::{CharacterIdMap, Louds, escaped_identifier, parse_entry_shard};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn looks_up_entries_in_the_fixed_dictionary() {
    let root = dictionary_root();
    let character_ids =
        CharacterIdMap::parse(&fs::read_to_string(root.join("louds/charID.chid")).unwrap())
            .unwrap();
    let identifier = escaped_identifier("シ");
    let louds = Louds::parse(
        &fs::read(root.join(format!("louds/{identifier}.louds"))).unwrap(),
        &fs::read(root.join(format!("louds/{identifier}.loudschars2"))).unwrap(),
    )
    .unwrap();
    let node = louds
        .search(&character_ids.encode("シカイ").unwrap())
        .unwrap();
    let shard = node >> 11;
    let local = node & 2047;
    let entries = parse_entry_shard(
        &fs::read(root.join(format!("louds/{identifier}{shard}.loudstxt3"))).unwrap(),
        [local],
    )
    .unwrap();

    let words: Vec<_> = entries.iter().map(|entry| entry.word.as_str()).collect();
    assert!(words.contains(&"司会"));
    assert!(words.contains(&"視界"));
    assert!(words.contains(&"死界"));
    assert!(entries.iter().all(|entry| entry.ruby == "シカイ"));
}
