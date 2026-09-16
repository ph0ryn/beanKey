use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
