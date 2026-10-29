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
        "[lm_typo]\nenabled = true\nbeam_size = 0",
    ] {
        assert!(DaemonConfig::parse(source).is_err(), "accepted: {source}");
    }
}

#[test]
fn typo_correction_uses_the_desktop_ngram_search_defaults() {
    let config = DaemonConfig::parse("").unwrap();
    assert!(!config.lm_typo.enabled);
    assert_eq!(config.lm_typo.beam_size, 16);
    assert_eq!(config.lm_typo.top_k, 32);
    assert_eq!(config.lm_typo.n_best, 3);
}

#[test]
fn typo_correction_does_not_offer_a_zenz_backend() {
    assert!(DaemonConfig::parse("[lm_typo]\nlanguage_model = 'zenz'\n").is_err());
}

#[test]
fn bundled_ngram_correction_does_not_require_user_asset_paths() {
    let config = DaemonConfig::parse("[lm_typo]\nenabled = true\n").unwrap();
    assert!(config.lm_typo.enabled);
    assert!(config.lm_typo.ngram.is_none());
}

#[test]
fn accepts_ngram_correction_without_a_backend_selector() {
    let config = DaemonConfig::parse(
        "[lm_typo]\nenabled = true\n[lm_typo.ngram]\n\
         prefix = '/model/lm_'\ntokenizer = '/model/tokenizer.json'\nn = 5\ndiscount = 0.75\n",
    )
    .unwrap();
    assert!(config.lm_typo.enabled);
    let ngram = config.lm_typo.ngram.unwrap();
    assert_eq!(ngram.n, 5);
    assert_eq!(ngram.discount, 0.75);
    assert!(
        DaemonConfig::parse(
            "[lm_typo]\nenabled = true\n[lm_typo.ngram]\n\
         prefix = '/model/lm_'\nn = 5\ndiscount = 0.75\n"
        )
        .is_err(),
        "an external ngram must declare its tokenizer"
    );
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
