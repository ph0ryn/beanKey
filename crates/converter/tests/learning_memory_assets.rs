use std::path::PathBuf;
use std::{fs, process};

use bean_key_converter::{
    ConversionSession, DictionaryMetadata, DictionaryStore, InputStyle, InputTableRegistry,
    LearningMemory, LearningMode, NormalConverter,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("data/azooKey_dictionary_storage/Dictionary")
}

fn temporary_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "bean-key-learning-{}-{}",
        process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn retries_failed_learning_saves_without_losing_or_double_counting_words() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    session.insert_str("しかい", InputStyle::Direct, &tables);
    let candidate = session
        .request_candidates(&converter, &tables, 10)
        .unwrap()
        .iter()
        .find(|candidate| candidate.text == "司会" && candidate.entries.len() == 1)
        .unwrap()
        .clone();

    // Exercise failure both before and after the recovery marker is published.
    for blocked_file in ["memory.louds.2", "memory.louds"] {
        let directory = temporary_directory();
        let memory = LearningMemory::open(
            &directory,
            LearningMode::InputAndOutput,
            128,
            dictionary.character_ids().clone(),
        )
        .unwrap();
        memory.learn(&candidate).unwrap();
        let expected = memory.entries().unwrap();
        fs::create_dir(directory.join(blocked_file)).unwrap();
        assert!(memory.commit().is_err());
        assert!(memory.commit().is_err());
        assert_eq!(memory.entries().unwrap(), expected);
        fs::remove_dir(directory.join(blocked_file)).unwrap();
        assert!(memory.commit().unwrap());
        assert!(!memory.commit().unwrap());
        let reopened = LearningMemory::open(
            &directory,
            LearningMode::OnlyOutput,
            128,
            dictionary.character_ids().clone(),
        )
        .unwrap();
        let actual = reopened.entries().unwrap();
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual[0].word, expected[0].word);
        assert_eq!(actual[0].value(), expected[0].value());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn learns_persists_recovers_forgets_and_resets_selected_candidates() {
    let directory = temporary_directory();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = LearningMemory::open(
        &directory,
        LearningMode::InputAndOutput,
        128,
        dictionary.character_ids().clone(),
    )
    .unwrap();
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    session.insert_str("しかい", InputStyle::Direct, &tables);
    let candidates = session.request_candidates(&converter, &tables, 10).unwrap();
    let index = candidates
        .iter()
        .position(|candidate| candidate.text == "司会" && candidate.entries.len() == 1)
        .unwrap();
    let selected = candidates[index].clone();
    session.select_candidate(index, &tables).unwrap();
    assert!(session.commit_learning().unwrap());

    fs::remove_file(directory.join("memory.louds")).unwrap();
    fs::write(directory.join(".pause"), []).unwrap();
    let recovered = LearningMemory::open(
        &directory,
        LearningMode::OnlyOutput,
        128,
        dictionary.character_ids().clone(),
    )
    .unwrap();
    let mut next_session = ConversionSession::new();
    next_session.set_learning_memory(recovered).unwrap();
    next_session.insert_str("しかい", InputStyle::Direct, &tables);
    let learned = next_session
        .request_candidates(&converter, &tables, 10)
        .unwrap()
        .iter()
        .find(|candidate| candidate.text == "司会")
        .unwrap();
    assert!(
        learned
            .entries
            .iter()
            .any(|entry| entry.metadata.contains(DictionaryMetadata::LEARNED))
    );

    next_session.set_learning_memory(memory.clone()).unwrap();
    next_session.forget_learning(&selected).unwrap();
    assert!(
        !memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == "司会")
    );
    next_session.reset_learning().unwrap();
    assert!(memory.entries().unwrap().is_empty());

    fs::remove_dir_all(directory).unwrap();
}
