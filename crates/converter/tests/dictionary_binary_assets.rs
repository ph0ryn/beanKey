use std::fs;
use std::path::PathBuf;

use bean_key_converter::{MeaningMatrix, parse_connection_cost_line};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn reads_fixed_meaning_and_connection_cost_assets() {
    let root = dictionary_root();
    let matrix = MeaningMatrix::parse(&fs::read(root.join("mm.binary")).unwrap()).unwrap();
    assert!((matrix.get(0, 0).unwrap() - -3.9985).abs() < 0.0001);
    assert_eq!(matrix.get(500, 12), Some(0.0));

    let line = parse_connection_cost_line(&fs::read(root.join("cb/1285.binary")).unwrap()).unwrap();
    assert!((line[0] - -18.43).abs() < 0.0001);
    assert!((line[1] - -10.9076).abs() < 0.0001);
    assert!((line[2] - -8.6864).abs() < 0.0001);
}
