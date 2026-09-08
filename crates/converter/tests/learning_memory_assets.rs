use std::path::PathBuf;
use std::{fs, process};

use bean_key_converter::{
    ConversionSession, DictionaryMetadata, DictionaryStore, InputStyle, InputTableRegistry,
    LearningMemory, LearningMode, NormalConverter, PrefixConstraint, RequestOptions,
    TypoCorrectionMode,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("data/azooKey_dictionary_storage/Dictionary")
}

#[test]
fn existing_sessions_search_current_learning_for_conversion_prediction_and_typos() {
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
    let mut training = ConversionSession::new();
    training.insert_str("がくせい", InputStyle::Direct, &tables);
    let mut candidate = training
        .request_candidates(&converter, &tables, 10)
        .unwrap()
        .iter()
        .find(|value| value.text == "学生" && value.entries.len() == 1)
        .unwrap()
        .clone();
    candidate.text = "学習試験語".into();
    candidate.entries[0].word.clone_from(&candidate.text);
    memory.learn(&candidate).unwrap();

    // The same session must see temporary learning and the published LOUDS.
    for persisted in [false, true] {
        if persisted {
            session.reset();
            session.insert_str("がくせい", InputStyle::Direct, &tables);
            assert!(
                session
                    .request_predictions(&converter, &tables, 10)
                    .unwrap()
                    .iter()
                    .any(|value| value.text == candidate.text)
            );
            assert!(session.commit_learning().unwrap());
            assert!(
                session
                    .request_predictions(&converter, &tables, 10)
                    .unwrap()
                    .iter()
                    .all(|value| value.text != candidate.text)
            );
        }
        session.reset();
        session.insert_str("がくせい", InputStyle::Direct, &tables);
        assert!(
            session
                .request_candidates(&converter, &tables, 10)
                .unwrap()
                .iter()
                .any(|value| value.text == candidate.text)
        );
        assert!(
            session
                .request_zenz_draft(
                    &converter,
                    &tables,
                    10,
                    &PrefixConstraint::new(candidate.text.as_bytes().to_vec()),
                )
                .unwrap()
                .iter()
                .any(|value| value.text == candidate.text)
        );
        session.reset();
        session.insert_str("がく", InputStyle::Direct, &tables);
        assert!(
            session
                .request_predictions(&converter, &tables, 10)
                .unwrap()
                .iter()
                .any(|value| value.text == candidate.text)
        );
        session.reset();
        session.insert_str("かくせい", InputStyle::Direct, &tables);
        let result = session
            .request(
                &converter,
                &tables,
                RequestOptions {
                    n_best: 10,
                    typo_correction: TypoCorrectionMode::Enabled,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            result
                .main_results
                .iter()
                .any(|value| value.text == candidate.text && value.is_typo_correction)
        );
    }
    session.reset();
    session.insert_str("がく", InputStyle::Direct, &tables);
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .any(|value| value.text == candidate.text)
    );
    memory.learn(&candidate).unwrap();
    assert!(session.commit_learning().unwrap());
    let updated = session
        .request_predictions(&converter, &tables, 10)
        .unwrap();
    let updated_value = updated
        .iter()
        .find(|value| value.text == candidate.text)
        .unwrap()
        .value;
    session.reset();
    session.insert_str("がく", InputStyle::Direct, &tables);
    let fresh = session
        .request_predictions(&converter, &tables, 10)
        .unwrap();
    let fresh_value = fresh
        .iter()
        .find(|value| value.text == candidate.text)
        .unwrap()
        .value;
    assert_eq!(updated_value, fresh_value);
    session.forget_learning(&candidate).unwrap();
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .all(|value| value.text != candidate.text)
    );
    session.reset();
    session.insert_str("がくせい", InputStyle::Direct, &tables);
    assert!(
        session
            .request_candidates(&converter, &tables, 10)
            .unwrap()
            .iter()
            .all(|value| value.text != candidate.text)
    );
    memory.learn(&candidate).unwrap();
    session.reset();
    session.insert_str("がく", InputStyle::Direct, &tables);
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .any(|value| value.text == candidate.text)
    );
    session.reset_learning().unwrap();
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .all(|value| value.text != candidate.text)
    );
    fs::remove_dir_all(directory).unwrap();
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
