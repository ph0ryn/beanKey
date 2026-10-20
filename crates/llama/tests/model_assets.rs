use beankey_llama::LlamaContext;

#[test]
fn bundled_inference_loads_and_decodes_with_only_the_model_path() {
    let model = std::env::var_os("BEANKEY_TEST_MODEL")
        .expect("BEANKEY_TEST_MODEL must identify the fixed GGUF model");
    let mut context = LlamaContext::load(model).unwrap();
    let tokens = context.tokenize("\u{ee00}テスト\u{ee01}候補", true).unwrap();
    assert!(!tokens.is_empty());

    let logits = context.next_logits(&tokens).unwrap();
    assert!(context.vocabulary_size() > 0);
    assert_eq!(logits.len(), context.vocabulary_size());
    assert!(logits.iter().all(|value| value.is_finite()));
}
