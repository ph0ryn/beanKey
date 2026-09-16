use std::path::{Path, PathBuf};
use std::{fs, time::SystemTime};

use bean_key_converter::{
    ConversionSession, DictionaryEntry, DictionaryMetadata, DictionaryStore, InputStyle,
    InputTableRegistry, NormalConverter, PredictionMode, RequestOptions,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

fn user_dictionary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("bean-key-user-dictionary-{nonce}"))
}

fn entry_block(word: &str, ruby: &str) -> Vec<u8> {
    let mut binary = Vec::new();
    binary.extend(1_u16.to_le_bytes());
    binary.extend(1288_u16.to_le_bytes());
    binary.extend(1288_u16.to_le_bytes());
    binary.extend(501_u16.to_le_bytes());
    binary.extend((-1.0_f32).to_le_bytes());
    binary.extend(ruby.as_bytes());
    binary.push(b'\t');
    binary.extend(word.as_bytes());
    binary.push(b'\t');
    binary
}

fn user_shard(word: &str, ruby: &str) -> Vec<u8> {
    let entry = entry_block(word, ruby);
    let mut binary = Vec::new();
    binary.extend(3_u16.to_le_bytes());
    binary.extend(14_u32.to_le_bytes());
    binary.extend(16_u32.to_le_bytes());
    binary.extend(18_u32.to_le_bytes());
    binary.extend(0_u16.to_le_bytes());
    binary.extend(0_u16.to_le_bytes());
    binary.extend(entry);
    binary
}

fn write_user_dictionary(root: &Path, identifier: &str, word: &str, ruby: &str) {
    let louds = !((1_u64 << 62) | (1_u64 << 60) | (1_u64 << 59));
    fs::write(
        root.join(format!("{identifier}.louds")),
        louds.to_le_bytes(),
    )
    .unwrap();
    fs::write(root.join(format!("{identifier}.loudschars2")), [0_u8; 3]).unwrap();
    fs::write(
        root.join(format!("{identifier}0.loudstxt3")),
        user_shard(word, ruby),
    )
    .unwrap();
}

fn user_entry(word: &str, ruby: &str) -> DictionaryEntry {
    DictionaryEntry {
        word: word.into(),
        ruby: ruby.into(),
        left_id: 1288,
        right_id: 1288,
        meaning_id: 501,
        base_value: -1.0,
        adjustment: 0.0,
        metadata: DictionaryMetadata::default(),
    }
}

#[test]
fn uses_dynamic_entries_for_conversion_prediction_and_shortcuts() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.import_dynamic_user_dictionary(
        vec![user_entry("Codex", "コーデックス")],
        vec![user_entry("openai.com", "オープンエーアイ")],
    );

    session.insert_str("こーでっくす", InputStyle::Direct, &tables);
    let result = session
        .request(
            &converter,
            &tables,
            RequestOptions {
                japanese_prediction: PredictionMode::Disabled,
                ..RequestOptions::default()
            },
        )
        .unwrap();
    let custom = result
        .main_results
        .iter()
        .find(|candidate| candidate.text == "Codex")
        .unwrap();
    assert!(
        custom.entries[0]
            .metadata
            .contains(DictionaryMetadata::USER_DICTIONARY)
    );

    session.reset();
    session.insert_str("こー", InputStyle::Direct, &tables);
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .any(|candidate| candidate.text == "Codex")
    );

    session.reset();
    session.insert_str("おーぷんえーあい", InputStyle::Direct, &tables);
    assert!(
        session
            .request(&converter, &tables, RequestOptions::default())
            .unwrap()
            .main_results
            .iter()
            .any(|candidate| candidate.text == "openai.com")
    );
}

#[test]
fn expands_dynamic_dictionary_templates_before_exposing_candidates() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.import_dynamic_user_dictionary(
        vec![user_entry(
            "<random type=\"int\" value=\"7,7\">",
            "テンプレート",
        )],
        Vec::new(),
    );

    session.insert_str("てんぷれーと", InputStyle::Direct, &tables);
    let result = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap();
    let candidate = result
        .main_results
        .iter()
        .find(|candidate| candidate.text == "7")
        .expect("expanded template candidate");
    assert!(!candidate.is_learning_target);
    assert!(
        !result
            .main_results
            .iter()
            .any(|candidate| candidate.text.starts_with("<random"))
    );
}

#[test]
fn loads_generated_user_dictionary_and_shortcut_files() {
    let user_root = user_dictionary_root();
    fs::create_dir(&user_root).unwrap();
    write_user_dictionary(&user_root, "user", "beanKey", "ビーンズキー");
    write_user_dictionary(
        &user_root,
        "user_shortcuts",
        "https://example.invalid",
        "ホームページ",
    );

    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.update_user_dictionary_path(&user_root).unwrap();

    session.insert_str("びーんずきー", InputStyle::Direct, &tables);
    let result = session
        .request(&converter, &tables, RequestOptions::default())
        .unwrap();
    let candidate = result
        .main_results
        .iter()
        .find(|candidate| candidate.text == "beanKey")
        .expect("file user dictionary candidate");
    assert!(
        candidate.entries[0]
            .metadata
            .contains(DictionaryMetadata::USER_DICTIONARY)
    );

    session.reset();
    session.insert_str("ほーむぺーじ", InputStyle::Direct, &tables);
    assert!(
        session
            .request(&converter, &tables, RequestOptions::default())
            .unwrap()
            .main_results
            .iter()
            .any(|candidate| candidate.text == "https://example.invalid")
    );
    fs::remove_dir_all(user_root).unwrap();
}
