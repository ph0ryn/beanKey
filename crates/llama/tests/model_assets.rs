use std::path::PathBuf;

use beankey_llama::{LlamaContext, LlamaError};
use tempfile::TempDir;

fn fixed_model() -> PathBuf {
    std::env::var_os("BEANKEY_TEST_MODEL")
        .expect("BEANKEY_TEST_MODEL must identify the fixed GGUF model")
        .into()
}

fn assert_decodes(mut context: LlamaContext) {
    let tokens = context
        .tokenize("\u{ee00}テスト\u{ee01}候補", true)
        .unwrap();
    assert!(!tokens.is_empty());

    let logits = context.next_logits(&tokens).unwrap();
    assert!(context.vocabulary_size() > 0);
    assert_eq!(logits.len(), context.vocabulary_size());
    assert!(logits.iter().all(|value| value.is_finite()));
}

#[test]
fn bundled_inference_loads_and_decodes_with_only_the_model_path() {
    assert_decodes(LlamaContext::load(fixed_model()).unwrap());
}

#[test]
fn model_paths_preserve_spaces_and_non_ascii_characters() {
    let directory = TempDir::new().unwrap();
    let model = directory.path().join("かな model with spaces.gguf");
    std::fs::copy(fixed_model(), &model).unwrap();

    assert_decodes(LlamaContext::load(&model).unwrap());
}

#[test]
fn missing_models_return_an_explicit_load_error() {
    let directory = TempDir::new().unwrap();
    assert!(matches!(
        LlamaContext::load(directory.path().join("missing.gguf")),
        Err(LlamaError::Load(_))
    ));
}

#[test]
fn model_paths_reject_null_bytes_before_native_loading() {
    assert!(matches!(
        LlamaContext::load("model\0.gguf"),
        Err(LlamaError::InvalidPath)
    ));
}

#[test]
fn invalid_token_pieces_are_rejected_without_affecting_valid_tokens() {
    let context = LlamaContext::load(fixed_model()).unwrap();
    let tokens = context.tokenize("テスト", false).unwrap();
    let expected = context.token_to_piece(tokens[0]).unwrap();
    assert!(!expected.is_empty());

    for token in [-1, context.vocabulary_size() as i32, i32::MAX] {
        assert_eq!(
            context.token_to_piece(token),
            Err(LlamaError::TokenPiece(token))
        );
    }
    assert_eq!(context.token_to_piece(tokens[0]).unwrap(), expected);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_model_paths_preserve_non_unicode_native_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = TempDir::new().unwrap();
    let model = directory
        .path()
        .join(OsString::from_vec(b"model-\xff.gguf".to_vec()));
    std::fs::copy(fixed_model(), &model).unwrap();

    assert_decodes(LlamaContext::load(&model).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_non_unicode_model_paths_report_native_load_errors() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = TempDir::new().unwrap();
    let model = directory
        .path()
        .join(OsString::from_vec(b"model-\xff.gguf".to_vec()));
    // APFS cannot create this name. Preserve its native bytes and report llama.cpp's open failure,
    // rather than converting the name lossily or rejecting Unix paths as Windows UTF-8 paths.
    assert!(matches!(
        LlamaContext::load(model),
        Err(LlamaError::Load(_))
    ));
}

#[cfg(windows)]
#[test]
fn windows_model_paths_reject_unpaired_surrogates() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let model = OsString::from_wide(&[0xd800]);
    assert!(matches!(
        LlamaContext::load(model),
        Err(LlamaError::InvalidPath)
    ));
}
