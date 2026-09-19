use beankey_llama::{LlamaContext, LlamaSequence};

const PROMPTS: [&str; 4] = [
    "\u{ee00}テスト\u{ee01}候補",
    "\u{ee00}ハシ\u{ee01}箸",
    "\u{ee02}前文脈\u{ee00}カンジ\u{ee01}漢字",
    "\u{ee00}これはテストです\u{ee01}",
];

fn fixed_context() -> LlamaContext {
    let model = std::env::var("BEANKEY_TEST_MODEL")
        .expect("BEANKEY_TEST_MODEL must be provided by the Nix test environment");
    let backend = std::env::var("BEANKEY_TEST_LLAMA_BACKEND")
        .expect("BEANKEY_TEST_LLAMA_BACKEND must be provided by the Nix test environment");
    LlamaContext::load(model, backend).unwrap()
}

fn top_token(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .unwrap()
        .0
}

fn probabilities(values: &[f32]) -> Vec<f64> {
    let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let weights: Vec<_> = values
        .iter()
        .map(|value| (f64::from(*value) - f64::from(maximum)).exp())
        .collect();
    let sum: f64 = weights.iter().sum();
    weights.into_iter().map(|value| value / sum).collect()
}

fn assert_token_predictions(
    label: &str,
    actual: &[f32],
    expected: &[f32],
    vocabulary_size: usize,
) -> f64 {
    assert_eq!(actual.len(), expected.len(), "{label}: logit count changed");
    assert!(!actual.is_empty(), "{label}: no logits returned");
    assert_eq!(actual.len() % vocabulary_size, 0);
    let mut maximum_variation = 0.0_f64;
    for (row, (actual, expected)) in actual
        .chunks_exact(vocabulary_size)
        .zip(expected.chunks_exact(vocabulary_size))
        .enumerate()
    {
        assert!(
            actual.iter().chain(expected).all(|value| value.is_finite()),
            "{label}: non-finite logit in row {row}"
        );
        assert_eq!(
            top_token(actual),
            top_token(expected),
            "{label}: token changed in row {row}"
        );
        let variation = probabilities(actual)
            .iter()
            .zip(probabilities(expected))
            .map(|(left, right)| (left - right).abs())
            .sum::<f64>()
            / 2.0;
        maximum_variation = maximum_variation.max(variation);
    }
    maximum_variation
}

fn compare_shared_prefix(context: &mut LlamaContext, prompt: &str, history: &str) -> f64 {
    let tokens = context.tokenize(prompt, true).unwrap();
    let vocabulary_size = context.vocabulary_size();
    let batched = context
        .logits(&tokens, 0, LlamaSequence::Evaluation)
        .unwrap();
    let cached = context.next_logits(&tokens).unwrap();
    assert_eq!(batched.len(), tokens.len() * vocabulary_size);
    assert_eq!(cached.len(), vocabulary_size);
    assert_token_predictions(
        &format!("{history}: batched versus cached for {prompt:?}"),
        &batched[batched.len() - vocabulary_size..],
        &cached,
        vocabulary_size,
    );

    // Match decode batch sizes, but use one sequence in the reference so it
    // reuses its own prefix instead of sharing it from another sequence.
    let mut reference = fixed_context();
    let fresh_batched = reference
        .logits(&tokens, 0, LlamaSequence::InputPrediction)
        .unwrap();
    let fresh_cached = reference.next_logits(&tokens).unwrap();
    let mut maximum_variation = 0.0_f64;
    for (mode, actual, expected) in [
        ("batched", &batched, &fresh_batched),
        ("cached", &cached, &fresh_cached),
    ] {
        let variation = assert_token_predictions(
            &format!("{history}: {mode} for {prompt:?}"),
            actual,
            expected,
            vocabulary_size,
        );
        eprintln!("{history}: {mode} for {prompt:?}: maximum row total variation = {variation}");
        maximum_variation = maximum_variation.max(variation);
    }
    maximum_variation
}

#[test]
fn isolated_prefix_sharing_preserves_logits() {
    for prompt in PROMPTS {
        // No previous prompt history affects allocation. Retain the numerical
        // bound for this isolated prefix-copy baseline and its same-sequence
        // reference, including their matching prefill batches.
        let variation = compare_shared_prefix(&mut fixed_context(), prompt, "isolated");
        assert!(
            variation < 0.01,
            "isolated prefix sharing changed the distribution by {variation} for {prompt:?}"
        );
    }
}

#[test]
fn prompt_history_preserves_token_predictions() {
    let mut reversed = PROMPTS;
    reversed.reverse();
    for (history, prompts) in [("forward", PROMPTS), ("reverse", reversed)] {
        let mut context = fixed_context();
        for prompt in prompts {
            // Retained sequences can change physical KV order and floating-point
            // accumulation even on the same CPU. Keep TV as a diagnostic (visible
            // with --nocapture), not a fixed bound across different KV layouts.
            compare_shared_prefix(&mut context, prompt, history);
        }
    }
}
