use beankey_daemon::{DaemonConfig, DaemonConfigError};

const CONFIG: &str = r#"
[conversion]
[zenz]
[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#;

#[test]
fn parses_configuration_without_fixed_asset_paths_or_socket() {
    let config = DaemonConfig::parse(CONFIG).unwrap();
    assert_eq!(config.conversion.n_best, 10);
}

#[test]
fn loads_empty_and_partial_toml_with_defaults() {
    let defaults = DaemonConfig::parse(CONFIG).unwrap();
    for source in [
        "",
        "# All settings are optional.\n",
        "[conversion]\n[learning]\n[zenz]\n[lm_typo]\n[inference]\n",
        "[inference]\nflash_attention = true\n",
    ] {
        assert_eq!(DaemonConfig::parse(source).unwrap(), defaults);
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    assert_eq!(DaemonConfig::load(file.path()).unwrap(), defaults);

    let mut expected = defaults;
    expected.conversion.type_backslash = true;
    expected.zenz.inference_limit = 4;
    assert_eq!(
        DaemonConfig::parse("[conversion]\ntype_backslash = true\n[zenz]\ninference_limit = 4\n")
            .unwrap(),
        expected
    );
}

#[test]
fn partial_toml_still_rejects_invalid_settings() {
    for source in [
        "unexpected = true",
        "[conversion]\nunexpected = true",
        "[conversion]\ntype_backslash = 1",
        "[conversion]\nn_best = 0",
        "[inference]\ncontext_size = 1024",
        "[inference]\nflash_attention = false",
        "[zenz.personalization]\nalpha = 1.0",
        "[lm_typo]\nenabled = true\nlanguage_model = 'ngram'",
    ] {
        assert!(DaemonConfig::parse(source).is_err(), "accepted: {source}");
    }
}

#[test]
fn rejects_the_obsolete_llama_backend_directory_setting() {
    let config = format!("llama_backend_directory = \"/obsolete/backend\"\n{CONFIG}");
    let error = DaemonConfig::parse(&config).unwrap_err();
    let DaemonConfigError::Parse(error) = error else {
        panic!("the obsolete setting must be rejected as an unknown field: {error}");
    };

    assert!(
        error
            .to_string()
            .contains("unknown field `llama_backend_directory`"),
        "unexpected config error: {error}"
    );
}
