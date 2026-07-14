#![forbid(unsafe_code)]

use openfrag_capture::{SaveDisposition, SaveProvenance};
use openfrag_domain::{
    AUTO_POST_ROLL_MS, AutoCaptureDecision, CandidateTrigger, CaptureWindow, FinalHighlightLabel,
    HighlightCandidate, REPLAY_BUFFER_MS, TimeRange, auto_capture_window, live_kill_trigger,
    manual_capture_window, merge_candidates,
};
use openfrag_gsi::{EvidenceReceipt, StateOutput, TransitionFact};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    sync::Arc,
};

mod storage_adapter;

pub use storage_adapter::StorageEvidenceStore;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimerId(String);

impl TimerId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    AutoRoundEnd,
    ManualFlag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    Stale,
    SessionReset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStatus {
    Scheduled,
    Requesting,
    SaveRequested(SaveDisposition),
    Retryable(String),
    Cancelled(CancelReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRecord {
    pub candidate: HighlightCandidate,
    pub final_labels: BTreeSet<FinalHighlightLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    pub id: String,
    pub kind: CaptureKind,
    pub round_id: Option<String>,
    pub provenance: SaveProvenance,
    pub source_receipt_ids: BTreeSet<String>,
    pub candidate: Option<HighlightCandidate>,
    pub final_labels: BTreeSet<FinalHighlightLabel>,
    pub round_end_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub timer: Option<TimerId>,
    pub window: Option<CaptureWindow>,
    pub raw_coverage: Option<TimeRange>,
    pub status: CaptureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveDiagnostic {
    Persistence(String),
    Unsupported(&'static str),
    Recorder(String),
    Scheduler(String),
    MissingCapture(String),
    InvalidDomain,
}

impl fmt::Display for LiveDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence(message) => write!(formatter, "live evidence persistence: {message}"),
            Self::Unsupported(message) => {
                write!(formatter, "unsupported durable live state: {message}")
            }
            Self::Recorder(message) => write!(formatter, "replay recorder: {message}"),
            Self::Scheduler(message) => write!(formatter, "live scheduler: {message}"),
            Self::MissingCapture(id) => write!(formatter, "missing live capture: {id}"),
            Self::InvalidDomain => formatter.write_str("invalid live highlight domain state"),
        }
    }
}

impl std::error::Error for LiveDiagnostic {}

pub trait EvidenceStore: Send + Sync {
    /// Persists a sanitized receipt.
    ///
    /// # Errors
    ///
    /// Returns a persistence diagnostic when durable storage fails.
    fn persist_receipt(&self, receipt: &EvidenceReceipt) -> Result<(), LiveDiagnostic>;
    /// Inserts or updates provisional candidate metadata.
    ///
    /// # Errors
    ///
    /// Returns a persistence diagnostic when durable storage fails.
    fn upsert_candidate(&self, candidate: &CandidateRecord) -> Result<(), LiveDiagnostic>;
    /// Inserts or updates provisional capture metadata.
    ///
    /// # Errors
    ///
    /// Returns a persistence diagnostic when durable storage fails.
    fn upsert_capture(&self, capture: &CaptureRecord) -> Result<(), LiveDiagnostic>;
}

pub trait Recorder: Send + Sync {
    fn available_from_ms(&self) -> u64;
    /// Requests one replay-buffer save using recorder-native coalescing.
    ///
    /// # Errors
    ///
    /// Returns a recorder diagnostic while leaving coordinator metadata retryable.
    fn request_save(&self, provenance: SaveProvenance) -> Result<SaveDisposition, String>;
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub trait Scheduler: Send + Sync {
    /// Registers a monotonic deadline.
    ///
    /// # Errors
    ///
    /// Returns a scheduler diagnostic if the deadline cannot be registered.
    fn schedule(&self, timer: &TimerId, deadline_ms: u64) -> Result<(), LiveDiagnostic>;
    /// Cancels a registered deadline.
    ///
    /// # Errors
    ///
    /// Returns a scheduler diagnostic if cancellation fails.
    fn cancel(&self, timer: &TimerId) -> Result<(), LiveDiagnostic>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerOutcome {
    NotDue,
    SaveRequested,
    AlreadyHandled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngestOutcome {
    pub candidates_created: usize,
    pub captures_scheduled: usize,
    pub captures_cancelled: usize,
}

pub struct Coordinator<E, R, C, S> {
    capture_session_id: String,
    store: Arc<E>,
    recorder: Arc<R>,
    clock: Arc<C>,
    scheduler: Arc<S>,
    processed_receipts: HashSet<String>,
    candidates: HashMap<String, CandidateRecord>,
    captures: HashMap<String, CaptureRecord>,
    timers: HashMap<TimerId, String>,
    manual_ordinal: u64,
}

impl<E, R, C, S> Coordinator<E, R, C, S>
where
    E: EvidenceStore,
    R: Recorder,
    C: Clock,
    S: Scheduler,
{
    #[must_use]
    pub fn new(
        capture_session_id: impl Into<String>,
        store: Arc<E>,
        recorder: Arc<R>,
        clock: Arc<C>,
        scheduler: Arc<S>,
    ) -> Self {
        Self {
            capture_session_id: capture_session_id.into(),
            store,
            recorder,
            clock,
            scheduler,
            processed_receipts: HashSet::new(),
            candidates: HashMap::new(),
            captures: HashMap::new(),
            timers: HashMap::new(),
            manual_ordinal: 0,
        }
    }

    /// Persists and applies one sanitized GSI evidence receipt.
    ///
    /// # Errors
    ///
    /// Returns a persistence or scheduler diagnostic without marking the receipt processed.
    pub fn ingest(&mut self, receipt: &EvidenceReceipt) -> Result<IngestOutcome, LiveDiagnostic> {
        self.store.persist_receipt(receipt)?;
        let receipt_id = receipt_id(receipt);
        if self.processed_receipts.contains(&receipt_id) {
            return Ok(IngestOutcome::default());
        }

        let mut outcome = IngestOutcome::default();
        match receipt.output {
            StateOutput::Stale => {
                outcome.captures_cancelled = self.cancel_unsaved(CancelReason::Stale)?;
            }
            StateOutput::SessionReset => {
                outcome.captures_cancelled = self.cancel_unsaved(CancelReason::SessionReset)?;
            }
            StateOutput::Seeded | StateOutput::Healthy | StateOutput::Recovered => {}
        }

        for fact in &receipt.facts {
            match fact {
                TransitionFact::RoundKillDelta {
                    previous,
                    current,
                    delta,
                } if *previous >= 0 && *delta > 0 && *current >= 3 => {
                    outcome.candidates_created +=
                        usize::from(self.create_candidate(receipt, &receipt_id, *current)?);
                }
                TransitionFact::RoundEnd { completed, .. } => {
                    outcome.captures_scheduled += usize::from(self.schedule_round_capture(
                        receipt,
                        &receipt_id,
                        *completed,
                    )?);
                }
                TransitionFact::CumulativeKillDelta { .. }
                | TransitionFact::CumulativeDeathDelta { .. }
                | TransitionFact::RoundKillDelta { .. }
                | TransitionFact::HealthDepleted { .. } => {}
            }
        }
        self.processed_receipts.insert(receipt_id);
        Ok(outcome)
    }

    /// Handles one scheduled deadline.
    ///
    /// # Errors
    ///
    /// Returns recorder or persistence diagnostics while leaving failed captures retryable.
    pub fn on_timer(&mut self, timer: &TimerId) -> Result<TimerOutcome, LiveDiagnostic> {
        let capture_id = self
            .timers
            .get(timer)
            .cloned()
            .ok_or_else(|| LiveDiagnostic::MissingCapture(timer.as_str().into()))?;
        let capture = self
            .captures
            .get(&capture_id)
            .ok_or_else(|| LiveDiagnostic::MissingCapture(capture_id.clone()))?;
        let Some(deadline_ms) = capture.deadline_ms else {
            return Ok(TimerOutcome::AlreadyHandled);
        };
        if self.clock.now_ms() < deadline_ms {
            return Ok(TimerOutcome::NotDue);
        }
        if capture.status != CaptureStatus::Scheduled {
            return Ok(TimerOutcome::AlreadyHandled);
        }
        self.request_capture_save(&capture_id)
    }

    /// Retries a recorder failure without creating a new capture identity.
    ///
    /// # Errors
    ///
    /// Returns recorder or persistence diagnostics if the retry fails.
    pub fn retry_capture(&mut self, capture_id: &str) -> Result<TimerOutcome, LiveDiagnostic> {
        let capture = self
            .captures
            .get(capture_id)
            .ok_or_else(|| LiveDiagnostic::MissingCapture(capture_id.into()))?;
        if !matches!(capture.status, CaptureStatus::Retryable(_)) {
            return Ok(TimerOutcome::AlreadyHandled);
        }
        self.request_capture_save(capture_id)
    }

    /// Immediately records and requests a Manual Flag replay save.
    ///
    /// # Errors
    ///
    /// Returns recorder or persistence diagnostics; recorder failures retain retryable metadata.
    pub fn manual_flag(&mut self) -> Result<String, LiveDiagnostic> {
        let now_ms = self.clock.now_ms();
        self.manual_ordinal = self.manual_ordinal.saturating_add(1);
        let id = format!("manual:{now_ms}:{}", self.manual_ordinal);
        let window = manual_capture_window(now_ms, self.recorder.available_from_ms());
        let capture = CaptureRecord {
            id: id.clone(),
            kind: CaptureKind::ManualFlag,
            round_id: None,
            provenance: SaveProvenance::ManualFlag,
            source_receipt_ids: BTreeSet::new(),
            candidate: None,
            final_labels: BTreeSet::new(),
            round_end_ms: None,
            deadline_ms: None,
            timer: None,
            window: Some(window),
            raw_coverage: Some(raw_coverage(now_ms, self.recorder.available_from_ms())),
            status: CaptureStatus::Requesting,
        };
        self.store.upsert_capture(&capture)?;
        self.captures.insert(id.clone(), capture);
        self.request_capture_save(&id)?;
        Ok(id)
    }

    fn create_candidate(
        &mut self,
        receipt: &EvidenceReceipt,
        receipt_id: &str,
        current_round_kills: i64,
    ) -> Result<bool, LiveDiagnostic> {
        let (Some(map_hash), Some(round), Some(context_round_kills)) = (
            receipt.context.map_hash.as_deref(),
            receipt.context.observed_round,
            receipt.context.round_kills,
        ) else {
            return Ok(false);
        };
        if context_round_kills != current_round_kills {
            return Ok(false);
        }
        let Ok(round_kills) = u8::try_from(current_round_kills) else {
            return Ok(false);
        };
        let Some(trigger) = live_kill_trigger(true, round_kills) else {
            return Ok(false);
        };
        let round_id = round_id(map_hash, round);
        if self.candidates.contains_key(&round_id) {
            return Ok(false);
        }
        let at_ms = duration_ms(receipt);
        let candidate = HighlightCandidate::new(
            &self.capture_session_id,
            &round_id,
            TimeRange {
                start_ms: at_ms,
                end_ms: at_ms,
            },
            trigger,
            receipt_id,
        )
        .map_err(|_| LiveDiagnostic::InvalidDomain)?;
        let record = CandidateRecord {
            candidate,
            final_labels: BTreeSet::new(),
        };
        self.store.upsert_candidate(&record)?;
        self.candidates.insert(round_id, record);
        Ok(true)
    }

    fn schedule_round_capture(
        &mut self,
        receipt: &EvidenceReceipt,
        receipt_id: &str,
        completed_round: u64,
    ) -> Result<bool, LiveDiagnostic> {
        let (Some(map_hash), Some(_)) = (
            receipt.context.map_hash.as_deref(),
            receipt.context.observed_round,
        ) else {
            return Ok(false);
        };
        let round_id = round_id(map_hash, completed_round);
        let capture_id = format!("auto:{round_id}");
        if self.captures.contains_key(&capture_id) {
            return Ok(false);
        }
        let round_end_ms = duration_ms(receipt);
        let official = HighlightCandidate::new(
            &self.capture_session_id,
            &round_id,
            TimeRange {
                start_ms: round_end_ms,
                end_ms: round_end_ms,
            },
            CandidateTrigger::OfficialRoundEndCapture,
            receipt_id,
        )
        .map_err(|_| LiveDiagnostic::InvalidDomain)?;
        let candidate = if let Some(existing) = self.candidates.get(&round_id) {
            let mut extended = existing.candidate.clone();
            extended.range.end_ms = round_end_ms.max(extended.range.end_ms);
            merge_candidates(vec![extended, official])
                .into_iter()
                .next()
                .ok_or(LiveDiagnostic::InvalidDomain)?
        } else {
            official
        };
        let deadline_ms = round_end_ms.saturating_add(AUTO_POST_ROLL_MS);
        let timer = TimerId::new(format!("timer:{capture_id}"));
        let capture = CaptureRecord {
            id: capture_id.clone(),
            kind: CaptureKind::AutoRoundEnd,
            round_id: Some(round_id),
            provenance: SaveProvenance::AutoRoundEnd,
            source_receipt_ids: candidate.receipt_ids.clone(),
            candidate: Some(candidate),
            final_labels: BTreeSet::new(),
            round_end_ms: Some(round_end_ms),
            deadline_ms: Some(deadline_ms),
            timer: Some(timer.clone()),
            window: None,
            raw_coverage: None,
            status: CaptureStatus::Scheduled,
        };
        self.scheduler.schedule(&timer, deadline_ms)?;
        if let Err(error) = self.store.upsert_capture(&capture) {
            let _ = self.scheduler.cancel(&timer);
            return Err(error);
        }
        self.timers.insert(timer, capture_id.clone());
        self.captures.insert(capture_id, capture);
        Ok(true)
    }

    fn cancel_unsaved(&mut self, reason: CancelReason) -> Result<usize, LiveDiagnostic> {
        let ids: Vec<_> = self
            .captures
            .iter()
            .filter(|(_, capture)| {
                capture.kind == CaptureKind::AutoRoundEnd
                    && matches!(
                        capture.status,
                        CaptureStatus::Scheduled | CaptureStatus::Retryable(_)
                    )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            let mut capture = self
                .captures
                .get(id)
                .cloned()
                .ok_or_else(|| LiveDiagnostic::MissingCapture(id.clone()))?;
            if let Some(timer) = &capture.timer {
                self.scheduler.cancel(timer)?;
            }
            capture.status = CaptureStatus::Cancelled(reason);
            self.store.upsert_capture(&capture)?;
            self.captures.insert(id.clone(), capture);
        }
        Ok(ids.len())
    }

    fn request_capture_save(&mut self, capture_id: &str) -> Result<TimerOutcome, LiveDiagnostic> {
        let mut capture = self
            .captures
            .get(capture_id)
            .cloned()
            .ok_or_else(|| LiveDiagnostic::MissingCapture(capture_id.into()))?;
        let now_ms = self.clock.now_ms();
        if capture.kind == CaptureKind::AutoRoundEnd {
            let candidate_start_ms = capture
                .candidate
                .as_ref()
                .map(|candidate| candidate.range.start_ms)
                .ok_or(LiveDiagnostic::InvalidDomain)?;
            let decision = auto_capture_window(
                candidate_start_ms,
                capture.round_end_ms,
                self.recorder.available_from_ms(),
            );
            let AutoCaptureDecision::Save(window) = decision else {
                return Err(LiveDiagnostic::InvalidDomain);
            };
            capture.window = Some(window);
            capture.raw_coverage = Some(raw_coverage(now_ms, self.recorder.available_from_ms()));
        }
        match self.recorder.request_save(capture.provenance) {
            Ok(disposition) => {
                capture.status = CaptureStatus::SaveRequested(disposition);
                self.store.upsert_capture(&capture)?;
                self.captures.insert(capture_id.into(), capture);
                Ok(TimerOutcome::SaveRequested)
            }
            Err(message) => {
                capture.status = CaptureStatus::Retryable(message.clone());
                self.store.upsert_capture(&capture)?;
                self.captures.insert(capture_id.into(), capture);
                Err(LiveDiagnostic::Recorder(message))
            }
        }
    }
}

fn receipt_id(receipt: &EvidenceReceipt) -> String {
    format!("{}:{}", receipt.payload_hash, receipt.sequence)
}

fn round_id(map_hash: &str, round: u64) -> String {
    format!("{map_hash}:{round}")
}

fn duration_ms(receipt: &EvidenceReceipt) -> u64 {
    u64::try_from(receipt.received_at.as_millis()).unwrap_or(u64::MAX)
}

fn raw_coverage(requested_at_ms: u64, recorder_available_from_ms: u64) -> TimeRange {
    TimeRange {
        start_ms: requested_at_ms
            .saturating_sub(REPLAY_BUFFER_MS)
            .max(recorder_available_from_ms),
        end_ms: requested_at_ms,
    }
}
