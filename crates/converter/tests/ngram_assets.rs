use std::path::{Path, PathBuf};
use std::sync::Arc;

use beankey_converter::{
    EfficientNGram, NGramError, NGramLanguageModel, ZenzLanguageModel, ZenzTokenizer,
};

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
fn preserves_native_probabilities_across_shared_parallel_context_searches() {
    // Golden values come from the native MARISA-backed public API on these fixtures.
    for (fixture, n, token_20, token_21, other) in [
        (
            "lm",
            2,
            0.541_708_333_333_333_3,
            0.208_375,
            0.000_041_666_666_666_666_665,
        ),
        (
            "personal",
            2,
            0.041_708_333_333_333_33,
            0.708_375,
            0.000_041_666_666_666_666_665,
        ),
        ("lm", 5, 0.250_125, 0.000_125, 0.000_125),
        ("personal", 5, 0.250_125, 0.000_125, 0.000_125),
    ] {
        let prefix = fixture_prefix().with_file_name(fixture);
        let model = Arc::new(EfficientNGram::open(prefix, n, 0.75).unwrap());
        let mut populated = vec![other; 6_000];
        populated[20] = token_20;
        populated[21] = token_21;
        let uniform = vec![1.0 / 6_000.0; 6_000];
        let cases = [
            (&[][..], uniform.clone()),
            (&[10][..], populated.clone()),
            (&[11][..], uniform),
            (&[5999, 10][..], populated),
        ];

        std::thread::scope(|scope| {
            for worker in 0..4 {
                let model = Arc::clone(&model);
                let cases = &cases;
                scope.spawn(move || {
                    for round in 0..16 {
                        let (history, expected) = &cases[(worker + round) % cases.len()];
                        let actual = model.probabilities(history).unwrap();
                        assert_eq!(actual.len(), 6_000);
                        assert!((actual.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                        for (token, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                            assert!(
                                (actual - expected).abs() < 1e-12,
                                "{fixture}, n={n}, history {history:?}, token {token}: \
                                 expected {expected}, got {actual}"
                            );
                        }
                    }
                });
            }
        });
    }
}

#[test]
fn reports_missing_corrupt_and_truncated_tries_with_the_failing_path() {
    const SUFFIXES: [&str; 4] = ["c_abc", "u_abx", "u_xbc", "r_xbx"];

    for suffix in SUFFIXES {
        let temporary = tempfile::tempdir().unwrap();
        let prefix = temporary.path().join("lm");
        let trie_path = temporary.path().join(format!("lm_{suffix}.marisa"));
        for other in SUFFIXES.into_iter().filter(|other| *other != suffix) {
            let name = format!("lm_{other}.marisa");
            std::fs::copy(
                fixture_prefix().with_file_name(&name),
                temporary.path().join(name),
            )
            .unwrap();
        }
        let assert_load_error = || {
            assert!(matches!(
                EfficientNGram::open(&prefix, 2, 0.75),
                Err(NGramError::LoadTrie(path)) if path == trie_path
            ));
        };
        assert_load_error();

        let source =
            std::fs::read(fixture_prefix().with_file_name(format!("lm_{suffix}.marisa"))).unwrap();
        let mut invalid_config = source.clone();
        let config_start = invalid_config.len() - 4;
        invalid_config[config_start..].fill(0xff);
        for bytes in [
            b"not a MARISA dictionary".as_slice(),
            &source[..source.len() - 1],
            &invalid_config,
        ] {
            std::fs::write(&trie_path, bytes).unwrap();
            assert_load_error();
        }
    }
}

#[test]
fn rejects_nul_in_the_prefix_with_the_invalid_trie_path() {
    let prefix = PathBuf::from("lm\0ignored");
    let trie_path = PathBuf::from("lm\0ignored_c_abc.marisa");

    assert!(matches!(
        EfficientNGram::open(prefix, 2, 0.75),
        Err(NGramError::InvalidPath(path)) if path == trie_path
    ));
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
