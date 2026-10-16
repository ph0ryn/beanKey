//! Read-only whole-history aggregation. Record payload memory is bounded to one
//! 16 KiB record plus its health envelope; summaries retain runs and finite strata,
//! never event history. Segment discovery uses the journal's strict sorted listing.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use beankey_converter::{LearningTask, MeasurementStatus, SelectionMeasurement};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use super::{Event, Operation, Record, RunSettings, SCHEMA, build, journal};

use super::journal::MAX_LINE_BYTES;
const HISTOGRAM_BUCKETS: usize = 66;
const INTERPRETATION: &[&str] = &[
    "All supported complete events across metrics-v1 history are included, with no time, size or run cutoff. Reading is not a locked point-in-time snapshot.",
    "Default agreement is same-selection agreement with the initial shown default. Corrections mean a different selected candidate, not post-commit editing or correctness. Missing defaults and invalid scopes are excluded; baseline-unavailable defaults remain eligible.",
    "Paired helped/harmed/unchanged/net are only a same-selection proxy comparing the default with its pre-local-rerank baseline, on comparable records with both matches. Unchanged means equal match flags, not necessarily an unchanged candidate. They are not correctness, causal benefit, or pre/post-editing evidence.",
    "Commit records describe daemon intentions, not OS/application apply acknowledgements. Request and commit durations cover the whole daemon request, including any follow-up candidate refresh after a partial commit; they exclude transport, frontend rendering and OS insertion.",
    "Coverage can be incomplete through corrupt/oversized/unsupported records, interrupted tails, sequence gaps or journal loss. Zero counters do not prove completeness: final counters can be lost at crash, disk-full or shutdown. An open run means no stop was observed, not proof it is still running.",
    "Journal losses sum per-run maxima, not per-line values. Sequence gaps are max(sequence) minus observed event count, independent of arrival order; duplicate lines can understate gaps and are not deduplicated. Unsupported schemas are excluded from event, run and loss aggregates.",
    "First observed RunStarted metadata is retained per run. Settings include upstream_ngram_personalization: the paired baseline does not disable upstream ngram personalization and is not a globally unpersonalized counterfactual. Compare like builds/settings/tasks/scopes/strata, not causal before/after groups.",
    "Commit strata are joint mode/origin/presented task/selected task/source-surface length bucket/selected-recalled/selected-rank bucket, globally and per run. Ranks are zero-based: 0, 1, 2_4, 5_9, 10_plus, unavailable. Missing presentation may carry zero counts or false flags; consult statuses and denominators.",
    "Latency histograms have 66 microsecond upper bounds: 0, 1, 2, 4, ... 2^63, u64::MAX. p50/p95/p99 are ceil-rank bucket upper bounds, not exact quantiles; count/sum/min/max are exact for observed durations. Candidate rerank includes zero-duration observations.",
    "Protocol error codes 0..6 are counted individually; other codes share an unknown bucket. No raw lines or error-message contents are included.",
];

/// Serializable, text-free observations and coverage limits, not a quality score.
#[derive(Serialize)]
pub struct MetricsSummary {
    record_schema: u32,
    interpretation: &'static [&'static str],
    coverage: Coverage,
    first_unix_ms: Option<u64>,
    latest_unix_ms: Option<u64>,
    #[serde(flatten)]
    totals: Aggregates,
    runs: BTreeMap<String, RunSummary>,
}

#[derive(Default, Serialize)]
struct Coverage {
    segments: u64,
    bytes: u64,
    complete_lines: u64,
    complete_events: u64,
    corrupt_lines: u64,
    oversized_lines: u64,
    unsupported_schema: u64,
    incomplete_tails: u64,
    oversized_tails: u64,
    runs: u64,
    observed_run_started_events: u64,
    observed_run_stopped_events: u64,
    open_runs: u64,
    runs_missing_start: u64,
    sequence_gaps: u64,
    journal_dropped_records: u64,
    journal_write_errors: u64,
}

#[derive(Default, Serialize)]
struct RunSummary {
    events: u64,
    max_sequence: u64,
    sequence_gaps: u64,
    run_started_events: u64,
    run_stopped_events: u64,
    open: bool,
    first_unix_ms: Option<u64>,
    latest_unix_ms: Option<u64>,
    journal: journal::Health,
    metadata: Option<RunMetadata>,
    #[serde(flatten)]
    totals: Aggregates,
}

#[derive(Serialize)]
struct RunMetadata {
    build: build::BuildInfo,
    settings: RunSettings,
}

#[derive(Default, Serialize)]
struct Aggregates {
    session_started_events: u64,
    session_ended_events: u64,
    commits: CommitSummary,
    requests: RequestSummary,
    candidate_requests: CandidateSummary,
}

#[derive(Default, Serialize)]
struct CommitSummary {
    #[serde(flatten)]
    counts: CommitCounts,
    duration_us: Latency,
    #[serde(serialize_with = "serialize_strata")]
    strata: BTreeMap<StratumKey, CommitCounts>,
}

#[derive(Default, Serialize)]
struct CommitCounts {
    total: u64,
    statuses: BTreeMap<String, u64>,
    default_agreement: DefaultAgreement,
    paired_same_selection_proxy: PairedProxy,
}

#[derive(Default, Serialize)]
struct DefaultAgreement {
    denominator: u64,
    matches: u64,
    corrections: u64,
}

#[derive(Default, Serialize)]
struct PairedProxy {
    denominator: u64,
    helped: u64,
    harmed: u64,
    unchanged: u64,
    net_helped_minus_harmed: i128,
}

// Every component has finite cardinality, including length/rank buckets. This
// cannot grow once per distinct raw count, request text, or candidate identity.
#[derive(Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct StratumKey {
    mode: String,
    origin: String,
    task: &'static str,
    selected_task: &'static str,
    source_length_bucket: &'static str,
    selected_recalled: bool,
    selected_rank_bucket: &'static str,
}

fn serialize_strata<S: Serializer>(
    strata: &BTreeMap<StratumKey, CommitCounts>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Row<'a> {
        #[serde(flatten)]
        key: &'a StratumKey,
        counts: &'a CommitCounts,
    }
    let mut sequence = serializer.serialize_seq(Some(strata.len()))?;
    for (key, counts) in strata {
        sequence.serialize_element(&Row { key, counts })?;
    }
    sequence.end()
}

#[derive(Default, Serialize)]
struct RequestSummary {
    #[serde(flatten)]
    counts: RequestCounts,
    operations: BTreeMap<Operation, RequestCounts>,
}

#[derive(Default, Serialize)]
struct RequestCounts {
    total: u64,
    consumed: u64,
    errors: u64,
    error_codes: BTreeMap<&'static str, u64>,
    duration_us: Latency,
}

#[derive(Default, Serialize)]
struct CandidateSummary {
    total: u64,
    errors: u64,
    error_codes: BTreeMap<&'static str, u64>,
    capacity_fallback: u64,
    learning_errors: u64,
    personalization_warnings: u64,
    neural_requests: u64,
    rich_requests: u64,
    segment_requests: u64,
    context_available: u64,
    right_context_available: u64,
    duration_us: Latency,
    rerank_us: Latency,
}

struct Latency {
    count: u64,
    sum_us: u128,
    min_us: Option<u64>,
    max_us: Option<u64>,
    buckets: [u64; HISTOGRAM_BUCKETS],
}

impl Default for Latency {
    fn default() -> Self {
        Self {
            count: 0,
            sum_us: 0,
            min_us: None,
            max_us: None,
            buckets: [0; HISTOGRAM_BUCKETS],
        }
    }
}

impl Latency {
    fn observe(&mut self, micros: u64) {
        self.count += 1;
        self.sum_us += u128::from(micros);
        time_range(&mut self.min_us, &mut self.max_us, micros);
        let bucket = if micros == 0 {
            0
        } else {
            (u64::BITS - (micros - 1).leading_zeros()) as usize + 1
        };
        self.buckets[bucket] += 1;
    }

    fn upper_bound(bucket: usize) -> u64 {
        match bucket {
            0 => 0,
            65 => u64::MAX,
            _ => 1u64 << (bucket - 1),
        }
    }

    fn quantile_upper_bound(&self, percentile: u64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let rank = (u128::from(self.count) * u128::from(percentile)).div_ceil(100);
        let mut cumulative = 0u128;
        self.buckets.iter().enumerate().find_map(|(bucket, count)| {
            cumulative += u128::from(*count);
            (cumulative >= rank).then(|| Self::upper_bound(bucket))
        })
    }
}

impl Serialize for Latency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Bucket {
            upper_bound_us: u64,
            count: u64,
        }
        let histogram: Vec<_> = self
            .buckets
            .iter()
            .enumerate()
            .map(|(bucket, count)| Bucket {
                upper_bound_us: Self::upper_bound(bucket),
                count: *count,
            })
            .collect();
        let mut state = serializer.serialize_struct("Latency", 8)?;
        state.serialize_field("count", &self.count)?;
        state.serialize_field("sum_us", &self.sum_us)?;
        state.serialize_field("min_us", &self.min_us)?;
        state.serialize_field("max_us", &self.max_us)?;
        state.serialize_field("p50_upper_bound_us", &self.quantile_upper_bound(50))?;
        state.serialize_field("p95_upper_bound_us", &self.quantile_upper_bound(95))?;
        state.serialize_field("p99_upper_bound_us", &self.quantile_upper_bound(99))?;
        state.serialize_field("histogram", &histogram)?;
        state.end()
    }
}

impl CommitCounts {
    fn observe(&mut self, comparison: &SelectionMeasurement) {
        self.total += 1;
        *self
            .statuses
            .entry(enum_name(&comparison.status))
            .or_default() += 1;
        // Baseline failure does not invalidate the measured default agreement.
        // Other statuses lack a supported same-scope presentation/selection.
        if matches!(
            comparison.status,
            MeasurementStatus::Comparable
                | MeasurementStatus::BaselineUnavailable
                | MeasurementStatus::BaselineDifferentScope
        ) && let Some(matches) = comparison.default_matches_selection
        {
            self.default_agreement.denominator += 1;
            self.default_agreement.matches += u64::from(matches);
            self.default_agreement.corrections += u64::from(!matches);
        }
        if comparison.status == MeasurementStatus::Comparable
            && let (Some(default), Some(baseline)) = (
                comparison.default_matches_selection,
                comparison.baseline_matches_selection,
            )
        {
            let paired = &mut self.paired_same_selection_proxy;
            paired.denominator += 1;
            match (default, baseline) {
                (true, false) => {
                    paired.helped += 1;
                    paired.net_helped_minus_harmed += 1;
                }
                (false, true) => {
                    paired.harmed += 1;
                    paired.net_helped_minus_harmed -= 1;
                }
                _ => paired.unchanged += 1,
            }
        }
    }
}

impl RequestCounts {
    fn observe(&mut self, consumed: bool, error: Option<i32>, duration: u64) {
        self.total += 1;
        self.consumed += u64::from(consumed);
        if let Some(code) = error {
            self.errors += 1;
            *self.error_codes.entry(error_bucket(code)).or_default() += 1;
        }
        self.duration_us.observe(duration);
    }
}

impl Aggregates {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::SessionStarted { .. } => self.session_started_events += 1,
            Event::SessionEnded { .. } => self.session_ended_events += 1,
            Event::Commit {
                mode,
                origin,
                comparison,
                duration_us,
                ..
            } => {
                self.commits.counts.observe(comparison);
                self.commits.duration_us.observe(*duration_us);
                let key = StratumKey {
                    mode: enum_name(mode),
                    origin: enum_name(origin),
                    task: task_name(comparison.task),
                    selected_task: task_name(comparison.selected_task),
                    source_length_bucket: length_bucket(comparison.source_surface_count),
                    selected_recalled: comparison.selected_recalled,
                    selected_rank_bucket: rank_bucket(comparison.selected_rank),
                };
                self.commits
                    .strata
                    .entry(key)
                    .or_default()
                    .observe(comparison);
            }
            Event::Request {
                operation,
                consumed,
                error_code,
                duration_us,
                ..
            } => {
                self.requests
                    .counts
                    .observe(*consumed, *error_code, *duration_us);
                self.requests
                    .operations
                    .entry(*operation)
                    .or_default()
                    .observe(*consumed, *error_code, *duration_us);
            }
            Event::CandidateRequest {
                rich,
                segment,
                context_available,
                right_context_available,
                neural,
                capacity_fallback,
                learning_error,
                personalization_warning,
                error_code,
                duration_us,
                rerank_us,
                ..
            } => {
                let candidates = &mut self.candidate_requests;
                candidates.total += 1;
                if let Some(code) = error_code {
                    candidates.errors += 1;
                    *candidates
                        .error_codes
                        .entry(error_bucket(*code))
                        .or_default() += 1;
                }
                candidates.capacity_fallback += u64::from(*capacity_fallback);
                candidates.learning_errors += u64::from(*learning_error);
                candidates.personalization_warnings += u64::from(*personalization_warning);
                candidates.neural_requests += u64::from(*neural);
                candidates.rich_requests += u64::from(*rich);
                candidates.segment_requests += u64::from(*segment);
                candidates.context_available += u64::from(*context_available);
                candidates.right_context_available += u64::from(*right_context_available);
                candidates.duration_us.observe(*duration_us);
                candidates.rerank_us.observe(*rerank_us);
            }
            Event::RunStarted { .. } | Event::RunStopped => {}
        }
    }
}

impl MetricsSummary {
    fn observe_line(&mut self, line: &[u8]) {
        let value = match serde_json::from_slice::<Value>(line) {
            Ok(value) => value,
            Err(_) => {
                self.coverage.corrupt_lines += 1;
                return;
            }
        };
        // Future record schemas need not deserialize as today's typed Event or
        // health envelope. Classify their schema before attempting either.
        match value
            .get("record")
            .and_then(|record| record.get("schema"))
            .and_then(Value::as_u64)
        {
            Some(schema) if schema != u64::from(SCHEMA) => {
                self.coverage.unsupported_schema += 1;
                return;
            }
            Some(_) => {}
            None => {
                self.coverage.corrupt_lines += 1;
                return;
            }
        }
        #[derive(Deserialize)]
        struct Envelope {
            journal: journal::Health,
            record: Record,
        }
        let envelope = match serde_json::from_value::<Envelope>(value) {
            Ok(envelope) if envelope.record.sequence > 0 => envelope,
            _ => {
                self.coverage.corrupt_lines += 1;
                return;
            }
        };
        let record = envelope.record;
        self.coverage.complete_events += 1;
        time_range(
            &mut self.first_unix_ms,
            &mut self.latest_unix_ms,
            record.unix_ms,
        );
        self.totals.observe(&record.event);
        let run = self.runs.entry(record.run.clone()).or_default();
        run.events += 1;
        run.max_sequence = run.max_sequence.max(record.sequence);
        run.journal.dropped_records = run
            .journal
            .dropped_records
            .max(envelope.journal.dropped_records);
        run.journal.write_errors = run.journal.write_errors.max(envelope.journal.write_errors);
        time_range(
            &mut run.first_unix_ms,
            &mut run.latest_unix_ms,
            record.unix_ms,
        );
        run.totals.observe(&record.event);
        match record.event {
            Event::RunStarted {
                build, settings, ..
            } => {
                run.run_started_events += 1;
                if run.metadata.is_none() {
                    run.metadata = Some(RunMetadata { build, settings });
                }
            }
            Event::RunStopped => run.run_stopped_events += 1,
            _ => {}
        }
    }

    fn finish(&mut self) {
        self.coverage.runs = self.runs.len() as u64;
        for run in self.runs.values_mut() {
            run.sequence_gaps = run.max_sequence.saturating_sub(run.events);
            run.open = run.run_stopped_events == 0;
            self.coverage.sequence_gaps += run.sequence_gaps;
            self.coverage.open_runs += u64::from(run.open);
            self.coverage.runs_missing_start += u64::from(run.run_started_events == 0);
            self.coverage.observed_run_started_events += run.run_started_events;
            self.coverage.observed_run_stopped_events += run.run_stopped_events;
            self.coverage.journal_dropped_records += run.journal.dropped_records;
            self.coverage.journal_write_errors += run.journal.write_errors;
        }
    }
}

/// Read every strictly named segment under `learning_directory/metrics-v1`.
/// Never creates a directory, opens a writer/lock, repairs a tail, or suppresses
/// filesystem errors. Malformed data is counted without retaining its contents.
pub fn summarize_metrics(learning_directory: impl AsRef<Path>) -> io::Result<MetricsSummary> {
    let mut summary = MetricsSummary {
        record_schema: SCHEMA,
        interpretation: INTERPRETATION,
        coverage: Coverage::default(),
        first_unix_ms: None,
        latest_unix_ms: None,
        totals: Aggregates::default(),
        runs: BTreeMap::new(),
    };
    for path in journal::segment_paths(&learning_directory.as_ref().join("metrics-v1"))? {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metrics segment is no longer a regular file",
            ));
        }
        summary.coverage.segments += 1;
        scan_segment(&mut BufReader::new(file), &mut summary)?;
    }
    summary.finish();
    Ok(summary)
}

fn scan_segment(reader: &mut impl BufRead, summary: &mut MetricsSummary) -> io::Result<()> {
    let mut line = Vec::new();
    let mut nonempty = false;
    let mut oversized = false;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            if nonempty {
                summary.coverage.incomplete_tails += 1;
                summary.coverage.oversized_tails += u64::from(oversized);
            }
            return Ok(());
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(bytes.len(), |position| position + 1);
        summary.coverage.bytes += consumed as u64;
        nonempty = true;
        if !oversized {
            if consumed > MAX_LINE_BYTES - line.len() {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(&bytes[..consumed]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            summary.coverage.complete_lines += 1;
            if oversized {
                summary.coverage.oversized_lines += 1;
            } else {
                summary.observe_line(&line[..line.len() - 1]);
            }
            line.clear();
            oversized = false;
            nonempty = false;
        }
    }
}

fn time_range(first: &mut Option<u64>, last: &mut Option<u64>, value: u64) {
    *first = Some(first.map_or(value, |first| first.min(value)));
    *last = Some(last.map_or(value, |last| last.max(value)));
}

// Called only with the closed, unit-variant metrics enums, never record strings.
fn enum_name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("metrics enum serializes")
        .as_str()
        .expect("metrics enum serializes as a string")
        .to_owned()
}

fn task_name(task: Option<LearningTask>) -> &'static str {
    match task {
        Some(LearningTask::Conversion) => "conversion",
        Some(LearningTask::PartialConversion) => "partial_conversion",
        Some(LearningTask::Prediction) => "prediction",
        Some(LearningTask::PostPrediction) => "post_prediction",
        None => "unavailable",
    }
}

fn length_bucket(length: usize) -> &'static str {
    match length {
        0 => "0",
        1..=4 => "1_4",
        5..=8 => "5_8",
        9..=16 => "9_16",
        17..=32 => "17_32",
        33..=64 => "33_64",
        _ => "65_plus",
    }
}

fn rank_bucket(rank: Option<usize>) -> &'static str {
    match rank {
        Some(0) => "0",
        Some(1) => "1",
        Some(2..=4) => "2_4",
        Some(5..=9) => "5_9",
        Some(_) => "10_plus",
        None => "unavailable",
    }
}

fn error_bucket(code: i32) -> &'static str {
    match code {
        0 => "0",
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5",
        6 => "6",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use beankey_converter::{LearningTask, MeasurementStatus, SelectionMeasurement};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::super::{CommitOrigin, Event, Mode, Operation, Record, RunSettings, SCHEMA, build};
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn started() -> Event {
        Event::RunStarted {
            build: build::BuildInfo::current(),
            settings: RunSettings {
                model_store_id: None,
                dictionary_store_id: None,
                llama_store_id: None,
                neural: true,
                upstream_ngram_personalization: true,
                live_conversion: true,
                neural_prediction: true,
                rich_candidates: false,
                inference_limit: 8,
                japanese_prediction: true,
                foreign_prediction: true,
                dictionary_typo: true,
                lm_typo: true,
                custom_input_table: false,
                user_dictionary_entries: 0,
            },
        }
    }

    fn comparison(
        status: MeasurementStatus,
        default: Option<bool>,
        baseline: Option<bool>,
    ) -> SelectionMeasurement {
        SelectionMeasurement {
            status,
            task: Some(LearningTask::Conversion),
            selected_task: Some(LearningTask::Conversion),
            default_matches_selection: default,
            baseline_matches_selection: baseline,
            default_changed: default.zip(baseline).map(|(a, b)| a != b),
            source_surface_count: 5,
            consumed_surface_count: 5,
            candidate_count: 3,
            selected_rank: Some(1),
            selected_recalled: true,
            default_recalled: false,
            fixed_prefix: false,
        }
    }

    fn commit(comparison: SelectionMeasurement) -> Event {
        Event::Commit {
            session: Some(1),
            operation: Operation::Enter,
            mode: Mode::Selecting,
            origin: CommitOrigin::Candidate,
            accepted_chars: 3,
            candidate_chars: 3,
            duration_us: 3,
            comparison,
        }
    }

    fn request(duration_us: u64, consumed: bool, error_code: Option<i32>) -> Event {
        Event::Request {
            session: Some(1),
            operation: Operation::Input,
            shift: false,
            option: false,
            before: Mode::Composing,
            after: Mode::Composing,
            consumed,
            error_code,
            input_chars: 1,
            source_before: 4,
            source_after: 5,
            candidate_window: 0,
            candidate_count: 3,
            selected_rank: Some(0),
            prediction_offered: false,
            duration_us,
        }
    }

    fn line(run: &str, sequence: u64, event: Event, dropped: u64, errors: u64) -> Vec<u8> {
        let record = Record {
            schema: SCHEMA,
            run: run.to_owned(),
            sequence,
            unix_ms: sequence * 100,
            elapsed_us: sequence * 10,
            event,
        };
        let mut bytes = serde_json::to_vec(&json!({
            "journal": {"dropped_records": dropped, "write_errors": errors},
            "record": record,
        }))
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn write_segment(learning: &Path, number: u64, bytes: &[u8]) {
        let directory = learning.join("metrics-v1");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("events-{number:020}.jsonl")), bytes).unwrap();
    }

    fn summary(learning: &Path) -> Value {
        serde_json::to_value(summarize_metrics(learning).unwrap()).unwrap()
    }

    #[test]
    fn includes_all_old_segments_without_a_history_or_run_cutoff() {
        let temporary = TempDir::new().unwrap();
        for sequence in 1..=40 {
            let event = commit(comparison(
                MeasurementStatus::Comparable,
                Some(true),
                Some(false),
            ));
            write_segment(temporary.path(), sequence, &line(A, sequence, event, 0, 0));
        }
        let report = summary(temporary.path());
        assert_eq!(report["coverage"]["segments"], 40);
        assert_eq!(report["coverage"]["complete_events"], 40);
        assert_eq!(report["commits"]["total"], 40);
        assert_eq!(
            report["commits"]["paired_same_selection_proxy"]["helped"],
            40
        );
        assert_eq!(report["first_unix_ms"], 100);
        assert_eq!(report["latest_unix_ms"], 4000);
        assert_eq!(report["runs"][A]["sequence_gaps"], 0);
    }

    #[test]
    fn counts_bad_unknown_oversized_and_incomplete_lines_without_echoing_payloads() {
        let temporary = TempDir::new().unwrap();
        let mut bytes = line(A, 1, started(), 0, 0);
        bytes.extend_from_slice(b"private invalid JSON\n");
        bytes.extend_from_slice(b"{\"record\":{\"schema\":999,\"private\":\"future secret\"}}\n");
        bytes.extend_from_slice(&vec![b'x'; 16513]);
        bytes.push(b'\n');
        bytes.extend_from_slice(
            b"{\"journal\":{},\"record\":{\"schema\":1,\"event\":\"private bad event\"}}\n",
        );
        bytes.extend_from_slice(&line(A, 2, Event::RunStopped, 0, 0));
        bytes.extend_from_slice(b"private interrupted tail");
        write_segment(temporary.path(), 1, &bytes);
        let report = summary(temporary.path());
        assert_eq!(report["coverage"]["bytes"], bytes.len());
        assert_eq!(report["coverage"]["complete_lines"], 6);
        assert_eq!(report["coverage"]["complete_events"], 2);
        assert_eq!(report["coverage"]["corrupt_lines"], 2);
        assert_eq!(report["coverage"]["oversized_lines"], 1);
        assert_eq!(report["coverage"]["unsupported_schema"], 1);
        assert_eq!(report["coverage"]["incomplete_tails"], 1);
        assert_eq!(
            fs::read(
                temporary
                    .path()
                    .join("metrics-v1/events-00000000000000000001.jsonl")
            )
            .unwrap(),
            bytes
        );
        let serialized = report.to_string();
        for private in [
            "private invalid",
            "future secret",
            "private bad event",
            "private interrupted",
        ] {
            assert!(!serialized.contains(private));
        }
    }

    #[test]
    fn newline_limit_is_inclusive_and_an_oversized_tail_is_not_an_event() {
        let temporary = TempDir::new().unwrap();
        let mut exact = line(A, 1, request(1, true, None), 0, 0);
        assert_eq!(exact.pop(), Some(b'\n'));
        exact.resize(MAX_LINE_BYTES - 1, b' ');
        exact.push(b'\n');
        let mut too_long = exact[..exact.len() - 1].to_vec();
        too_long.extend_from_slice(b" \n");
        assert_eq!(too_long.len(), MAX_LINE_BYTES + 1);
        let mut bytes = exact;
        bytes.extend(too_long);
        bytes.extend(vec![b'x'; MAX_LINE_BYTES * 8]);
        write_segment(temporary.path(), 1, &bytes);

        let report = summary(temporary.path());
        assert_eq!(report["coverage"]["complete_lines"], 2);
        assert_eq!(report["coverage"]["complete_events"], 1);
        assert_eq!(report["coverage"]["corrupt_lines"], 0);
        assert_eq!(report["coverage"]["oversized_lines"], 1);
        assert_eq!(report["coverage"]["incomplete_tails"], 1);
        assert_eq!(report["coverage"]["oversized_tails"], 1);
        assert_eq!(report["coverage"]["bytes"], bytes.len());
    }

    #[test]
    fn zero_and_maximum_latencies_keep_finite_buckets_and_unknown_error_counts() {
        let temporary = TempDir::new().unwrap();
        let mut bytes = line(A, 1, request(0, true, Some(-1)), 0, 0);
        bytes.extend(line(A, 2, request(1, true, Some(i32::MAX)), 0, 0));
        bytes.extend(line(A, 3, request(u64::MAX - 1, true, None), 0, 0));
        write_segment(temporary.path(), 1, &bytes);

        let report = summary(temporary.path());
        assert_eq!(report["requests"]["error_codes"], json!({"unknown": 2}));
        let latency = &report["requests"]["duration_us"];
        assert_eq!(latency["count"], 3);
        assert_eq!(latency["sum_us"], u64::MAX);
        assert_eq!(latency["min_us"], 0);
        assert_eq!(latency["max_us"], u64::MAX - 1);
        assert_eq!(latency["p50_upper_bound_us"], 1);
        assert_eq!(latency["p99_upper_bound_us"], u64::MAX);
        let histogram = latency["histogram"].as_array().unwrap();
        assert_eq!(histogram.len(), 66);
        assert_eq!(histogram[0], json!({"upper_bound_us": 0, "count": 1}));
        assert_eq!(
            histogram[65],
            json!({"upper_bound_us": u64::MAX, "count": 1})
        );
    }

    #[test]
    fn missing_directory_is_not_created_and_read_only_history_is_not_changed() {
        let temporary = TempDir::new().unwrap();
        let missing = temporary.path().join("missing");
        let empty = summary(&missing);
        assert_eq!(empty["coverage"]["segments"], 0);
        assert_eq!(empty["coverage"]["complete_events"], 0);
        assert!(empty["first_unix_ms"].is_null());
        assert!(!missing.exists());

        let bytes = line(A, 1, request(1, true, None), 0, 0);
        write_segment(temporary.path(), 1, &bytes);
        let directory = temporary.path().join("metrics-v1");
        let path = directory.join("events-00000000000000000001.jsonl");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(summary(temporary.path())["coverage"]["complete_events"], 1);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o500
        );
        assert!(!directory.join("journal.lock").exists());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn sums_per_run_max_loss_and_computes_gaps_after_reordered_arrival() {
        let temporary = TempDir::new().unwrap();
        let mut bytes = line(A, 1, started(), 0, 0);
        bytes.extend(line(A, 3, request(3, true, None), 2, 1));
        bytes.extend(line(A, 2, request(2, false, None), 1, 0));
        bytes.extend(line(A, 5, Event::RunStopped, 4, 2));
        bytes.extend(line(B, 2, request(2, true, None), 3, 5));
        write_segment(temporary.path(), 1, &bytes);
        let report = summary(temporary.path());
        assert_eq!(report["coverage"]["journal_dropped_records"], 7);
        assert_eq!(report["coverage"]["journal_write_errors"], 7);
        assert_eq!(report["coverage"]["sequence_gaps"], 2);
        assert_eq!(report["coverage"]["observed_run_started_events"], 1);
        assert_eq!(report["coverage"]["observed_run_stopped_events"], 1);
        assert_eq!(report["coverage"]["open_runs"], 1);
        assert_eq!(report["coverage"]["runs_missing_start"], 1);
        assert_eq!(report["runs"][A]["sequence_gaps"], 1);
        assert_eq!(report["runs"][A]["open"], false);
        assert_eq!(report["runs"][B]["open"], true);
        assert_eq!(
            report["runs"][A]["metadata"]["settings"]["upstream_ngram_personalization"],
            true
        );
        assert!(report["runs"][A]["metadata"]["build"].is_object());
    }

    #[test]
    fn separates_default_agreement_from_paired_proxy_and_retains_missing_reasons() {
        let temporary = TempDir::new().unwrap();
        let cases = [
            (MeasurementStatus::Comparable, Some(true), Some(false)),
            (MeasurementStatus::Comparable, Some(false), Some(true)),
            (MeasurementStatus::Comparable, Some(true), Some(true)),
            (MeasurementStatus::BaselineUnavailable, Some(false), None),
            (MeasurementStatus::BaselineDifferentScope, Some(true), None),
            // Even contradictory optional matches must not admit an invalid scope.
            (MeasurementStatus::DifferentScope, Some(true), Some(true)),
            (MeasurementStatus::NoPresentation, None, None),
            (MeasurementStatus::Comparable, Some(true), Some(false)),
        ];
        let mut bytes = Vec::new();
        for (index, (status, default, baseline)) in cases.into_iter().enumerate() {
            let event = if status == MeasurementStatus::NoPresentation {
                Event::Commit {
                    session: Some(1),
                    operation: Operation::Enter,
                    mode: Mode::Composing,
                    origin: CommitOrigin::Raw,
                    accepted_chars: 1,
                    candidate_chars: 0,
                    duration_us: 0,
                    comparison: SelectionMeasurement::unavailable(status),
                }
            } else {
                commit(comparison(status, default, baseline))
            };
            bytes.extend(line(A, index as u64 + 1, event, 0, 0));
        }
        write_segment(temporary.path(), 1, &bytes);
        let report = summary(temporary.path());
        let commits = &report["commits"];
        assert_eq!(commits["total"], 8);
        assert_eq!(
            commits["default_agreement"],
            json!({"denominator":6,"matches":4,"corrections":2})
        );
        assert_eq!(
            commits["paired_same_selection_proxy"],
            json!({
                "denominator":4,"helped":2,"harmed":1,"unchanged":1,"net_helped_minus_harmed":1
            })
        );
        for reason in [
            "baseline_unavailable",
            "baseline_different_scope",
            "different_scope",
            "no_presentation",
        ] {
            assert_eq!(commits["statuses"][reason], 1);
        }
        let strata = commits["strata"].as_array().unwrap();
        assert!(strata.iter().any(|row| row["mode"] == "selecting"
            && row["origin"] == "candidate"
            && row["task"] == "conversion"
            && row["source_length_bucket"] == "5_8"
            && row["selected_recalled"] == true
            && row["selected_rank_bucket"] == "1"
            && row["counts"]["total"] == 7));
        assert!(strata.iter().any(|row| row["origin"] == "raw"
            && row["task"] == "unavailable"
            && row["counts"]["total"] == 1));
        assert_eq!(
            report["runs"][A]["commits"]["default_agreement"]["denominator"],
            6
        );
        let interpretation = report["interpretation"].to_string();
        assert!(interpretation.contains("same-selection"));
        assert!(interpretation.contains("causal"));
        assert!(interpretation.contains("upstream_ngram_personalization"));
    }

    #[test]
    fn aggregates_operations_error_codes_candidates_and_bounded_latency_upper_bounds() {
        let temporary = TempDir::new().unwrap();
        let mut bytes = line(A, 1, request(3, true, None), 0, 0);
        bytes.extend(line(A, 2, request(9, false, Some(6)), 0, 0));
        bytes.extend(line(
            A,
            3,
            Event::CandidateRequest {
                session: 1,
                mode: Mode::Previewing,
                source_surface_count: 4,
                candidate_count: 3,
                rich: true,
                segment: false,
                context_available: true,
                right_context_available: false,
                neural: true,
                capacity_fallback: true,
                learning_error: true,
                personalization_warning: true,
                error_code: Some(6),
                duration_us: 33,
                rerank_us: 3,
            },
            0,
            0,
        ));
        write_segment(temporary.path(), 1, &bytes);
        let report = summary(temporary.path());
        assert_eq!(report["requests"]["total"], 2);
        assert_eq!(report["requests"]["consumed"], 1);
        assert_eq!(report["requests"]["errors"], 1);
        assert_eq!(report["requests"]["error_codes"]["6"], 1);
        assert_eq!(report["requests"]["operations"]["input"]["total"], 2);
        let latency = &report["requests"]["duration_us"];
        assert_eq!(latency["count"], 2);
        assert_eq!(latency["sum_us"], 12);
        assert_eq!(latency["min_us"], 3);
        assert_eq!(latency["max_us"], 9);
        assert_eq!(latency["p50_upper_bound_us"], 4);
        assert_eq!(latency["p95_upper_bound_us"], 16);
        assert_eq!(latency["p99_upper_bound_us"], 16);
        assert_eq!(latency["histogram"].as_array().unwrap().len(), 66);
        let candidates = &report["candidate_requests"];
        for field in [
            "total",
            "errors",
            "capacity_fallback",
            "learning_errors",
            "personalization_warnings",
            "neural_requests",
        ] {
            assert_eq!(candidates[field], 1);
        }
        assert_eq!(candidates["duration_us"]["p50_upper_bound_us"], 64);
        assert_eq!(candidates["rerank_us"]["p50_upper_bound_us"], 4);
    }

    #[test]
    fn ignores_unrelated_names_and_symlink_files_and_returns_io_errors() {
        let temporary = TempDir::new().unwrap();
        let bytes = line(A, 1, request(1, true, None), 0, 0);
        write_segment(temporary.path(), 1, &bytes);
        let directory = temporary.path().join("metrics-v1");
        let victim = temporary.path().join("private-victim");
        fs::write(&victim, b"private victim contents").unwrap();
        symlink(&victim, directory.join("events-00000000000000000002.jsonl")).unwrap();
        fs::write(directory.join("events-3.jsonl"), b"unrelated").unwrap();
        assert_eq!(summary(temporary.path())["coverage"]["segments"], 1);
        assert_eq!(fs::read(&victim).unwrap(), b"private victim contents");

        let invalid_learning = temporary.path().join("invalid");
        fs::create_dir(&invalid_learning).unwrap();
        fs::write(invalid_learning.join("metrics-v1"), b"not a directory").unwrap();
        assert!(summarize_metrics(&invalid_learning).is_err());
    }
}
