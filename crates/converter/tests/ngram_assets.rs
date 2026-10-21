use std::path::{Path, PathBuf};
use std::sync::Arc;

use beankey_converter::{
    EfficientNGram, NGramError, NGramLanguageModel, ZenzLanguageModel, ZenzTokenizer,
};

fn fixture_prefix() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ngram/lm")
}

fn tokenizer_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/tokenizer/tokenizer.json")
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
    let tokenizer = ZenzTokenizer::open(tokenizer_path()).unwrap();
    // Golden IDs and decoded text were captured with tokenizers 0.23.1 + onig.
    // The fixed asset drops ASCII spaces/CR/LF; decode is not a text roundtrip.
    let cases: &[(&str, &[i32], &str)] = &[
        ("", &[], ""),
        (
            "これは日本語です",
            &[268, 262, 253, 304, 358, 698, 246, 255],
            "これは日本語です",
        ),
        (
            " leading  spaces\tand\nnewlines\r\n ",
            &[
                79, 72, 68, 71, 76, 81, 74, 86, 83, 68, 70, 72, 86, 185, 68, 81, 71, 81, 72, 90,
                79, 76, 81, 72, 86,
            ],
            "leadingspaces\tandnewlines",
        ),
        (
            " \t\r\n\u{a0}\u{3000}",
            &[185, 2577, 567],
            "\t\u{a0}\u{3000}",
        ),
        (
            "we're I've I'll I'd don't it's I'M WE'RE",
            &[
                90, 72, 10, 85, 72, 44, 10, 89, 72, 44, 10, 79, 79, 44, 10, 71, 71, 82, 81, 10, 87,
                76, 87, 10, 86, 44, 10, 48, 58, 40, 10, 53, 40,
            ],
            "we'reI'veI'llI'ddon'tit'sI'MWE'RE",
        ),
        (
            "ASCII\u{3000}SPACE",
            &[36, 54, 38, 44, 44, 567, 54, 51, 36, 38, 40],
            "ASCII\u{3000}SPACE",
        ),
        (
            "漢字かなカナabc123 １２３ Ελληνικά",
            &[
                1922, 1112, 260, 252, 350, 434, 68, 69, 70, 20, 21, 22, 678, 768, 958, 140, 223,
                4178, 4178, 4912, 4590, 4978, 4010, 140, 109,
            ],
            "漢字かなカナabc123１２３Ελληνικά",
        ),
        (
            "é e\u{301} が か\u{3099} ｶﾞ",
            &[2478, 72, 4095, 254, 260, 2079, 2813, 1963],
            "ée\u{301}がか\u{3099}ｶﾞ",
        ),
        (
            "👩‍💻🇯🇵👍🏽、。！？",
            &[
                3470, 106, 3825, 2913, 123, 4786, 111, 4786, 117, 4412, 3956, 125, 241, 248, 469,
                608,
            ],
            "👩‍💻🇯🇵👍🏽、。！？",
        ),
        (
            "\u{ee00}ハシ\u{ee01}箸",
            &[172, 120, 202, 623, 330, 172, 120, 203, 2993],
            "\u{ee00}ハシ\u{ee01}箸",
        ),
        (
            "\u{ee02}前文脈\u{ee00}カンジ\u{ee01}漢字",
            &[
                172, 120, 204, 480, 679, 2121, 172, 120, 202, 350, 259, 354, 172, 120, 203, 1922,
                1112,
            ],
            "\u{ee02}前文脈\u{ee00}カンジ\u{ee01}漢字",
        ),
        ("a\0b\u{1f}c", &[68, 177, 69, 200, 70], "a\0b\u{1f}c"),
        (
            "<s>日本語</s>[PAD][UNK]",
            &[2, 304, 358, 698, 3, 1, 0],
            "日本語",
        ),
        (
            "これは  日本語\tです\n漢字\r\nかな\u{85}\u{2028}終",
            &[
                268, 262, 253, 304, 358, 698, 185, 246, 255, 1922, 1112, 260, 252, 128, 207, 5434,
                831,
            ],
            "これは日本語\tです漢字かな\u{85}\u{2028}終",
        ),
        ("² Ⅳ ٣", &[3769, 4185, 151, 100], "²Ⅳ٣"),
        (
            "か\u{3099}☀️’s",
            &[260, 2079, 4628, 2640, 2289, 86],
            "か\u{3099}☀️’s",
        ),
    ];
    for (input, expected_tokens, expected_text) in cases {
        let tokens = tokenizer.encode(input).unwrap();
        assert_eq!(&tokens, expected_tokens, "token IDs for {input:?}");
        assert_eq!(
            tokenizer.decode(&tokens).unwrap(),
            *expected_text,
            "decoded text for {input:?}"
        );
    }
}

#[test]
fn preserves_fixed_token_ids_across_long_whitespace_and_text_runs() {
    let tokenizer = ZenzTokenizer::open(tokenizer_path()).unwrap();
    let text = "これは日本語です";
    let expected_tokens = [268, 262, 253, 304, 358, 698, 246, 255];

    // Exercise the ByteLevel regex's whitespace lookahead before text and at EOF.
    let padded = format!("{}{text}{}", " ".repeat(16_384), " ".repeat(16_384));
    assert_eq!(tokenizer.encode(&padded).unwrap(), expected_tokens);
    let repeated = format!("{text} ").repeat(1_024);
    let tokens = tokenizer.encode(&repeated).unwrap();
    assert_eq!(tokens, expected_tokens.repeat(1_024));
    assert_eq!(tokenizer.decode(&tokens).unwrap(), text.repeat(1_024));
}

#[test]
fn exposes_ngram_probabilities_through_the_typo_language_model_boundary() {
    let mut model = NGramLanguageModel::open(fixture_prefix(), tokenizer_path(), 2, 0.75).unwrap();
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
