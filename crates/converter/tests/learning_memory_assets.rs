use std::path::{Path, PathBuf};
use std::{fs, process};

use beankey_converter::{
    Candidate, ConversionSession, DictionaryStore, InputStyle, InputTableRegistry, LearningMemory,
    LearningMode, LearningScope, LearningTask, NormalConverter, PersonalizationComparison,
    PredictionMode, PrefixConstraint, RequestOptions, TypoCorrectionMode, ZenzEvaluator,
    ZenzInferenceError, ZenzLanguageModel, ZenzVersionConfig,
};

fn dictionary_root() -> PathBuf {
    PathBuf::from(std::env::var_os("BEANKEY_TEST_DICTIONARY").expect("Nix test dictionary"))
}

fn options() -> RequestOptions {
    RequestOptions {
        japanese_prediction: PredictionMode::Disabled,
        foreign_prediction: PredictionMode::Disabled,
        ..Default::default()
    }
}

fn training_candidate(converter: &NormalConverter<'_>, tables: &InputTableRegistry) -> Candidate {
    let mut training = ConversionSession::new();
    training.insert_str("がくせい", InputStyle::Direct, tables);
    let mut candidate = training
        .request_candidates(converter, tables, 10)
        .unwrap()
        .iter()
        .find(|candidate| candidate.text == "学生" && candidate.entries.len() == 1)
        .unwrap()
        .clone();
    candidate.text = "学習試験語".into();
    candidate.entries[0].word.clone_from(&candidate.text);
    candidate
}

fn open(directory: &Path, dictionary: &DictionaryStore, mode: LearningMode) -> LearningMemory {
    LearningMemory::open(directory, mode, 128, dictionary.character_ids().clone()).unwrap()
}

fn compose(session: &mut ConversionSession, tables: &InputTableRegistry, reading: &str) {
    session.reset();
    session.insert_str(reading, InputStyle::Direct, tables);
}

#[test]
fn recall_is_current_persistent_and_separate_from_baseline_and_zenz_drafts() {
    let directory = TemporaryDirectory::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = open(&directory.0, &dictionary, LearningMode::InputAndOutput);
    let candidate = training_candidate(&converter, &tables);
    let mut baseline = ConversionSession::new();
    compose(&mut baseline, &tables, "がくせい");
    let baseline_order = baseline
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    memory.learn(&candidate).unwrap();

    for persisted in [false, true] {
        if persisted {
            memory.commit().unwrap();
        }
        compose(&mut session, &tables, "がくせい");
        let result = session.request(&converter, &tables, options()).unwrap();
        assert_eq!(result.main_results[..baseline_order.len()], baseline_order);
        let recalled = result
            .main_results
            .iter()
            .find(|value| value.text == candidate.text)
            .unwrap();
        assert_eq!(
            recalled.entries[0].base_value,
            candidate.entries[0].base_value
        );
        assert_eq!(
            recalled.entries[0].adjustment,
            candidate.entries[0].adjustment
        );
        assert!(
            session
                .request_zenz_draft(
                    &converter,
                    &tables,
                    10,
                    &PrefixConstraint::new(candidate.text.as_bytes().to_vec())
                )
                .unwrap()
                .iter()
                .all(|value| value.text != candidate.text)
        );
        // Exact readings are conversions, never transient-only predictions.
        assert!(
            session
                .request_predictions(&converter, &tables, 10)
                .unwrap()
                .iter()
                .all(|value| value.text != candidate.text)
        );
        compose(&mut session, &tables, "がく");
        let predicted = session
            .request_predictions(&converter, &tables, 10)
            .unwrap();
        let recalled = predicted
            .iter()
            .find(|value| value.text == candidate.text)
            .unwrap();
        assert_eq!(session.consumed_surface_count(recalled, &tables), 2);
        compose(&mut session, &tables, "かくせい");
        let result = session
            .request(
                &converter,
                &tables,
                RequestOptions {
                    typo_correction: TypoCorrectionMode::Enabled,
                    ..options()
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
    let recovered = open(&directory.0, &dictionary, LearningMode::OnlyOutput);
    assert!(
        recovered
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.word == candidate.text)
    );
    compose(&mut session, &tables, "がく");
    session
        .request_predictions(&converter, &tables, 10)
        .unwrap();
    session.forget_learning(&candidate).unwrap();
    assert!(
        session
            .request_predictions(&converter, &tables, 10)
            .unwrap()
            .iter()
            .all(|value| value.text != candidate.text)
    );
    memory.learn(&candidate).unwrap();
    session.reset_learning().unwrap();
    assert!(memory.entries().unwrap().is_empty());
    assert!(
        open(&directory.0, &dictionary, LearningMode::OnlyOutput)
            .entries()
            .unwrap()
            .is_empty()
    );
}

struct NoInference;
impl ZenzLanguageModel for NoInference {
    fn vocabulary_size(&self) -> usize {
        1
    }
    fn eos_token(&self) -> i32 {
        0
    }
    fn tokenize(&mut self, _: &str, _: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        panic!("no evidence or a model-independent win must not tokenize");
    }
    fn next_logits(&mut self, _: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        panic!("no evidence or a model-independent win must not decode");
    }
    fn token_to_piece(&mut self, _: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        unreachable!()
    }
}

#[test]
fn displayed_corrections_not_passive_accepts_promote_even_an_extremely_unlikely_path() {
    let directory = TemporaryDirectory::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = open(&directory.0, &dictionary, LearningMode::InputAndOutput);
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    let mut rare = training_candidate(&converter, &tables);
    rare.entries[0].base_value = -10_000.0;
    rare.value = 999_999.0; // A ranking carrier is not the original path's prior.
    memory.learn(&rare).unwrap();
    let scope = LearningScope::new("ガクセイ", LearningTask::Conversion, "会議で")
        .with_action("span=4;fixed=");
    compose(&mut session, &tables, "がくせい");
    let mut shown = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    let original = shown[0].clone();
    let mut model = NoInference;
    let mut evaluator = ZenzEvaluator::default();
    session.personalize_candidates(
        &converter,
        &tables,
        &mut shown,
        "会議で",
        Some(PersonalizationComparison {
            model: &mut model,
            evaluator: &mut evaluator,
            version: &ZenzVersionConfig::default(),
            token_budget: 192,
        }),
    );
    assert_eq!(shown[0].text, original.text);
    for _ in 0..4 {
        compose(&mut session, &tables, "がくせい");
        let shown = session
            .request(&converter, &tables, options())
            .unwrap()
            .main_results;
        session.observe_presentation(&shown, 0, &tables, "会議で");
        session
            .select_candidate_value(shown[0].clone(), &tables)
            .unwrap();
    }
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
    for support in 0..3 {
        compose(&mut session, &tables, "がくせい");
        let mut shown = session
            .request(&converter, &tables, options())
            .unwrap()
            .main_results;
        session.personalize_candidates(&converter, &tables, &mut shown, "会議で", None);
        assert_eq!(shown[0].text, original.text, "support={support}");
        session.observe_presentation(&shown, 0, &tables, "会議で");
        let selected = shown
            .iter()
            .find(|value| value.text == rare.text)
            .unwrap()
            .clone();
        assert_eq!(
            session.select_candidate_value(selected, &tables).unwrap(),
            rare.text
        );
    }
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .unwrap()
            .wins_without_model()
    );
    compose(&mut session, &tables, "がくせい");
    let mut shown = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    let unranked_tail = shown
        .iter()
        .filter(|value| value.text != rare.text)
        .cloned()
        .collect::<Vec<_>>();
    session.personalize_candidates(
        &converter,
        &tables,
        &mut shown,
        "会議で",
        Some(PersonalizationComparison {
            model: &mut model,
            evaluator: &mut evaluator,
            version: &ZenzVersionConfig::default(),
            token_budget: 192,
        }),
    );
    assert_eq!(shown[0].text, rare.text);
    assert_eq!(shown[1..], unranked_tail);
    // The actual personalized B was displayed: correcting back to A cancels B's support.
    for _ in 0..3 {
        session.observe_presentation(&shown, 0, &tables, "会議で");
        session
            .select_candidate_value(original.clone(), &tables)
            .unwrap();
        compose(&mut session, &tables, "がくせい");
    }
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
    assert!(
        memory
            .challenge(&scope, &rare, &original)
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_unshown_different_task_and_different_context_choices_do_not_form_corrections() {
    let directory = TemporaryDirectory::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = open(&directory.0, &dictionary, LearningMode::InputAndOutput);
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    let rare = training_candidate(&converter, &tables);
    compose(&mut session, &tables, "がくせい");
    let shown = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    let original = shown[0].clone();
    session.observe_presentation(&shown, 0, &tables, "会議で");
    session
        .select_candidate_value(rare.clone(), &tables)
        .unwrap(); // not displayed
    let scope = LearningScope::new("ガクセイ", LearningTask::Conversion, "会議で")
        .with_action("span=4;fixed=");
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
    compose(&mut session, &tables, "がくせい");
    let shown = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    session.observe_presentation(&shown, 0, &tables, "会議で");
    session.insert_str("あ", InputStyle::Direct, &tables); // stale presentation
    session
        .select_candidate_value(rare.clone(), &tables)
        .unwrap();
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
    compose(&mut session, &tables, "がく");
    let mut shown = session
        .request_predictions(&converter, &tables, 10)
        .unwrap();
    let predicted = shown
        .iter()
        .find(|candidate| candidate.text == rare.text)
        .unwrap()
        .clone();
    // A full prediction cannot teach a correction against a shorter conversion default.
    let conversion = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results[0]
        .clone();
    shown.insert(0, conversion);
    session.observe_presentation(&shown, 0, &tables, "会議で");
    session.select_candidate_value(predicted, &tables).unwrap();
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
    compose(&mut session, &tables, "がくせい");
    let shown = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    session.observe_presentation(&shown, 0, &tables, "会議で");
    let selected = shown
        .iter()
        .find(|candidate| candidate.text == rare.text)
        .unwrap()
        .clone();
    session.select_candidate_value(selected, &tables).unwrap();
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_some()
    );
    let elsewhere = LearningScope::new("ガクセイ", LearningTask::Conversion, "別の話")
        .with_action("span=4;fixed=");
    assert!(
        memory
            .challenge(&elsewhere, &original, &rare)
            .unwrap()
            .is_none()
    );
    session.commit_learning().unwrap();
    let reopened = open(&directory.0, &dictionary, LearningMode::OnlyOutput);
    assert!(
        reopened
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_some()
    );
    session.forget_learning(&original).unwrap(); // forgetting the loser removes the pair too
    assert!(
        memory
            .challenge(&scope, &original, &rare)
            .unwrap()
            .is_none()
    );
}

#[test]
fn failed_persistence_does_not_lose_a_selected_text_or_in_memory_recall_and_can_retry() {
    let directory = TemporaryDirectory::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = open(&directory.0, &dictionary, LearningMode::InputAndOutput);
    // Block atomic publication, not an unrelated legacy filename.
    let blocked = directory.0.join("personalization-v1.json");
    fs::create_dir(&blocked).unwrap();
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    compose(&mut session, &tables, "がくせい");
    let rare = training_candidate(&converter, &tables);
    assert_eq!(
        session
            .select_candidate_value(rare.clone(), &tables)
            .unwrap(),
        rare.text
    );
    assert!(session.composing().is_empty());
    assert!(memory.commit().is_err());
    let expected = memory.entries().unwrap();
    assert!(expected.iter().any(|entry| entry.word == rare.text));
    assert_eq!(blocked.parent(), Some(directory.0.as_path()));
    fs::remove_dir(&blocked).unwrap();
    assert!(memory.commit().unwrap());
    assert!(!memory.commit().unwrap());
    let reopened = open(&directory.0, &dictionary, LearningMode::OnlyOutput);
    assert_eq!(reopened.entries().unwrap(), expected);
}

#[test]
fn a_segment_inside_an_accepted_prediction_cannot_reuse_unfixed_partial_evidence() {
    let directory = TemporaryDirectory::new();
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let memory = open(&directory.0, &dictionary, LearningMode::InputAndOutput);
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory.clone()).unwrap();
    compose(&mut session, &tables, "がくせい");
    session.move_cursor(-2);
    session.begin_segment_request(&tables);
    let choices = session
        .request(&converter, &tables, options())
        .unwrap()
        .main_results;
    let anchor = choices
        .iter()
        .find(|candidate| {
            candidate.is_learning_target && session.consumed_surface_count(candidate, &tables) == 2
        })
        .unwrap()
        .clone();
    let challenger = choices
        .iter()
        .find(|candidate| {
            candidate.is_learning_target
                && candidate.text != anchor.text
                && session.consumed_surface_count(candidate, &tables) == 2
        })
        .unwrap()
        .clone();
    session.end_segment_request();
    let scope = LearningScope::new("ガク", LearningTask::PartialConversion, "")
        .with_action("span=2;fixed=");
    for _ in 0..3 {
        memory
            .record_correction(&scope, &anchor, &challenger)
            .unwrap();
    }
    let confidence = memory
        .challenge(&scope, &anchor, &challenger)
        .unwrap()
        .unwrap()
        .confidence;

    let accepted = training_candidate(&converter, &tables);
    compose(&mut session, &tables, "がく");
    assert!(session.accept_prediction_candidate(&accepted, &tables));
    session.move_cursor(-2);
    session.begin_segment_request(&tables);
    let mut displayed = vec![anchor.clone(), challenger.clone()];
    session.personalize_candidates(
        &converter,
        &tables,
        &mut displayed,
        "",
        Some(PersonalizationComparison {
            model: &mut NoInference,
            evaluator: &mut ZenzEvaluator::default(),
            version: &ZenzVersionConfig::default(),
            token_budget: 192,
        }),
    );
    assert_eq!(displayed, vec![anchor.clone(), challenger.clone()]);
    session.observe_presentation(&displayed, 0, &tables, "");
    session.end_segment_request();
    assert_eq!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .bytes,
        accepted.text.as_bytes()
    );
    session
        .select_candidate_value(challenger.clone(), &tables)
        .unwrap();
    assert_eq!(
        memory
            .challenge(&scope, &anchor, &challenger)
            .unwrap()
            .unwrap()
            .confidence,
        confidence
    );
    assert!(
        session
            .constrain_zenz_prefix(PrefixConstraint::default())
            .is_empty()
    );
}

#[test]
#[ignore = "performance probe; run with --release --ignored --nocapture"]
fn repeated_recall_surroundings() {
    let dictionary = DictionaryStore::open(
        std::env::var_os("BEANKEY_TEST_DICTIONARY").expect("Nix test dictionary"),
    )
    .unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let directory = TemporaryDirectory::new();
    let memory = LearningMemory::open(
        &directory.0,
        LearningMode::InputAndOutput,
        128,
        dictionary.character_ids().clone(),
    )
    .unwrap();
    let mut training = ConversionSession::new();
    training.insert_str("きょう", InputStyle::Direct, &tables);
    let original = training
        .request_candidates(&converter, &tables, 10)
        .unwrap()
        .iter()
        .find(|candidate| candidate.text == "今日" && candidate.entries.len() == 1)
        .expect("pinned dictionary supplies 今日")
        .clone();
    for text in ["試験甲", "試験乙", "試験丙", "試験丁"] {
        let mut candidate = original.clone();
        candidate.text = text.into();
        candidate.entries[0].word = text.into();
        memory.learn(&candidate).unwrap();
    }
    memory.commit().unwrap();
    let mut session = ConversionSession::new();
    session.set_learning_memory(memory).unwrap();
    for (style, reading) in [
        (InputStyle::Direct, "きょうはいいてんきです"),
        (
            InputStyle::Direct,
            "きょうはいいてんきなのでがっこうまであるいていきます",
        ),
        (InputStyle::RomanToKana, "kyouhaiitenkidesu"),
        (
            InputStyle::RomanToKana,
            "kyouhaiitenkinanodegakkoumadearuiteikimasu",
        ),
        (
            InputStyle::RomanToKana,
            "watashihakyouhaiitenkinanodegakkoumadearuiteikimasu",
        ),
    ] {
        session.reset();
        session.insert_str(reading, style.clone(), &tables);
        let options = RequestOptions {
            japanese_prediction: PredictionMode::Disabled,
            foreign_prediction: PredictionMode::Disabled,
            ..Default::default()
        };
        let expected = session
            .request(&converter, &tables, options.clone())
            .unwrap();
        let started = std::time::Instant::now();
        let mut rerank_us = 0;
        for _ in 0..20 {
            let actual = session
                .request(&converter, &tables, options.clone())
                .unwrap();
            assert_eq!(actual, expected, "warm requests preserve all output fields");
            let mut candidates = session.desktop_candidates(&actual, &tables);
            let rerank_started = std::time::Instant::now();
            session.personalize_candidates(&converter, &tables, &mut candidates, "", None);
            rerank_us += rerank_started.elapsed().as_micros();
        }
        eprintln!(
            "recall_probe style={style:?} chars={} candidates={} total_us={} rerank_us={}",
            session.composing().surface_graphemes().len(),
            expected.main_results.len(),
            started.elapsed().as_micros() / 20,
            rerank_us / 20,
        );
    }
}

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(
            format!("beankey-learning-{}-{}", process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
        ))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let Some(name) = self.0.file_name().and_then(|value| value.to_str()) else {
            return;
        };
        assert!(name.starts_with("beankey-learning-"));
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        if self.0.exists() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
