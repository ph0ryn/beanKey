use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::protocol::envelope::Payload;
use crate::protocol::protocol_error::Code;
use crate::{Engine, FrameError, PROTOCOL_VERSION, protocol, read_envelope, write_envelope};

static TERMINATING: AtomicBool = AtomicBool::new(false);
static CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    fn bind(runtime_root: &Path, relative_socket: &Path) -> Result<Self, ServerError> {
        let uid = effective_uid();
        let runtime_directory_path = runtime_root.join(
            relative_socket
                .parent()
                .expect("validated socket has a parent"),
        );
        let runtime_directory = prepare_runtime_directory(&runtime_directory_path, uid)?;
        let socket_path = runtime_root.join(relative_socket);
        let lock_path = runtime_directory_path.join("daemon.lock");
        let lock = open_lock(&lock_path, uid)?;
        acquire_lock(&lock, &socket_path)?;
        remove_stale_socket(&socket_path, uid)?;

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

pub struct DaemonServer {
    endpoint: RuntimeEndpoint,
    engine: Arc<Mutex<Engine>>,
    uid: u32,
}

impl DaemonServer {
    pub fn bind(
        engine: Engine,
        runtime_root: impl AsRef<Path>,
        relative_socket: impl AsRef<Path>,
    ) -> Result<Self, ServerError> {
        if relative_socket.as_ref() != Path::new("beankey/daemon.sock") {
            return Err(ServerError::InvalidSocketPath);
        }
        Ok(Self {
            endpoint: RuntimeEndpoint::bind(runtime_root.as_ref(), relative_socket.as_ref())?,
            engine: Arc::new(Mutex::new(engine)),
            uid: effective_uid(),
        })
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
                    let engine = Arc::clone(&self.engine);
                    let clients = Arc::clone(&active_clients);
                    let connection = CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                    workers.push(thread::spawn(move || {
                        let _active_client = ActiveClient(clients);
                        serve_connection(stream, engine, connection);
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

fn serve_connection(mut stream: UnixStream, engine: Arc<Mutex<Engine>>, connection: u64) {
    if stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return;
    }
    let mut sessions = HashSet::new();
    while !TERMINATING.load(Ordering::Acquire) {
        match wait_for_readable(&stream, Duration::from_millis(100)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => break,
        }
        let mut request = match read_envelope(&mut stream) {
            Ok(request) => request,
            Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                let response = frame_error_envelope(error);
                let _ = write_envelope(&mut stream, &response);
                break;
            }
        };
        let external_session = request.session_id.clone();
        let internal_session = format!("{connection}:{external_session}");
        let starts = matches!(request.payload, Some(Payload::StartSession(_)));
        let ends = matches!(request.payload, Some(Payload::EndSession(_)));
        request.session_id.clone_from(&internal_session);
        let mut response = engine
            .lock()
            .expect("a poisoned engine cannot preserve conversion state")
            .handle(request);
        let succeeded = matches!(response.payload, Some(Payload::StateResponse(_)));
        response.session_id = external_session;
        if write_envelope(&mut stream, &response).is_err() {
            break;
        }
        if succeeded && starts {
            sessions.insert(internal_session.clone());
        }
        if succeeded && ends {
            sessions.remove(&internal_session);
        }
    }
    let mut engine = engine
        .lock()
        .expect("a poisoned engine cannot preserve conversion state");
    for session in sessions {
        engine.remove_session(&session);
    }
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

    #[test]
    fn replaces_only_an_owned_stale_socket_and_sets_private_modes() {
        let root = TempDir::new().unwrap();
        let runtime = root.path().join("beankey");
        fs::create_dir(&runtime).unwrap();
        let stale_path = runtime.join("daemon.sock");
        drop(UnixListener::bind(&stale_path).unwrap());

        let endpoint =
            RuntimeEndpoint::bind(root.path(), Path::new("beankey/daemon.sock")).unwrap();
        let directory = fs::metadata(&runtime).unwrap();
        let socket = fs::symlink_metadata(&stale_path).unwrap();
        assert_eq!(directory.mode() & 0o777, 0o700);
        assert_eq!(socket.mode() & 0o777, 0o600);
        assert!(matches!(
            RuntimeEndpoint::bind(root.path(), Path::new("beankey/daemon.sock")),
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
            RuntimeEndpoint::bind(root.path(), Path::new("beankey/daemon.sock")),
            Err(ServerError::UnsafeSocket)
        ));
        assert!(runtime.join("daemon.sock").is_file());
    }

    #[test]
    fn rejects_socket_paths_outside_the_canonical_runtime_location() {
        let root = TempDir::new().unwrap();
        let engine = Engine::open(PathBuf::from(
            std::env::var_os("BEANKEY_TEST_DICTIONARY")
                .expect("BEANKEY_TEST_DICTIONARY must be set by the Nix test environment"),
        ))
        .unwrap();
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
