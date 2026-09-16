use std::path::{Path, PathBuf};

use beankey_converter::{EfficientNGram, NGramLanguageModel, ZenzLanguageModel, ZenzTokenizer};

fn fixture_prefix() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ngram/lm")
}

#[test]
fn predicts_with_fixed_marisa_count_tries() {
    let model = EfficientNGram::open(fixture_prefix(), 2, 0.75).unwrap();
    let probabilities = model.probabilities(&[10]).unwrap();

    assert_eq!(probabilities.len(), 6_000);
    assert!((probabilities.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!((probabilities[20] - 0.541_708_333_333_333_3).abs() < 1e-12);
    assert!((probabilities[21] - 0.208_375).abs() < 1e-12);
    assert!((probabilities[22] - 0.000_041_666_666_666_666_665).abs() < 1e-12);
}

#[test]
fn tokenizes_with_the_fixed_upstream_zenz_asset() {
    let tokenizer_path = std::env::var_os("BEANKEY_TEST_ZENZ_TOKENIZER")
        .expect("BEANKEY_TEST_ZENZ_TOKENIZER must point to the fixed tokenizer.json");
    let tokenizer = ZenzTokenizer::open(tokenizer_path).unwrap();
    let tokens = tokenizer.encode("これは日本語です").unwrap();

    assert_eq!(tokens, [268, 262, 253, 304, 358, 698, 246, 255]);
    assert_eq!(tokenizer.decode(&tokens).unwrap(), "これは日本語です");
}

#[test]
fn exposes_ngram_probabilities_through_the_typo_language_model_boundary() {
    let tokenizer_path = std::env::var_os("BEANKEY_TEST_ZENZ_TOKENIZER")
        .expect("BEANKEY_TEST_ZENZ_TOKENIZER must point to the fixed tokenizer.json");
    let mut model = NGramLanguageModel::open(fixture_prefix(), tokenizer_path, 2, 0.75).unwrap();
    let logits = model.next_logits(&[10]).unwrap();

    assert_eq!(logits.len(), 6_000);
    assert_eq!(
        logits
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(token, _)| token),
        Some(20)
    );
}
