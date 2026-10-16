use super::*;
use crate::{ComposingCount, DictionaryEntry};

#[derive(Default)]
struct ComparisonModel {
    row_shift: f32,
    tokenizations: Vec<(String, bool)>,
    requests: Vec<(Vec<i32>, usize, ZenzInferenceSequence)>,
    next_evaluations: usize,
    suffix_error: Option<(usize, ZenzInferenceError)>,
    tokenization_error: Option<ZenzInferenceError>,
    merge_alignment: bool,
}

impl ComparisonModel {
    fn row(&self, tokens: &[i32]) -> Vec<f32> {
        let values = match tokens.last() {
            Some(1) => [-20.0, -20.0, -4.0, 2.0, 1.0, 3.0, -2.0, 0.0, -1.0],
            Some(3) => [-20.0, -20.0, 1.0, 0.0, 2.0, -5.0, -1.0, -20.0, -20.0],
            Some(4) => [-20.0, -20.0, 4.0, 2.0, 0.0, -5.0, -3.0, -20.0, -20.0],
            _ => [-20.0, -20.0, 3.0, 1.0, 0.0, -5.0, -2.0, -20.0, -20.0],
        };
        values
            .into_iter()
            .map(|value| value + self.row_shift * tokens.len() as f32)
            .collect()
    }
}

impl ZenzLanguageModel for ComparisonModel {
    fn vocabulary_size(&self) -> usize {
        9
    }

    fn eos_token(&self) -> i32 {
        2
    }

    fn tokenize(&mut self, text: &str, add_special: bool) -> Result<Vec<i32>, ZenzInferenceError> {
        self.tokenizations.push((text.to_owned(), add_special));
        if let Some(error) = self.tokenization_error.take() {
            return Err(error);
        }
        if add_special {
            return Ok(vec![1]);
        }
        if self.merge_alignment && text == format!("a{ALIGNMENT_SEPARATOR}") {
            return Ok(vec![7]);
        }
        Ok(text
            .chars()
            .map(|character| match character {
                'a' => 3,
                'b' => 4,
                'x' => 5,
                ALIGNMENT_SEPARATOR => 6,
                'c' => 7,
                'y' => 8,
                _ => 0,
            })
            .collect())
    }

    fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
        Ok(match token {
            3 => b"a".to_vec(),
            4 => b"b".to_vec(),
            5 => b"x".to_vec(),
            6 => ALIGNMENT_SEPARATOR.to_string().into_bytes(),
            7 => b"c".to_vec(),
            8 => b"y".to_vec(),
            _ => Vec::new(),
        })
    }

    fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
        self.next_evaluations += 1;
        Ok(self.row(tokens))
    }

    fn logits_for_suffix(
        &mut self,
        tokens: &[i32],
        start_index: usize,
        sequence: ZenzInferenceSequence,
    ) -> Result<Vec<f32>, ZenzInferenceError> {
        self.requests.push((tokens.to_vec(), start_index, sequence));
        if let Some((call, error)) = &self.suffix_error
            && *call == self.requests.len()
        {
            return Err(error.clone());
        }
        Ok((start_index..tokens.len())
            .flat_map(|index| self.row(&tokens[..=index]))
            .collect())
    }
}

fn candidate(text: &str, ruby: &str) -> Candidate {
    let count = UnicodeSegmentation::graphemes(ruby, true).count();
    Candidate::single(
        text.into(),
        -10.0,
        ComposingCount::Surface(count),
        501,
        vec![DictionaryEntry {
            word: text.into(),
            ruby: ruby.into(),
            left_id: 1285,
            right_id: 1285,
            meaning_id: 501,
            base_value: -10.0,
            adjustment: 0.0,
            metadata: DictionaryMetadata::default(),
        }],
    )
}

fn ordinary_request<'a>(candidates: &'a [Candidate]) -> ZenzComparisonRequest<'a> {
    ZenzComparisonRequest {
        input: "ア",
        input_cursor_position: None,
        candidates,
        prefix_constraint: &EMPTY_CONSTRAINT,
        version: &ORDINARY_VERSION,
        token_budget: 32,
    }
}

static EMPTY_CONSTRAINT: PrefixConstraint = PrefixConstraint {
    bytes: Vec::new(),
    has_eos: false,
    ignore_memory_and_user_dictionary: false,
    fixed_prefix: None,
};
static ORDINARY_VERSION: ZenzVersionConfig = ZenzVersionConfig::V3(ZenzV3Config {
    profile: None,
    topic: None,
    style: None,
    preference: None,
    left_context: None,
    right_context: None,
    max_left_context_length: None,
    max_right_context_length: None,
    enable_alignment_separator: false,
});

fn expected_log_probability(row: &[f32], token: usize) -> f64 {
    f64::from(row[token])
        - row
            .iter()
            .map(|&value| f64::from(value).exp())
            .sum::<f64>()
            .ln()
}

#[test]
fn compares_chosen_tokens_with_normalized_log_probabilities_and_full_termination() {
    let mut model = ComparisonModel::default();
    let candidates = [candidate("a", "ア"), candidate("b", "ア")];
    let initial = model.row(&[1]);
    let expected = [
        expected_log_probability(&initial, 3) + expected_log_probability(&model.row(&[1, 3]), 2),
        expected_log_probability(&initial, 4) + expected_log_probability(&model.row(&[1, 4]), 2),
    ];
    let scores = ZenzEvaluator::default()
        .compare_candidates(&mut model, ordinary_request(&candidates))
        .unwrap()
        .unwrap();
    for (&actual, expected) in scores.iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        assert!(actual < 0.0);
    }
    // The model's top token is x, not either candidate. Comparison scores the
    // chosen alternatives anyway, unlike greedy candidate evaluation.
    assert_eq!(model.requests.len(), 2);
    assert_eq!(
        model.requests[0],
        (vec![1, 3], 0, ZenzInferenceSequence::Evaluation)
    );
    assert_eq!(
        model.requests[1],
        (vec![1, 4], 0, ZenzInferenceSequence::Evaluation)
    );
    assert_eq!(model.next_evaluations, 0);
}

#[test]
fn comparison_is_invariant_to_large_independent_row_logit_shifts() {
    let candidates = [candidate("a", "ア"), candidate("b", "ア")];
    let mut shifted = ComparisonModel {
        row_shift: 10_000.0,
        ..Default::default()
    };
    let original = ZenzEvaluator::default()
        .compare_candidates(
            &mut ComparisonModel::default(),
            ordinary_request(&candidates),
        )
        .unwrap();
    let shifted = ZenzEvaluator::default()
        .compare_candidates(&mut shifted, ordinary_request(&candidates))
        .unwrap();
    assert_eq!(original, shifted);
}

#[test]
fn comparison_uses_the_same_deterministic_explicit_dictionary_union_for_all_paths() {
    let mut split = candidate("ab", "アイ");
    let mut first = candidate("a", "ア").entries.remove(0);
    first.metadata.insert(DictionaryMetadata::USER_DICTIONARY);
    first.metadata.insert(DictionaryMetadata::LEARNED);
    let mut learned = candidate("b", "イ").entries.remove(0);
    learned.metadata.insert(DictionaryMetadata::LEARNED);
    split.entries = vec![first, learned];
    let mut whole = candidate("ab", "アイ");
    whole.entries[0]
        .metadata
        .insert(DictionaryMetadata::USER_DICTIONARY);
    whole.value = 100.0;
    let mut candidates = [split, whole];
    let mut model = ComparisonModel::default();
    let mut request = ordinary_request(&candidates);
    request.input = "アイ";
    let scores = ZenzEvaluator::default()
        .compare_candidates(&mut model, request)
        .unwrap()
        .unwrap();
    assert_eq!(scores[0], scores[1]);
    let prompt =
        ZenzPromptBuilder::candidate_evaluation("アイ", None, "a(あ)ab(あい)", &ORDINARY_VERSION);
    assert_eq!(
        model
            .tokenizations
            .iter()
            .filter(|(_, special)| *special)
            .collect::<Vec<_>>(),
        vec![&(prompt.clone(), true)]
    );
    assert!(!prompt.contains("b(い)"));
    candidates.reverse();
    let mut reversed_model = ComparisonModel::default();
    let mut request = ordinary_request(&candidates);
    request.input = "アイ";
    ZenzEvaluator::default()
        .compare_candidates(&mut reversed_model, request)
        .unwrap();
    assert_eq!(reversed_model.tokenizations[0], (prompt, true));
}

#[test]
fn preflights_the_entire_group_budget_before_any_logits() {
    let candidates = [candidate("a", "ア"), candidate("ab", "ア")];
    let mut model = ComparisonModel::default();
    let mut request = ordinary_request(&candidates);
    request.token_budget = 4; // a + EOS + ab + EOS requires five rows.
    assert!(
        ZenzEvaluator::default()
            .compare_candidates(&mut model, request)
            .unwrap()
            .is_none()
    );
    assert!(model.requests.is_empty());
    assert_eq!(model.next_evaluations, 0);
    assert!(model.tokenizations.iter().any(|(text, _)| text == "ab"));

    let mut request = ordinary_request(&candidates);
    request.token_budget = 5;
    assert!(
        ZenzEvaluator::default()
            .compare_candidates(&mut model, request)
            .unwrap()
            .is_some()
    );
    assert_eq!(model.requests.len(), 2);
}

#[test]
fn fixed_prefix_is_common_unscored_left_context_with_partial_alignment_termination() {
    let mut candidates = [candidate("xa", "アイ"), candidate("xb", "アイ")];
    for (index, candidate) in candidates.iter_mut().enumerate() {
        let mut prefix = self::candidate("x", "ア").entries.remove(0);
        prefix.metadata.insert(DictionaryMetadata::USER_DICTIONARY);
        candidate.entries = vec![
            prefix,
            self::candidate(if index == 0 { "a" } else { "b" }, "イ")
                .entries
                .remove(0),
        ];
    }
    let constraint = PrefixConstraint {
        bytes: b"x".to_vec(),
        fixed_prefix: Some(FixedPrefix {
            byte_count: 1,
            ruby_count: 1,
        }),
        ..Default::default()
    };
    let version = ZenzVersionConfig::V3(ZenzV3Config {
        enable_alignment_separator: true,
        left_context: Some("ctx".into()),
        ..Default::default()
    });
    let mut evaluator = ZenzEvaluator {
        previous_evaluation_prompt: "greedy task".into(),
        ..Default::default()
    };
    let mut model = ComparisonModel::default();
    let scores = evaluator
        .compare_candidates(
            &mut model,
            ZenzComparisonRequest {
                input: "アイウ",
                input_cursor_position: Some(2),
                candidates: &candidates,
                prefix_constraint: &constraint,
                version: &version,
                token_budget: 4,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        model.tokenizations,
        vec![
            ("\u{EE02}ctxx\u{EE00}イ\u{EE08}ウ\u{EE01}".into(), true),
            (format!("a{ALIGNMENT_SEPARATOR}"), false),
            (format!("b{ALIGNMENT_SEPARATOR}"), false),
        ]
    );
    let expected = expected_log_probability(&model.row(&[1]), 3)
        + expected_log_probability(&model.row(&[1, 3]), 6);
    assert!((scores[0] - expected).abs() < 1e-12);
    assert_eq!(model.requests[0].0, vec![1, 3]);
    assert_eq!(evaluator.previous_evaluation_prompt, "greedy task");
    assert!(evaluator.evaluations.entries.is_empty());
}

#[test]
fn alignment_terminal_is_tokenized_together_with_candidate_text_without_eos() {
    let candidates = [candidate("a", "ア")];
    let mut model = ComparisonModel {
        merge_alignment: true,
        ..Default::default()
    };
    let version = ZenzVersionConfig::V3(ZenzV3Config {
        enable_alignment_separator: true,
        ..Default::default()
    });
    let mut request = ordinary_request(&candidates);
    request.input = "アイ";
    request.input_cursor_position = Some(1);
    request.version = &version;
    request.token_budget = 1;
    let score = ZenzEvaluator::default()
        .compare_candidates(&mut model, request)
        .unwrap()
        .unwrap()[0];
    assert!((score - expected_log_probability(&model.row(&[1]), 7)).abs() < 1e-12);
    assert_eq!(model.requests[0].0, vec![1]);
}

#[test]
fn invalid_or_mismatched_fixed_prefixes_skip_the_group_without_inference() {
    let candidates = [candidate("xa", "アイ"), candidate("yb", "アイ")];
    let constraints = [
        PrefixConstraint {
            bytes: b"x".to_vec(),
            fixed_prefix: Some(FixedPrefix {
                byte_count: 1,
                ruby_count: 1,
            }),
            ..Default::default()
        },
        PrefixConstraint {
            bytes: "橋".as_bytes().to_vec(),
            fixed_prefix: Some(FixedPrefix {
                byte_count: 1,
                ruby_count: 1,
            }),
            ..Default::default()
        },
        PrefixConstraint {
            bytes: b"x".to_vec(),
            fixed_prefix: Some(FixedPrefix {
                byte_count: 9,
                ruby_count: 1,
            }),
            ..Default::default()
        },
        PrefixConstraint {
            bytes: b"x".to_vec(),
            fixed_prefix: Some(FixedPrefix {
                byte_count: 1,
                ruby_count: 3,
            }),
            ..Default::default()
        },
        PrefixConstraint {
            bytes: Vec::new(),
            fixed_prefix: Some(FixedPrefix::default()),
            ..Default::default()
        },
    ];
    let mut model = ComparisonModel::default();
    for constraint in &constraints {
        let mut request = ordinary_request(&candidates);
        request.input = "アイ";
        request.prefix_constraint = constraint;
        assert!(
            ZenzEvaluator::default()
                .compare_candidates(&mut model, request)
                .unwrap()
                .is_none()
        );
    }
    assert!(model.tokenizations.is_empty());
    assert!(model.requests.is_empty());
}

#[test]
fn empty_invalid_prediction_and_differently_consumed_candidates_skip_safely() {
    let mut different_input = candidate("b", "ア");
    different_input.composing_count = ComposingCount::Input(1);
    let mut typo = candidate("b", "ア");
    typo.is_typo_correction = true;
    let groups = [
        Vec::new(),
        vec![candidate("", "ア")],
        vec![candidate("\n\n", "ア")],
        vec![candidate("a", "ア"), candidate("b", "アイ")],
        vec![candidate("a", "ア"), different_input],
        vec![candidate("a", "ア"), typo],
        vec![candidate("a", "ア"); 5],
    ];
    let mut model = ComparisonModel::default();
    for candidates in &groups {
        assert!(
            ZenzEvaluator::default()
                .compare_candidates(&mut model, ordinary_request(candidates))
                .unwrap()
                .is_none()
        );
    }
    let candidates = [candidate("a", "ア")];
    for (input, cursor, budget) in [
        ("", None, 32),
        ("ア", Some(0), 32),
        ("ア", Some(2), 32),
        ("ア", None, 0),
    ] {
        let mut request = ordinary_request(&candidates);
        request.input = input;
        request.input_cursor_position = cursor;
        request.token_budget = budget;
        assert!(
            ZenzEvaluator::default()
                .compare_candidates(&mut model, request)
                .unwrap()
                .is_none()
        );
    }
    assert!(model.tokenizations.is_empty());
    assert!(model.requests.is_empty());
}

#[test]
fn unframed_partial_tasks_skip_instead_of_using_full_result_eos() {
    let candidates = [candidate("a", "ア")];
    let mut request = ordinary_request(&candidates);
    request.input = "アイ";
    request.input_cursor_position = Some(1);
    let mut model = ComparisonModel::default();
    assert!(
        ZenzEvaluator::default()
            .compare_candidates(&mut model, request)
            .unwrap()
            .is_none()
    );
    assert!(model.requests.is_empty());
}

#[test]
fn context_capacity_skips_the_whole_group_but_genuine_inference_errors_propagate() {
    let candidates = [candidate("a", "ア"), candidate("b", "ア")];
    let mut model = ComparisonModel {
        suffix_error: Some((
            2,
            ZenzInferenceError::ContextCapacity {
                tokens: 513,
                capacity: 512,
            },
        )),
        ..Default::default()
    };
    assert!(
        ZenzEvaluator::default()
            .compare_candidates(&mut model, ordinary_request(&candidates))
            .unwrap()
            .is_none()
    );
    assert_eq!(model.requests.len(), 2);
    let mut model = ComparisonModel {
        suffix_error: Some((2, ZenzInferenceError::Message("decode failed".into()))),
        ..Default::default()
    };
    assert_eq!(
        ZenzEvaluator::default().compare_candidates(&mut model, ordinary_request(&candidates)),
        Err(ZenzInferenceError::Message("decode failed".into()))
    );
    let mut model = ComparisonModel {
        tokenization_error: Some(ZenzInferenceError::ContextCapacity {
            tokens: 513,
            capacity: 512,
        }),
        ..Default::default()
    };
    assert!(
        ZenzEvaluator::default()
            .compare_candidates(&mut model, ordinary_request(&candidates))
            .unwrap()
            .is_none()
    );
    assert!(model.requests.is_empty());
}

#[test]
fn learned_only_metadata_cannot_accept_a_non_greedy_token() {
    let plain = candidate("a", "ア");
    let mut learned = plain.clone();
    learned.entries[0]
        .metadata
        .insert(DictionaryMetadata::LEARNED);
    for candidate in [&plain, &learned] {
        let mut model = ComparisonModel::default();
        assert_eq!(
            evaluate_candidate(
                &mut model,
                ZenzEvaluationRequest {
                    input: "ア",
                    input_cursor_position: None,
                    candidate,
                    request_rich_candidates: false,
                    prefix_constraint: &EMPTY_CONSTRAINT,
                    personalization: None,
                    version: &ORDINARY_VERSION,
                }
            )
            .unwrap(),
            CandidateEvaluation::FixRequired(b"x".to_vec())
        );
        assert_eq!(
            model.tokenizations,
            vec![("\u{EE00}ア\u{EE01}".into(), true), ("a".into(), false),]
        );
    }
}

#[test]
fn greedy_evaluation_stays_opt_in_and_preserves_explicit_dictionary_and_ngram_apis() {
    let mut candidate = candidate("a", "ア");
    candidate.entries[0]
        .metadata
        .insert(DictionaryMetadata::USER_DICTIONARY);
    let mut evaluator = ZenzEvaluator::default();
    let mut model = ComparisonModel::default();
    let evaluation = evaluator
        .evaluate(
            &mut model,
            ZenzEvaluationRequest {
                input: "ア",
                input_cursor_position: None,
                candidate: &candidate,
                request_rich_candidates: false,
                prefix_constraint: &EMPTY_CONSTRAINT,
                personalization: None,
                version: &ORDINARY_VERSION,
            },
        )
        .unwrap();
    assert_eq!(evaluation, CandidateEvaluation::FixRequired(b"x".to_vec()));
    assert!(model.tokenizations[0].0.contains("a(あ)"));
    assert_eq!(model.requests.len(), 1);
    assert_eq!(model.requests[0].0, vec![1]); // No terminal scoring/comparison.
    assert!(evaluator.previous_evaluation_prompt.contains("a(あ)"));

    struct Probabilities {
        prefer_candidate: bool,
    }
    impl TokenProbabilityModel for Probabilities {
        fn probabilities(&self, _prefix: &[i32], vocabulary_size: usize) -> Option<Vec<f32>> {
            let mut probabilities = vec![0.1; vocabulary_size];
            if self.prefer_candidate {
                probabilities[3] = 0.9;
            }
            Some(probabilities)
        }
    }
    let base = Probabilities {
        prefer_candidate: false,
    };
    let personal = Probabilities {
        prefer_candidate: true,
    };
    let personalized_candidate = self::candidate("xa", "ア");
    let evaluation = evaluate_candidate(
        &mut ComparisonModel::default(),
        ZenzEvaluationRequest {
            input: "ア",
            input_cursor_position: None,
            candidate: &personalized_candidate,
            request_rich_candidates: false,
            prefix_constraint: &EMPTY_CONSTRAINT,
            personalization: Some(ZenzPersonalization {
                alpha: 1.0,
                base: &base,
                personal: &personal,
            }),
            version: &ORDINARY_VERSION,
        },
    )
    .unwrap();
    assert!(matches!(evaluation, CandidateEvaluation::Pass { .. }));
}

#[test]
fn malformed_log_probability_rows_are_errors_not_nan_scores() {
    for row in [
        vec![],
        vec![f32::NAN, 0.0],
        vec![f32::INFINITY, 0.0],
        vec![f32::NEG_INFINITY; 2],
    ] {
        assert!(chosen_token_log_probability(&row, 0).is_err());
    }
    assert!(chosen_token_log_probability(&[0.0], -1).is_err());
    assert!(chosen_token_log_probability(&[0.0], 1).is_err());
    assert_eq!(
        chosen_token_log_probability(&[f32::NEG_INFINITY, 0.0], 0).unwrap(),
        f64::NEG_INFINITY
    );
}
