use std::path::Path;

use beankey_daemon::{DaemonConfig, DaemonConfigError};

const CONFIG: &str = r#"
dictionary = "/nix/store/dictionary"
model = "/nix/store/model.gguf"
emoji_dictionary = "/nix/store/emoji_all_E17.0.txt"
runtime_socket = "beankey/daemon.sock"

[hunspell]
english_dictionary = "/nix/store/en_US"
greek_dictionary = "/nix/store/el_GR"

[conversion]
[zenz]
[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#;

#[test]
fn parses_model_configuration_without_an_external_backend_directory() {
    let config = DaemonConfig::parse(CONFIG).unwrap();

    assert_eq!(config.model, Path::new("/nix/store/model.gguf"));
    assert_eq!(config.runtime_socket, Path::new("beankey/daemon.sock"));
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
