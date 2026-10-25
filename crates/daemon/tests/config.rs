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
