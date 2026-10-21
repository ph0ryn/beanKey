use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::protocol::envelope::Payload;
use crate::protocol::protocol_error::Code;
use crate::{Engine, FrameError, PROTOCOL_VERSION, protocol, read_envelope, write_envelope};

static TERMINATING: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub enum ServerError {
    RuntimeDirectory(io::Error),
    UnsafeRuntimeDirectory,
    Lock(io::Error),
    LockBusy,
    AlreadyRunning,
    InvalidSocketPath,
    UnsafeSocket,
    Socket(io::Error),
    UnsupportedPlatform,
    WorkerPanicked,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeDirectory(error) => {
                write!(formatter, "could not prepare runtime directory: {error}")
            }
            Self::UnsafeRuntimeDirectory => write!(
                formatter,
                "runtime directory must be an owned directory with mode 0700"
            ),
            Self::Lock(error) => write!(formatter, "could not acquire daemon lock: {error}"),
            Self::LockBusy => write!(
                formatter,
                "daemon lock is held but its socket is unavailable"
            ),
            Self::AlreadyRunning => write!(formatter, "a beanKey daemon is already running"),
            Self::InvalidSocketPath => write!(formatter, "socket path must be beankey/daemon.sock"),
            Self::UnsafeSocket => write!(
                formatter,
                "existing socket path is not an owned Unix domain socket"
            ),
            Self::Socket(error) => write!(formatter, "daemon socket failed: {error}"),
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "peer credential verification is unsupported on this platform"
                )
            }
            Self::WorkerPanicked => write!(formatter, "a daemon connection worker panicked"),
        }
    }
}

impl Error for ServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RuntimeDirectory(error) | Self::Lock(error) | Self::Socket(error) => Some(error),
            _ => None,
        }
    }
}

struct RuntimeEndpoint {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
    _runtime_directory: File,
    _lock: File,
}

impl RuntimeEndpoint {
    fn bind(reservation: DaemonServerReservation) -> Result<Self, ServerError> {
        let DaemonServerReservation {
            socket_path,
            runtime_directory,
            lock,
            uid,
        } = reservation;

        let listener = UnixListener::bind(&socket_path).map_err(ServerError::Socket)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(ServerError::Socket)?;
        let metadata = fs::symlink_metadata(&socket_path).map_err(ServerError::Socket)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != uid
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(ServerError::UnsafeSocket);
        }
        listener
            .set_nonblocking(true)
            .map_err(ServerError::Socket)?;
        Ok(Self {
            listener,
            socket_path,
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
            _runtime_directory: runtime_directory,
            _lock: lock,
        })
    }
}

impl Drop for RuntimeEndpoint {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.socket_path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.socket_device
            && metadata.ino() == self.socket_inode
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

/// Holds the process lock during resource initialization without publishing a socket.
/// Dropping an unused reservation releases the lock.
pub struct DaemonServerReservation {
    socket_path: PathBuf,
    runtime_directory: File,
    lock: File,
    uid: u32,
}

impl DaemonServerReservation {
    /// Publishes the listener only once the engine is ready, retaining the same lock.
    pub fn bind(self, engine: Engine) -> Result<DaemonServer, ServerError> {
        let uid = self.uid;
        Ok(DaemonServer {
            endpoint: RuntimeEndpoint::bind(self)?,
            engine,
            uid,
        })
    }
}

pub struct DaemonServer {
    endpoint: RuntimeEndpoint,
    engine: Engine,
    uid: u32,
}

impl DaemonServer {
    pub fn reserve(
        runtime_root: impl AsRef<Path>,
        relative_socket: impl AsRef<Path>,
    ) -> Result<DaemonServerReservation, ServerError> {
        let relative_socket = relative_socket.as_ref();
        if relative_socket != Path::new("beankey/daemon.sock") {
            return Err(ServerError::InvalidSocketPath);
        }
        let uid = effective_uid();
        let runtime_directory_path = runtime_root.as_ref().join(
            relative_socket
                .parent()
                .expect("validated socket has a parent"),
        );
        let runtime_directory = prepare_runtime_directory(&runtime_directory_path, uid)?;
        let socket_path = runtime_root.as_ref().join(relative_socket);
        let lock = open_lock(&runtime_directory_path.join("daemon.lock"), uid)?;
        acquire_lock(&lock, &socket_path)?;
        remove_stale_socket(&socket_path, uid)?;
        Ok(DaemonServerReservation {
            socket_path,
            runtime_directory,
            lock,
            uid,
        })
    }

    pub fn bind(
        engine: Engine,
        runtime_root: impl AsRef<Path>,
        relative_socket: impl AsRef<Path>,
    ) -> Result<Self, ServerError> {
        Self::reserve(runtime_root, relative_socket)?.bind(engine)
    }

    pub fn run(self) -> Result<(), ServerError> {
        install_signal_handlers();
        let active_clients = Arc::new(AtomicUsize::new(0));
        let mut accepted_client = false;
        let mut workers = Vec::new();

        while !TERMINATING.load(Ordering::Acquire) {
            reap_workers(&mut workers)?;
            match self.endpoint.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).map_err(ServerError::Socket)?;
                    if peer_uid(&stream)? != self.uid {
                        continue;
                    }
                    accepted_client = true;
                    active_clients.fetch_add(1, Ordering::AcqRel);
                    let engine = self.engine.for_connection();
                    let clients = Arc::clone(&active_clients);
                    workers.push(thread::spawn(move || {
                        let _active_client = ActiveClient(clients);
                        serve_connection(stream, engine);
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if accepted_client && active_clients.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(ServerError::Socket(error)),
            }
        }

        for worker in workers {
            worker.join().map_err(|_| ServerError::WorkerPanicked)?;
        }
        Ok(())
    }
}

struct ActiveClient(Arc<AtomicUsize>);

impl Drop for ActiveClient {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reap_workers(workers: &mut Vec<thread::JoinHandle<()>>) -> Result<(), ServerError> {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            workers
                .swap_remove(index)
                .join()
                .map_err(|_| ServerError::WorkerPanicked)?;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn prepare_runtime_directory(path: &Path, uid: u32) -> Result<File, ServerError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            if let Err(error) = builder.create(path)
                && error.kind() != io::ErrorKind::AlreadyExists
            {
                return Err(ServerError::RuntimeDirectory(error));
            }
        }
        Err(error) => return Err(ServerError::RuntimeDirectory(error)),
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(ServerError::RuntimeDirectory)?;
    let metadata = directory
        .metadata()
        .map_err(ServerError::RuntimeDirectory)?;
    if !metadata.is_dir() || metadata.uid() != uid {
        return Err(ServerError::UnsafeRuntimeDirectory);
    }
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(ServerError::RuntimeDirectory)?;
    let metadata = directory
        .metadata()
        .map_err(ServerError::RuntimeDirectory)?;
    if metadata.mode() & 0o777 != 0o700 {
        return Err(ServerError::UnsafeRuntimeDirectory);
    }
    Ok(directory)
}

fn open_lock(path: &Path, uid: u32) -> Result<File, ServerError> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(ServerError::Lock)?;
    let metadata = lock.metadata().map_err(ServerError::Lock)?;
    if !metadata.is_file() || metadata.uid() != uid {
        return Err(ServerError::Lock(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "lock is not an owned regular file",
        )));
    }
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(ServerError::Lock)?;
    Ok(lock)
}

fn acquire_lock(lock: &File, socket_path: &Path) -> Result<(), ServerError> {
    // SAFETY: flock only inspects the valid descriptor and does not retain pointers.
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return if UnixStream::connect(socket_path).is_ok() {
            Err(ServerError::AlreadyRunning)
        } else {
            Err(ServerError::LockBusy)
        };
    }
    Err(ServerError::Lock(error))
}

fn remove_stale_socket(path: &Path, uid: u32) -> Result<(), ServerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ServerError::Socket(error)),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != uid {
        return Err(ServerError::UnsafeSocket);
    }
    if UnixStream::connect(path).is_ok() {
        return Err(ServerError::AlreadyRunning);
    }
    fs::remove_file(path).map_err(ServerError::Socket)
}

fn serve_connection(mut stream: UnixStream, mut engine: Engine) {
    if stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return;
    }
    while !TERMINATING.load(Ordering::Acquire) {
        match wait_for_readable(&stream, Duration::from_millis(100)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => break,
        }
        let request = match read_envelope(&mut stream) {
            Ok(request) => request,
            Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                let response = frame_error_envelope(error);
                let _ = write_envelope(&mut stream, &response);
                break;
            }
        };
        let response = engine.handle(request);
        if write_envelope(&mut stream, &response).is_err() {
            break;
        }
    }
    // Dropping this exclusively owned registry disposes every session, including
    // a successful StartSession whose response could not be written.
}

fn wait_for_readable(stream: &UnixStream, timeout: Duration) -> io::Result<bool> {
    let timeout = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let mut descriptor = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: poll receives one initialized descriptor for the duration of the call.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout) };
        if result > 0 {
            return Ok(true);
        }
        if result == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn frame_error_envelope(error: FrameError) -> protocol::Envelope {
    let code = match error {
        FrameError::UnsupportedVersion(_) => Code::UnsupportedVersion,
        FrameError::MessageTooLarge(_) => Code::MessageTooLarge,
        _ => Code::InvalidPayload,
    };
    protocol::Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: 0,
        session_id: String::new(),
        payload: Some(Payload::ProtocolError(protocol::ProtocolError {
            code: code as i32,
            message: error.to_string(),
        })),
        trace: Vec::new(),
    }
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Result<u32, ServerError> {
    // SAFETY: The kernel initializes the fixed-size credential structure for this socket.
    unsafe {
        let mut credential: libc::ucred = std::mem::zeroed();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credential).cast(),
            &raw mut length,
        );
        if result != 0 {
            return Err(ServerError::Socket(io::Error::last_os_error()));
        }
        Ok(credential.uid)
    }
}

#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> Result<u32, ServerError> {
    // SAFETY: getpeereid writes the effective peer IDs to the two valid output pointers.
    unsafe {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        if libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) != 0 {
            return Err(ServerError::Socket(io::Error::last_os_error()));
        }
        Ok(uid)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_uid(_stream: &UnixStream) -> Result<u32, ServerError> {
    Err(ServerError::UnsupportedPlatform)
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and has no failure mode.
    unsafe { libc::geteuid() }
}

extern "C" fn request_termination(_signal: libc::c_int) {
    TERMINATING.store(true, Ordering::Release);
}

fn install_signal_handlers() {
    // SAFETY: The handler only performs a lock-free atomic store, and this is a standalone process.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            request_termination as *const () as libc::sighandler_t,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use tempfile::TempDir;

    use super::*;

    fn dictionary_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../converter/assets/dictionary/azookeyKanaKanji/Dictionary")
    }

    struct BlockingModel {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        blocked_once: bool,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for BlockingModel {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl beankey_converter::ZenzLanguageModel for BlockingModel {
        fn vocabulary_size(&self) -> usize {
            3
        }
        fn eos_token(&self) -> i32 {
            2
        }
        fn tokenize(
            &mut self,
            _text: &str,
            _add_special: bool,
        ) -> Result<Vec<i32>, beankey_converter::ZenzInferenceError> {
            Ok(vec![1])
        }
        fn token_to_piece(
            &mut self,
            _token: i32,
        ) -> Result<Vec<u8>, beankey_converter::ZenzInferenceError> {
            Ok(b"x".to_vec())
        }
        fn next_logits(
            &mut self,
            _tokens: &[i32],
        ) -> Result<Vec<f32>, beankey_converter::ZenzInferenceError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            if !self.blocked_once {
                self.blocked_once = true;
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![0.0, 10.0, 0.0])
        }
    }

    fn request(id: u64, payload: Payload) -> protocol::Envelope {
        protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: id,
            session_id: "same-external-id".into(),
            payload: Some(payload),
            trace: Vec::new(),
        }
    }

    fn start_request(id: u64) -> protocol::Envelope {
        request(
            id,
            Payload::StartSession(protocol::StartSession {
                input_style: protocol::InputStyle::Direct as i32,
                keyboard_language: protocol::KeyboardLanguage::Japanese as i32,
                ..Default::default()
            }),
        )
    }

    fn assert_state(stream: &mut UnixStream, id: u64) -> protocol::StateResponse {
        let reply = read_envelope(stream).expect("independent connection must make progress");
        assert_eq!(reply.request_id, id);
        assert_eq!(reply.session_id, "same-external-id");
        match reply.payload {
            Some(Payload::StateResponse(state)) => state,
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn blocked_model_does_not_lock_other_connections_and_keeps_request_order() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let template = Engine::open_with_zenz_model(
            dictionary_root(),
            Box::new(BlockingModel {
                entered: entered_tx,
                release: release_rx,
                blocked_once: false,
                active: Arc::clone(&active),
                peak: Arc::clone(&peak),
                drops: Arc::new(AtomicUsize::new(0)),
            }),
        )
        .unwrap();
        let (mut a, server_a) = UnixStream::pair().unwrap();
        let (mut b, server_b) = UnixStream::pair().unwrap();
        a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let engine_a = template.for_connection();
        let engine_b = template.for_connection();
        let worker_a = thread::spawn(move || serve_connection(server_a, engine_a));
        let worker_b = thread::spawn(move || serve_connection(server_b, engine_b));
        write_envelope(&mut a, &start_request(1)).unwrap();
        assert_state(&mut a, 1);
        write_envelope(
            &mut a,
            &request(
                2,
                Payload::KeyEvent(protocol::KeyEvent {
                    action: protocol::UserAction::Input as i32,
                    text: "はし".into(),
                    ..Default::default()
                }),
            ),
        )
        .unwrap();
        // A later request on this same session must not overtake the blocked input.
        write_envelope(
            &mut a,
            &request(3, Payload::ResetSession(protocol::ResetSession {})),
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 1);

        // Identical external IDs belong to different connection-owned registries.
        write_envelope(&mut b, &start_request(1)).unwrap();
        assert_state(&mut b, 1);
        write_envelope(
            &mut b,
            &request(2, Payload::ResetSession(protocol::ResetSession {})),
        )
        .unwrap();
        assert!(assert_state(&mut b, 2).preedit.is_empty());
        write_envelope(
            &mut b,
            &request(3, Payload::EndSession(protocol::EndSession {})),
        )
        .unwrap();
        assert_state(&mut b, 3);
        write_envelope(&mut b, &start_request(4)).unwrap();
        assert_state(&mut b, 4);
        write_envelope(
            &mut b,
            &request(
                5,
                Payload::KeyEvent(protocol::KeyEvent {
                    action: protocol::UserAction::Input as i32,
                    text: "ねこ".into(),
                    ..Default::default()
                }),
            ),
        )
        .unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(assert_state(&mut a, 2).preedit, "はし");
        assert!(assert_state(&mut a, 3).preedit.is_empty());
        assert_eq!(assert_state(&mut b, 5).preedit, "ねこ");
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "native model calls must serialize"
        );

        // Disconnect A must dispose A only, and must not change B's composition.
        drop(a);
        worker_a.join().unwrap();
        write_envelope(
            &mut b,
            &request(
                6,
                Payload::KeyEvent(protocol::KeyEvent {
                    action: protocol::UserAction::Enter as i32,
                    ..Default::default()
                }),
            ),
        )
        .unwrap();
        assert_eq!(assert_state(&mut b, 6).commit, "ねこ");
        drop(b);
        worker_b.join().unwrap();
    }

    #[test]
    fn releases_the_connection_engine_when_the_response_cannot_be_written() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (entered, _) = std::sync::mpsc::channel();
        let (_, release) = std::sync::mpsc::channel();
        let mut engine = Engine::open_with_zenz_model(
            dictionary_root(),
            Box::new(BlockingModel {
                entered,
                release,
                blocked_once: false,
                active: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                drops: Arc::clone(&drops),
            }),
        )
        .unwrap();
        let start = start_request(1);
        // Verify the same request succeeds before making its response unwritable.
        assert!(matches!(
            engine.handle(start.clone()).payload,
            Some(Payload::StateResponse(_))
        ));
        let (mut client, server) = UnixStream::pair().unwrap();
        // Keep the peer object open for Darwin's SO_RCVTIMEO, but disallow replies.
        client.shutdown(std::net::Shutdown::Read).unwrap();
        write_envelope(&mut client, &start_request(2)).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        serve_connection(server, engine);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "connection resources and sessions must be dropped"
        );
    }

    #[test]
    fn malformed_frame_closes_only_its_connection() {
        use std::io::{Read, Write};

        let template = Engine::open(dictionary_root()).unwrap();
        let (mut bad, server_bad) = UnixStream::pair().unwrap();
        let (mut healthy, server_healthy) = UnixStream::pair().unwrap();
        bad.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        healthy
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let bad_engine = template.for_connection();
        let healthy_engine = template.for_connection();
        let bad_worker = thread::spawn(move || serve_connection(server_bad, bad_engine));
        let healthy_worker =
            thread::spawn(move || serve_connection(server_healthy, healthy_engine));
        write_envelope(&mut healthy, &start_request(1)).unwrap();
        assert_state(&mut healthy, 1);
        let mut malformed = vec![0x80; 9];
        malformed.push(0x02); // The tenth byte cannot represent bits above u64.
        bad.write_all(&malformed).unwrap();
        let failure = read_envelope(&mut bad).unwrap();
        assert!(
            matches!(failure.payload, Some(Payload::ProtocolError(error))
            if error.code == Code::InvalidPayload as i32)
        );
        assert_eq!(bad.read(&mut [0]).unwrap(), 0);
        bad_worker.join().unwrap();
        write_envelope(
            &mut healthy,
            &request(2, Payload::ResetSession(protocol::ResetSession {})),
        )
        .unwrap();
        assert_state(&mut healthy, 2);
        drop(healthy);
        healthy_worker.join().unwrap();
    }

    #[test]
    fn reserves_the_lock_without_publishing_an_unready_socket() {
        let root = TempDir::new().unwrap();
        let socket = root.path().join("beankey/daemon.sock");
        let reservation = DaemonServer::reserve(root.path(), "beankey/daemon.sock").unwrap();
        assert!(!socket.exists());
        assert!(matches!(
            DaemonServer::reserve(root.path(), "beankey/daemon.sock"),
            Err(ServerError::LockBusy)
        ));
        let endpoint = RuntimeEndpoint::bind(reservation).unwrap();
        assert!(UnixStream::connect(&socket).is_ok());
        assert!(matches!(
            DaemonServer::reserve(root.path(), "beankey/daemon.sock"),
            Err(ServerError::AlreadyRunning)
        ));
        drop(endpoint);
        assert!(!socket.exists());

        let unused = DaemonServer::reserve(root.path(), "beankey/daemon.sock").unwrap();
        drop(unused);
        assert!(DaemonServer::reserve(root.path(), "beankey/daemon.sock").is_ok());
        assert!(!socket.exists());
    }

    #[test]
    fn replaces_only_an_owned_stale_socket_and_sets_private_modes() {
        let root = TempDir::new().unwrap();
        let runtime = root.path().join("beankey");
        fs::create_dir(&runtime).unwrap();
        let stale_path = runtime.join("daemon.sock");
        drop(UnixListener::bind(&stale_path).unwrap());

        let endpoint = DaemonServer::reserve(root.path(), "beankey/daemon.sock")
            .and_then(RuntimeEndpoint::bind)
            .unwrap();
        let directory = fs::metadata(&runtime).unwrap();
        let socket = fs::symlink_metadata(&stale_path).unwrap();
        assert_eq!(directory.mode() & 0o777, 0o700);
        assert_eq!(socket.mode() & 0o777, 0o600);
        assert!(matches!(
            DaemonServer::reserve(root.path(), "beankey/daemon.sock")
                .and_then(RuntimeEndpoint::bind),
            Err(ServerError::AlreadyRunning)
        ));
        drop(endpoint);
        assert!(!stale_path.exists());
    }

    #[test]
    fn refuses_to_remove_a_non_socket_runtime_path() {
        let root = TempDir::new().unwrap();
        let runtime = root.path().join("beankey");
        fs::create_dir(&runtime).unwrap();
        File::create(runtime.join("daemon.sock")).unwrap();

        assert!(matches!(
            DaemonServer::reserve(root.path(), "beankey/daemon.sock")
                .and_then(RuntimeEndpoint::bind),
            Err(ServerError::UnsafeSocket)
        ));
        assert!(runtime.join("daemon.sock").is_file());
    }

    #[test]
    fn rejects_socket_paths_outside_the_canonical_runtime_location() {
        let root = TempDir::new().unwrap();
        let engine = Engine::open(dictionary_root()).unwrap();
        assert!(matches!(
            DaemonServer::bind(engine, root.path(), "../daemon.sock"),
            Err(ServerError::InvalidSocketPath)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_uid_of_a_darwin_unix_socket_peer() {
        let (stream, _peer) = UnixStream::pair().unwrap();

        assert_eq!(peer_uid(&stream).unwrap(), effective_uid());
    }
}
