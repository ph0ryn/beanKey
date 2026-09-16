use std::path::PathBuf;

use bean_key_converter::{
    ComposingCount, ConversionSession, DictionaryStore, InputStyle, InputTableRegistry,
    NormalConverter, ZenzInferenceError, ZenzLanguageModel, ZenzVersionConfig,
};

struct PredictiveModel {
    evaluations: usize,
}

impl ZenzLanguageModel for PredictiveModel {
    fn vocabulary_size(&self) -> usize {
        6
    }

    fn eos_token(&self) -> i32 {
        2
    }

    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        Ok(if add_special {
            vec![1]
        } else {
            match text {
                "カ" => vec![3],
                "ナ" => vec![4],
                _ => Vec::new(),
            }
        })
    }

    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        Ok(match token {
            3 => "カ".as_bytes().to_vec(),
            4 => "ナ".as_bytes().to_vec(),
            5 => "。".as_bytes().to_vec(),
            _ => Vec::new(),
        })
    }

    fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        self.evaluations += 1;
        let selected = if !tokens.contains(&3) {
            3
        } else if !tokens.contains(&4) {
            4
        } else {
            5
        };
        let mut logits = vec![-10.0; 6];
        logits[selected] = 10.0;
        Ok(logits)
    }
}

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEAN_KEY_TEST_DICTIONARY")
            .expect("BEAN_KEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

#[test]
fn reuses_the_unconsumed_suffix_of_a_compatible_prediction() {
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    let mut model = PredictiveModel { evaluations: 0 };
    session.insert_str("あ", InputStyle::Direct, &tables);

    let first = session
        .predict_next_input_text(
            &tables,
            &mut model,
            &ZenzVersionConfig::default(),
            "左文脈",
            10,
            None,
        )
        .unwrap();
    assert_eq!(first, ("カナ".into(), 0));
    let evaluations = model.evaluations;

    session.insert_str("カ", InputStyle::Direct, &tables);
    let remaining = session
        .predict_next_input_text(
            &tables,
            &mut model,
            &ZenzVersionConfig::default(),
            "左文脈",
            10,
            None,
        )
        .unwrap();
    assert_eq!(remaining, ("ナ".into(), 0));
    assert_eq!(model.evaluations, evaluations);
}

#[test]
fn converts_generated_input_when_dictionary_prediction_is_empty() {
    let dictionary = DictionaryStore::open(dictionary_root()).unwrap();
    let converter = NormalConverter::new(&dictionary);
    let tables = InputTableRegistry::new();
    let mut session = ConversionSession::new();
    let mut model = PredictiveModel { evaluations: 0 };
    session.insert_str("あいうえおかきくけこ", InputStyle::Direct, &tables);
    let mut evaluated_prediction = false;

    let predictions = session
        .request_zenz_prediction(
            &converter,
            &tables,
            &mut model,
            &ZenzVersionConfig::default(),
            "",
            |session, _model| {
                evaluated_prediction = true;
                session.request_zenz_draft(
                    &converter,
                    &tables,
                    1,
                    &bean_key_converter::PrefixConstraint::default(),
                )?;
                Ok(())
            },
        )
        .unwrap();

    assert!(!predictions.is_empty());
    assert!(evaluated_prediction);
    assert!(model.evaluations > 0);
    assert_eq!(predictions[0].composing_count, ComposingCount::Surface(10));
}
