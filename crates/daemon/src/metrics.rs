//! Local, text-free diagnostic records. Saved segments are never automatically deleted.
mod build;
mod journal;
mod report;

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use beankey_converter::{MeasurementStatus, SelectionMeasurement};
use serde::{Deserialize, Serialize};

pub(crate) use build::store_id;
pub use report::{MetricsSummary, summarize_metrics};

const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RunSettings {
    pub model_store_id: Option<String>,
    pub dictionary_store_id: Option<String>,
    pub llama_store_id: Option<String>,
    pub neural: bool,
    pub upstream_ngram_personalization: bool,
    pub live_conversion: bool,
    pub neural_prediction: bool,
    pub rich_candidates: bool,
    pub inference_limit: usize,
    pub japanese_prediction: bool,
    pub foreign_prediction: bool,
    pub dictionary_typo: bool,
    pub lm_typo: bool,
    pub custom_input_table: bool,
    pub user_dictionary_entries: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    StartSession,
    EndSession,
    ResetSession,
    Input,
    Backspace,
    DeleteForward,
    Enter,
    Escape,
    Space,
    Tab,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Forget,
    Hiragana,
    Katakana,
    HalfWidthKatakana,
    FullWidthRoman,
    HalfWidthRoman,
    Consume,
    StartUnicodeInput,
    Eisu,
    Kana,
    SelectCandidate,
    PageCandidates,
    CommitComposition,
    ForgetCandidate,
    ResetLearning,
    RequestTypoCorrections,
    SelectTypoCorrection,
    Invalid,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    #[default]
    None,
    Composing,
    Previewing,
    Selecting,
    Unicode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommitOrigin {
    Candidate,
    CandidateAndRaw,
    DictionaryTypo,
    LmTypo,
    Raw,
    Unicode,
    Direct,
}

/// Typing events accept only scalar/closed-enum observations, never text or candidate scopes.
/// Run metadata is limited to code/build identifiers and non-text settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Event {
    RunStarted {
        build: build::BuildInfo,
        settings: RunSettings,
    },
    SessionStarted {
        session: u64,
    },
    SessionEnded {
        session: u64,
    },
    Request {
        session: Option<u64>,
        operation: Operation,
        shift: bool,
        option: bool,
        before: Mode,
        after: Mode,
        consumed: bool,
        error_code: Option<i32>,
        input_chars: usize,
        source_before: usize,
        source_after: usize,
        candidate_window: i32,
        candidate_count: usize,
        selected_rank: Option<usize>,
        prediction_offered: bool,
        duration_us: u64,
    },
    CandidateRequest {
        session: u64,
        mode: Mode,
        source_surface_count: usize,
        candidate_count: usize,
        rich: bool,
        segment: bool,
        context_available: bool,
        right_context_available: bool,
        neural: bool,
        capacity_fallback: bool,
        learning_error: bool,
        personalization_warning: bool,
        error_code: Option<i32>,
        duration_us: u64,
        rerank_us: u64,
    },
    Commit {
        session: Option<u64>,
        operation: Operation,
        mode: Mode,
        origin: CommitOrigin,
        accepted_chars: usize,
        candidate_chars: usize,
        duration_us: u64,
        comparison: SelectionMeasurement,
    },
    RunStopped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    pub schema: u32,
    pub run: String,
    pub sequence: u64,
    pub unix_ms: u64,
    pub elapsed_us: u64,
    pub event: Event,
}

struct Writer {
    journal: journal::Journal,
    run: String,
    started: Instant,
    sequence: AtomicU64,
    identity: AtomicU64,
    build: build::BuildInfo,
    announced: AtomicBool,
}

impl Writer {
    fn record(&self, event: Event) {
        self.journal.record(&Record {
            schema: SCHEMA,
            run: self.run.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            unix_ms: unix_ms(),
            elapsed_us: micros(self.started.elapsed()),
            event,
        });
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.record(Event::RunStopped);
        // Only final shutdown blocks on the writer, never a typing request.
        if let Err(error) = self.journal.flush() {
            eprintln!("beanKey metrics final flush failed: {error}");
        }
        let health = self.journal.health();
        if health.dropped_records != 0 || health.write_errors != 0 {
            eprintln!(
                "beanKey metrics recording loss: {} dropped records, {} write errors",
                health.dropped_records, health.write_errors
            );
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Metrics(Option<Arc<Writer>>);

impl Metrics {
    pub fn open(learning_directory: &Path) -> io::Result<Self> {
        // Random run identity is unrelated to text, application or protocol session IDs.
        use std::io::Read;
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let run = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self(Some(Arc::new(Writer {
            journal: journal::Journal::open(learning_directory.join("metrics-v1"))?,
            run,
            started: Instant::now(),
            sequence: AtomicU64::new(1),
            identity: AtomicU64::new(1),
            build: build::BuildInfo::current(),
            announced: AtomicBool::new(false),
        }))))
    }

    pub fn start(&self, settings: impl FnOnce() -> RunSettings) {
        if let Some(writer) = &self.0
            && !writer.announced.swap(true, Ordering::Relaxed)
        {
            writer.record(Event::RunStarted {
                build: writer.build.clone(),
                settings: settings(),
            });
        }
    }

    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn record(&self, event: Event) {
        if let Some(writer) = &self.0 {
            writer.record(event);
        }
    }

    pub fn identity(&self) -> u64 {
        self.0
            .as_ref()
            .map_or(0, |writer| writer.identity.fetch_add(1, Ordering::Relaxed))
    }
}

/// Connection/session identifiers are newly allocated scalars, never protocol IDs.
pub(crate) struct SessionMetrics {
    sink: Metrics,
    pub id: u64,
}

impl SessionMetrics {
    pub fn new(sink: Metrics) -> Self {
        let id = sink.identity();
        sink.record(Event::SessionStarted { session: id });
        Self { sink, id }
    }
}

impl Drop for SessionMetrics {
    fn drop(&mut self) {
        self.sink.record(Event::SessionEnded { session: self.id });
    }
}

pub(crate) fn unavailable() -> SelectionMeasurement {
    SelectionMeasurement::unavailable(MeasurementStatus::NoPresentation)
}

pub(crate) fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
