use std::fs;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn summary(path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
        .args(["--metrics-summary", "--learning-directory"])
        .arg(path)
        .output()
        .unwrap()
}

#[test]
fn summary_is_read_only_and_needs_no_daemon_configuration() {
    let directory = TempDir::new().unwrap();
    let missing = directory.path().join("not-created");
    let output = summary(&missing);
    assert!(output.status.success(), "{:?}", output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["coverage"]["complete_events"], 0);
    assert!(!missing.exists());

    let journal = directory.path().join("metrics-v1");
    fs::create_dir(&journal).unwrap();
    let path = journal.join("events-00000000000000000001.jsonl");
    let bytes = b"private-corrupt-line\n{\"incomplete\":true}";
    fs::write(&path, bytes).unwrap();
    let output = summary(directory.path());
    assert!(output.status.success(), "{:?}", output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["coverage"]["corrupt_lines"], 1);
    assert_eq!(value["coverage"]["incomplete_tails"], 1);
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("private-corrupt-line")
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(!journal.join("journal.lock").exists());
}

#[test]
fn summary_help_and_argument_errors_do_not_start_a_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--metrics-summary"));
    assert!(help.contains("never deleted"));
    for arguments in [
        vec!["--metrics-summary"],
        vec!["--metrics-summary", "--learning-directory", "relative"],
        vec![
            "--metrics-summary",
            "--learning-directory",
            "/unused",
            "extra",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
