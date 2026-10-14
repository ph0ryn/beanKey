use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use beankey_daemon::protocol::envelope::Payload;
use beankey_daemon::{
    DaemonServer, Engine, PROTOCOL_VERSION, protocol, read_envelope, write_envelope,
};
use tempfile::TempDir;

fn dictionary_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("BEANKEY_TEST_DICTIONARY")
            .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
    )
}

fn envelope(request_id: u64, payload: Payload) -> protocol::Envelope {
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        session_id: "shared-external-id".into(),
        payload: Some(payload),
        trace: Vec::new(),
    }
}

fn request(stream: &mut UnixStream, envelope: protocol::Envelope) -> protocol::StateResponse {
    write_envelope(stream, &envelope).unwrap();
    match read_envelope(stream).unwrap().payload.unwrap() {
        Payload::StateResponse(response) => response,
        Payload::ProtocolError(error) => panic!("protocol error: {}", error.message),
        _ => panic!("unexpected daemon response"),
    }
}

#[test]
fn serves_isolated_connections_and_exits_after_the_last_disconnect() {
    let runtime = TempDir::new().unwrap();
    let socket = runtime.path().join("beankey/daemon.sock");
    let server = DaemonServer::bind(
        Engine::open(dictionary_root()).unwrap(),
        runtime.path(),
        "beankey/daemon.sock",
    )
    .unwrap();
    let worker = thread::spawn(move || server.run().unwrap());
    let mut first = UnixStream::connect(&socket).unwrap();
    let mut second = UnixStream::connect(&socket).unwrap();

    for stream in [&mut first, &mut second] {
        request(
            stream,
            envelope(
                1,
                Payload::StartSession(protocol::StartSession {
                    input_style: protocol::InputStyle::Direct as i32,
                    surrounding_text: None,
                    keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                    custom_input_table: String::new(),
                }),
            ),
        );
    }
    let first_state = request(
        &mut first,
        envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "あ".into(),
                ..Default::default()
            }),
        ),
    );
    let second_state = request(
        &mut second,
        envelope(
            2,
            Payload::KeyEvent(protocol::KeyEvent {
                action: protocol::UserAction::Input as i32,
                text: "い".into(),
                ..Default::default()
            }),
        ),
    );
    assert_eq!(first_state.preedit, "あ");
    assert_eq!(second_state.preedit, "い");

    drop(first);
    drop(second);
    worker.join().unwrap();
    assert!(!socket.exists());
}

#[test]
fn duplicate_launch_checks_the_lock_before_opening_resources() {
    let runtime = TempDir::new().unwrap();
    let socket = runtime.path().join("beankey/daemon.sock");
    let owner = DaemonServer::bind(
        Engine::open(dictionary_root()).unwrap(),
        runtime.path(),
        "beankey/daemon.sock",
    )
    .unwrap();
    let config = runtime.path().join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
dictionary = "{}"
model = "/missing/beankey/model.gguf"
emoji_dictionary = "/missing/beankey/emoji.txt"
llama_backend_directory = "/missing/beankey/backend"
runtime_socket = "beankey/daemon.sock"
[hunspell]
english_dictionary = "/missing/beankey/en_US"
greek_dictionary = "/missing/beankey/el_GR"
[conversion]
[zenz]
[inference]
context_size = 512
batch_size = 512
micro_batch_size = 64
flash_attention = true
"#,
            dictionary_root().display()
        ),
    )
    .unwrap();
    beankey_daemon::DaemonConfig::load(&config).unwrap();
    let learning = runtime.path().join("learning");
    let launch = || {
        Command::new(env!("CARGO_BIN_EXE_beankey-daemon"))
            .arg("--config")
            .arg(&config)
            .arg("--runtime-root")
            .arg(runtime.path())
            .arg("--learning-directory")
            .arg(&learning)
            .output()
            .unwrap()
    };
    let duplicate = launch();
    assert!(
        duplicate.status.success(),
        "duplicate daemon opened resources: {}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    assert!(!learning.exists());
    assert!(UnixStream::connect(&socket).is_ok());
    drop(owner);
    assert!(!socket.exists());

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(runtime.path().join("beankey/daemon.lock"))
        .unwrap();
    // SAFETY: flock receives this live file descriptor and retains no pointers.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let initializing_duplicate = launch();
    assert!(!initializing_duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&initializing_duplicate.stderr)
            .contains("daemon lock is held but its socket is unavailable"),
        "initializing duplicate opened resources: {}",
        String::from_utf8_lossy(&initializing_duplicate.stderr)
    );
    assert!(!learning.exists());
    assert!(!socket.exists());
}
