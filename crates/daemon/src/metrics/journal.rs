//! A best-effort, bounded-memory JSONL sink. History is never deleted or truncated.
//!
//! Files are `events-<20 zero-padded decimal digits>.jsonl`, numbered from one.
//! Each open starts a fresh segment; closed segments (including interrupted tails)
//! are immutable. Readers must count a nonempty unterminated tail as incomplete,
//! not parse it as a complete record. Health counters cover one shared writer's
//! lifetime; the supplied record owns any run/session identifiers.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use serde::{Deserialize, Serialize};

// Bounds apply before enqueueing: at most 4 MiB of serialized records queued.
// The fixed health envelope adds at most 128 bytes to each persisted JSONL line.
const MAX_RECORD_BYTES: usize = 16 * 1024;
const QUEUE_CAPACITY: usize = 256;
const MAX_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
const WARNING_INTERVAL_SECONDS: u64 = 30;
const LOCK_FILE: &str = "journal.lock";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Health {
    pub(super) dropped_records: u64,
    pub(super) write_errors: u64,
}

#[derive(Default)]
struct Counters {
    dropped_records: AtomicU64,
    write_errors: AtomicU64,
    last_warning: AtomicU64,
}

impl Counters {
    fn health(&self) -> Health {
        Health {
            dropped_records: self.dropped_records.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
        }
    }

    // Only the writer (or its joining Drop) touches stderr, never record().
    fn report(&self, started: Instant, error: Option<&io::Error>) {
        let health = self.health();
        if health.dropped_records == 0 && health.write_errors == 0 {
            return;
        }
        let now = started.elapsed().as_secs().saturating_add(1);
        let previous = self.last_warning.load(Ordering::Relaxed);
        if previous != 0 && now.saturating_sub(previous) < WARNING_INTERVAL_SECONDS {
            return;
        }
        if self
            .last_warning
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let mut stderr = io::stderr().lock();
            let _ = write!(
                stderr,
                "beanKey metrics journal: dropped_records={}, write_errors={}",
                health.dropped_records, health.write_errors
            );
            if let Some(error) = error {
                let _ = write!(stderr, ": {error}");
            }
            let _ = writeln!(stderr);
        }
    }
}

enum Command {
    Record(Vec<u8>),
    Flush(SyncSender<io::Result<()>>),
    Report,
}

struct Inner {
    sender: Option<SyncSender<Command>>,
    worker: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
    started: Instant,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the only sender makes the worker drain all queued commands.
        self.sender.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            self.counters.write_errors.fetch_add(1, Ordering::Relaxed);
            self.counters.report(
                self.started,
                Some(&io::Error::other("metrics writer panicked")),
            );
        }
    }
}

#[derive(Clone)]
pub(super) struct Journal(Arc<Inner>);

impl Journal {
    pub(super) fn open(directory: PathBuf) -> io::Result<Self> {
        Self::open_with_limit(directory, MAX_SEGMENT_BYTES)
    }

    fn open_with_limit(path: PathBuf, segment_bytes: u64) -> io::Result<Self> {
        let directory = prepare_directory(&path)?;
        let lock = open_at(&directory, LOCK_FILE, libc::O_RDWR | libc::O_CREAT)?;
        let metadata = lock.metadata()?;
        // A hard-linked lock is not ours to chmod/use as a process lock either.
        if !metadata.is_file() || metadata.uid() != effective_uid() || metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "journal lock is not an owned regular file",
            ));
        }
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
        // SAFETY: flock uses a valid open descriptor, retaining no Rust pointers.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Scan just the maximum number, not an in-memory history of all segments.
        // Occupied symlink/directory names reserve their number but are never opened.
        let mut sequence = 0;
        for entry in fs::read_dir(path)? {
            if let Some(number) = segment_number(&entry?.file_name()) {
                sequence = sequence.max(number);
            }
        }
        let mut writer = Writer {
            directory,
            _lock: lock,
            file: None,
            sequence,
            bytes: 0,
            segment_bytes,
        };
        writer.create_segment()?;
        let counters = Arc::new(Counters::default());
        let worker_counters = Arc::clone(&counters);
        let started = Instant::now();
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker = thread::Builder::new()
            .name("beankey-metrics".into())
            .spawn(move || writer.run(receiver, worker_counters, started))?;
        Ok(Self(Arc::new(Inner {
            sender: Some(sender),
            worker: Some(worker),
            counters,
            started,
        })))
    }

    /// Serialization is bounded and enqueueing never waits for disk or the worker.
    pub(super) fn record<T: Serialize>(&self, record: &T) {
        let sender = self
            .0
            .sender
            .as_ref()
            .expect("live journal owns its sender");
        let mut buffer = BoundedRecord(Vec::new());
        if serde_json::to_writer(&mut buffer, record).is_err() {
            self.0
                .counters
                .dropped_records
                .fetch_add(1, Ordering::Relaxed);
            let _ = sender.try_send(Command::Report);
            return;
        }
        if let Err(error) = sender.try_send(Command::Record(buffer.0)) {
            self.0
                .counters
                .dropped_records
                .fetch_add(1, Ordering::Relaxed);
            if matches!(error, TrySendError::Disconnected(_)) {
                self.0.counters.write_errors.fetch_add(1, Ordering::Relaxed);
            }
            let _ = sender.try_send(Command::Report);
        }
    }

    /// Blocking durability/error barrier for tests and explicit callers only.
    /// Reports the first I/O error since the preceding barrier even after recovery.
    pub(super) fn flush(&self) -> io::Result<()> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.0
            .sender
            .as_ref()
            .expect("live journal owns its sender")
            .send(Command::Flush(sender))
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "metrics writer disconnected")
            })?;
        receiver.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "metrics flush barrier disconnected",
            )
        })?
    }

    pub(super) fn health(&self) -> Health {
        self.0.counters.health()
    }
}

struct BoundedRecord(Vec<u8>);

impl Write for BoundedRecord {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RECORD_BYTES - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metrics record exceeds 16 KiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Writer {
    directory: File,
    _lock: File,
    file: Option<File>,
    sequence: u64,
    bytes: u64,
    segment_bytes: u64,
}

impl Writer {
    fn create_segment(&mut self) -> io::Result<()> {
        let number = self.sequence.checked_add(1).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "journal segment number exhausted",
            )
        })?;
        let name = format!("events-{number:020}.jsonl");
        let file = open_at(
            &self.directory,
            &name,
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_EXCL,
        )?;
        // Once created, reserve the number even if chmod fails: a later record
        // must not become permanently stuck on this preserved empty file.
        self.sequence = number;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        self.file = Some(file);
        self.bytes = 0;
        Ok(())
    }

    fn append(&mut self, record: &[u8], health: &Health) -> io::Result<()> {
        let mut line = Vec::with_capacity(record.len() + 128);
        line.extend_from_slice(b"{\"journal\":");
        serde_json::to_writer(&mut line, health).map_err(io::Error::other)?;
        line.extend_from_slice(b",\"record\":");
        line.extend_from_slice(record);
        line.extend_from_slice(b"}\n");
        let length = line.len() as u64;
        if length > self.segment_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metrics line exceeds segment size",
            ));
        }
        if self.file.is_none() || self.bytes + length > self.segment_bytes {
            if let Some(file) = self.file.take() {
                // Sync closed segments on rotation, never on each input event.
                file.sync_data()?;
            }
            self.create_segment()?;
        }
        self.file
            .as_mut()
            .expect("segment opened before append")
            .write_all(&line)?;
        self.bytes += length;
        Ok(())
    }

    fn sync(&self) -> io::Result<()> {
        if let Some(file) = &self.file {
            file.sync_data()?;
        }
        self.directory.sync_all()
    }

    fn run(mut self, receiver: Receiver<Command>, counters: Arc<Counters>, started: Instant) {
        let mut pending_error = None;
        while let Ok(command) = receiver.recv() {
            match command {
                Command::Record(record) => {
                    if let Err(error) = self.append(&record, &counters.health()) {
                        // Never retry this record or append after a possible partial
                        // write. Preserve every existing byte and use a new segment.
                        self.file.take();
                        counters.dropped_records.fetch_add(1, Ordering::Relaxed);
                        counters.write_errors.fetch_add(1, Ordering::Relaxed);
                        pending_error.get_or_insert(error);
                    }
                }
                Command::Flush(reply) => {
                    if let Err(error) = self.sync() {
                        self.file.take();
                        counters.write_errors.fetch_add(1, Ordering::Relaxed);
                        pending_error.get_or_insert(error);
                    }
                    counters.report(started, pending_error.as_ref());
                    let _ = reply.send(pending_error.take().map_or(Ok(()), Err));
                }
                Command::Report => {}
            }
            counters.report(started, pending_error.as_ref());
        }
        if let Err(error) = self.sync() {
            counters.write_errors.fetch_add(1, Ordering::Relaxed);
            pending_error.get_or_insert(error);
        }
        counters.report(started, pending_error.as_ref());
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no pointers and has no failure mode.
    unsafe { libc::geteuid() }
}

fn prepare_directory(path: &Path) -> io::Result<File> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if directory.metadata()?.uid() != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "journal directory is not owned by the current user",
        ));
    }
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

fn open_at(directory: &File, name: &str, flags: libc::c_int) -> io::Result<File> {
    let name = CString::new(name).expect("journal filenames contain no NUL");
    // SAFETY: the directory descriptor and NUL-terminated name remain valid for
    // openat. O_NOFOLLOW rejects log/lock symlinks; O_EXCL protects new segments.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600 as libc::c_uint,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat returned a fresh, exclusively owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn segment_number(name: &std::ffi::OsStr) -> Option<u64> {
    let digits = name
        .to_str()?
        .strip_prefix("events-")?
        .strip_suffix(".jsonl")?;
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok().filter(|number| *number != 0)
}

/// Strict, oldest-first discovery, with no age or total-size cutoff. Returned
/// paths must still be opened with O_NOFOLLOW by readers (discovery is not a lock).
pub(super) fn segment_paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if segment_number(&entry.file_name()).is_some() && entry.file_type()?.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use serde::Serialize;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    fn lines(directory: &Path) -> Vec<Value> {
        segment_paths(directory)
            .unwrap()
            .into_iter()
            .flat_map(|path| {
                let bytes = fs::read(path).unwrap();
                assert!(bytes.is_empty() || bytes.ends_with(b"\n"));
                bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_slice(line).unwrap())
                    .collect::<Vec<Value>>()
            })
            .collect()
    }

    fn segment(directory: &Path, number: u64) -> PathBuf {
        directory.join(format!("events-{number:020}.jsonl"))
    }

    #[test]
    fn appends_across_clones_reopen_and_last_drop() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("metrics");
        let journal = Journal::open(directory.clone()).unwrap();
        let clone = journal.clone();
        journal.record(&json!({"sequence": 1}));
        journal.flush().unwrap();
        let first_path = segment_paths(&directory).unwrap().remove(0);
        let first_bytes = fs::read(&first_path).unwrap();
        drop(journal);
        clone.record(&json!({"sequence": 2}));
        drop(clone);
        let closed_bytes = fs::read(&first_path).unwrap();
        assert!(closed_bytes.starts_with(&first_bytes));

        let journal = Journal::open(directory.clone()).unwrap();
        journal.record(&json!({"sequence": 3}));
        journal.flush().unwrap();
        drop(journal);

        let records = lines(&directory);
        assert_eq!(records.len(), 3);
        for (index, line) in records.iter().enumerate() {
            assert_eq!(line["record"]["sequence"], index + 1);
            assert_eq!(line["journal"]["dropped_records"], 0);
            assert_eq!(line["journal"]["write_errors"], 0);
        }
        assert_eq!(fs::read(first_path).unwrap(), closed_bytes);
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        for path in segment_paths(&directory).unwrap() {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn rotation_bounds_each_segment_but_never_evicts_history() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("metrics");
        let journal = Journal::open_with_limit(directory.clone(), 128).unwrap();
        fs::write(directory.join("unrelated.jsonl"), b"unrelated history").unwrap();
        for sequence in 0..100 {
            journal.record(&json!({"sequence": sequence}));
            journal.flush().unwrap();
        }
        drop(journal);

        let paths = segment_paths(&directory).unwrap();
        assert!(paths.len() > 32, "old segments must not be evicted");
        for path in paths {
            assert!(fs::metadata(path).unwrap().len() <= 128);
        }
        let records = lines(&directory);
        assert_eq!(records.len(), 100);
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record["record"]["sequence"], index);
        }
        assert_eq!(
            fs::read(directory.join("unrelated.jsonl")).unwrap(),
            b"unrelated history"
        );
    }

    #[test]
    fn oversize_and_serialization_losses_are_persisted_on_later_success() {
        struct InvalidRecord;
        impl Serialize for InvalidRecord {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("deliberately invalid record"))
            }
        }

        let temporary = TempDir::new().unwrap();
        let journal = Journal::open(temporary.path().to_owned()).unwrap();
        journal.record(&"x".repeat(MAX_RECORD_BYTES));
        journal.record(&InvalidRecord);
        assert_eq!(journal.health().dropped_records, 2);
        assert_eq!(journal.health().write_errors, 0);
        journal.record(&json!({"sequence": 1}));
        journal.flush().unwrap();

        let records = lines(temporary.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["journal"]["dropped_records"], 2);
        assert_eq!(records[0]["journal"]["write_errors"], 0);
    }

    #[test]
    fn accepts_exactly_sixteen_kib_of_serialized_record_but_not_one_byte_more() {
        let temporary = TempDir::new().unwrap();
        let journal = Journal::open(temporary.path().to_owned()).unwrap();
        // A JSON string adds two quote bytes to these unescaped ASCII contents.
        journal.record(&"x".repeat(MAX_RECORD_BYTES - 2));
        journal.flush().unwrap();
        assert_eq!(journal.health().dropped_records, 0);
        journal.record(&"x".repeat(MAX_RECORD_BYTES - 1));
        journal.record(&json!({"sequence": 2}));
        journal.flush().unwrap();

        let records = lines(temporary.path());
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0]["record"].as_str().unwrap().len(),
            MAX_RECORD_BYTES - 2
        );
        assert_eq!(records[1]["journal"]["dropped_records"], 1);
        assert_eq!(records[1]["journal"]["write_errors"], 0);
    }

    #[test]
    fn a_write_failure_is_visible_and_later_records_recover_without_retrying_loss() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path();
        let journal = Journal::open_with_limit(directory.to_owned(), 128).unwrap();
        journal.record(&json!({"sequence": 1}));
        journal.flush().unwrap();
        let blocked = segment(directory, 2);
        fs::create_dir(&blocked).unwrap();
        journal.record(&json!({"sequence": 2}));
        assert!(journal.flush().is_err());
        assert_eq!(journal.health().dropped_records, 1);
        assert_eq!(journal.health().write_errors, 1);
        assert_eq!(lines(directory).len(), 1);

        assert_eq!(blocked, directory.join("events-00000000000000000002.jsonl"));
        fs::remove_dir(&blocked).unwrap();
        journal.record(&json!({"sequence": 3}));
        journal.flush().unwrap();
        let records = lines(directory);
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["record"]["sequence"], 3);
        assert_eq!(records[1]["journal"]["dropped_records"], 1);
        assert_eq!(records[1]["journal"]["write_errors"], 1);
    }

    #[test]
    fn refuses_symlink_lock_directory_and_rotation_targets() {
        let temporary = TempDir::new().unwrap();
        let victim = temporary.path().join("victim");
        fs::write(&victim, b"do not change").unwrap();
        let locked_directory = temporary.path().join("locked");
        fs::create_dir(&locked_directory).unwrap();
        symlink(&victim, locked_directory.join("journal.lock")).unwrap();
        assert!(Journal::open(locked_directory).is_err());

        let linked_directory = temporary.path().join("linked");
        symlink(temporary.path(), &linked_directory).unwrap();
        assert!(Journal::open(linked_directory).is_err());

        let directory = temporary.path().join("metrics");
        let journal = Journal::open_with_limit(directory.clone(), 128).unwrap();
        journal.record(&json!({"sequence": 1}));
        journal.flush().unwrap();
        symlink(&victim, segment(&directory, 2)).unwrap();
        journal.record(&json!({"sequence": 2}));
        assert!(journal.flush().is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"do not change");
        assert_eq!(segment_paths(&directory).unwrap().len(), 1);
    }

    #[test]
    fn an_interrupted_tail_is_preserved_and_new_lines_start_a_new_segment() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path();
        let interrupted = segment(directory, 1);
        let old_bytes = b"{\"journal\":{\"dropped_records\":0,\"write_errors\":0},\"record\":{\"sequence\":1}}\n{\"journal\":";
        fs::write(&interrupted, old_bytes).unwrap();
        let journal = Journal::open(directory.to_owned()).unwrap();
        journal.record(&json!({"sequence": 2}));
        journal.flush().unwrap();
        drop(journal);

        assert_eq!(fs::read(&interrupted).unwrap(), old_bytes);
        let paths = segment_paths(directory).unwrap();
        assert_eq!(paths, vec![interrupted, segment(directory, 2)]);
        let new_bytes = fs::read(&paths[1]).unwrap();
        assert!(new_bytes.ends_with(b"\n"));
        let record: Value = serde_json::from_slice(&new_bytes).unwrap();
        assert_eq!(record["record"]["sequence"], 2);
    }

    #[test]
    fn paths_are_strict_sorted_regular_files_and_missing_directory_is_empty() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path();
        assert!(
            segment_paths(&directory.join("missing"))
                .unwrap()
                .is_empty()
        );
        fs::write(segment(directory, 12), b"{}").unwrap();
        fs::write(segment(directory, 2), b"{}").unwrap();
        for name in [
            "events.jsonl",
            "events-2.jsonl",
            "events-00000000000000000000.jsonl",
            "events-99999999999999999999.jsonl",
            "events-00000000000000000001.jsonl.backup",
        ] {
            fs::write(directory.join(name), b"unrelated").unwrap();
        }
        fs::create_dir(segment(directory, 3)).unwrap();
        symlink(segment(directory, 2), segment(directory, 4)).unwrap();
        assert_eq!(
            segment_paths(directory).unwrap(),
            vec![segment(directory, 2), segment(directory, 12)]
        );
    }

    #[test]
    fn only_one_writer_may_open_the_directory_until_the_last_clone_is_dropped() {
        let temporary = TempDir::new().unwrap();
        let journal = Journal::open(temporary.path().to_owned()).unwrap();
        let clone = journal.clone();
        assert!(Journal::open(temporary.path().to_owned()).is_err());
        drop(journal);
        assert!(Journal::open(temporary.path().to_owned()).is_err());
        clone.record(&json!({"sequence": 1}));
        drop(clone);
        let reopened = Journal::open(temporary.path().to_owned()).unwrap();
        reopened.flush().unwrap();
        assert_eq!(lines(temporary.path()).len(), 1);
        assert_eq!(
            fs::metadata(temporary.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
