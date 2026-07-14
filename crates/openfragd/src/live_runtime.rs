use openfrag_capture::{SaveDisposition, SaveProvenance};
use openfrag_gsi::{EventSink, EvidenceReceipt};
use openfrag_live::{
    Clock, Coordinator, EvidenceStore, IngestOutcome, LiveDiagnostic, Recorder, Scheduler,
    StorageEvidenceStore, TimerId, TimerOutcome,
};
use openfrag_storage::Storage;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const MAX_DIAGNOSTICS: usize = 32;

/// Cloneable monotonic clock sharing one daemon-relative epoch.
#[derive(Clone)]
pub struct MonotonicClock {
    started: Arc<Instant>,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            started: Arc::new(Instant::now()),
        }
    }
}

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

enum DeadlineCommand {
    Schedule { timer: TimerId, deadline_ms: u64 },
    Cancel { timer: TimerId },
}

/// Coordinator-owned scheduling handle; it never owns or calls the live runtime.
#[derive(Clone)]
pub struct ChannelScheduler {
    sender: mpsc::Sender<DeadlineCommand>,
}

/// Daemon-owned deadline receiver that yields timer identities for dispatch.
pub struct DeadlineDriver {
    receiver: mpsc::Receiver<DeadlineCommand>,
    deadlines: HashMap<TimerId, u64>,
    disconnected: bool,
}

#[must_use]
pub fn deadline_channel() -> (ChannelScheduler, DeadlineDriver) {
    let (sender, receiver) = mpsc::channel();
    (
        ChannelScheduler { sender },
        DeadlineDriver {
            receiver,
            deadlines: HashMap::new(),
            disconnected: false,
        },
    )
}

impl Scheduler for ChannelScheduler {
    fn schedule(&self, timer: &TimerId, deadline_ms: u64) -> Result<(), LiveDiagnostic> {
        self.sender
            .send(DeadlineCommand::Schedule {
                timer: timer.clone(),
                deadline_ms,
            })
            .map_err(|_| LiveDiagnostic::Scheduler("deadline driver is unavailable".into()))
    }

    fn cancel(&self, timer: &TimerId) -> Result<(), LiveDiagnostic> {
        self.sender
            .send(DeadlineCommand::Cancel {
                timer: timer.clone(),
            })
            .map_err(|_| LiveDiagnostic::Scheduler("deadline driver is unavailable".into()))
    }
}

impl DeadlineDriver {
    /// Applies queued schedule changes and returns every timer due at `now_ms`.
    #[must_use]
    pub fn take_due(&mut self, now_ms: u64) -> Vec<TimerId> {
        self.drain_commands();
        let mut due = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now_ms)
            .map(|(timer, deadline)| (*deadline, timer.clone()))
            .collect::<Vec<_>>();
        due.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.as_str().cmp(right.1.as_str()))
        });
        for (_, timer) in &due {
            self.deadlines.remove(timer);
        }
        due.into_iter().map(|(_, timer)| timer).collect()
    }

    /// Returns the earliest known deadline after applying queued changes.
    #[must_use]
    pub fn next_deadline_ms(&mut self) -> Option<u64> {
        self.drain_commands();
        self.deadlines.values().copied().min()
    }

    /// Blocks a dedicated worker until a timer is due or every scheduler handle is dropped.
    ///
    /// A Tokio daemon can run this method in `spawn_blocking`, then send each returned
    /// [`TimerId`] to the async owner of [`LiveRuntime::dispatch_timer`].
    pub fn wait_next_due<C: Clock>(&mut self, clock: &C) -> Option<TimerId> {
        loop {
            self.drain_commands();
            if let Some(timer) = self.take_next_due(clock.now_ms()) {
                return Some(timer);
            }
            let command = match self.next_deadline_ms() {
                Some(deadline_ms) => {
                    let wait_ms = deadline_ms.saturating_sub(clock.now_ms());
                    if self.disconnected {
                        std::thread::sleep(Duration::from_millis(wait_ms));
                        None
                    } else {
                        match self.receiver.recv_timeout(Duration::from_millis(wait_ms)) {
                            Ok(command) => Some(command),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                self.disconnected = true;
                                None
                            }
                        }
                    }
                }
                None if self.disconnected => return None,
                None => {
                    if let Ok(command) = self.receiver.recv() {
                        Some(command)
                    } else {
                        self.disconnected = true;
                        None
                    }
                }
            };
            if let Some(command) = command {
                self.apply(command);
            } else if self.disconnected && self.deadlines.is_empty() {
                return None;
            }
        }
    }

    fn drain_commands(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(command) => self.apply(command),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
    }

    fn take_next_due(&mut self, now_ms: u64) -> Option<TimerId> {
        let timer = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now_ms)
            .min_by(|left, right| {
                left.1
                    .cmp(right.1)
                    .then_with(|| left.0.as_str().cmp(right.0.as_str()))
            })
            .map(|(timer, _)| timer.clone())?;
        self.deadlines.remove(&timer);
        Some(timer)
    }

    fn apply(&mut self, command: DeadlineCommand) {
        match command {
            DeadlineCommand::Schedule { timer, deadline_ms } => {
                self.deadlines.insert(timer, deadline_ms);
            }
            DeadlineCommand::Cancel { timer } => {
                self.deadlines.remove(&timer);
            }
        }
    }
}

pub trait CaptureRuntimePort: Send {
    fn readiness(&self) -> Result<(), String>;
    fn available_from_ms(&self) -> u64;
    fn request_save(&mut self, provenance: SaveProvenance) -> Result<SaveDisposition, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    Capture(String),
    Storage(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveRuntimeStatus {
    Ready,
    Unavailable(UnavailableReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveRuntimeError {
    Unavailable(UnavailableReason),
    Diagnostic(LiveDiagnostic),
    State(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticRecord {
    pub operation: &'static str,
    pub message: String,
}

struct RecorderFacade<R> {
    capture: Arc<Mutex<R>>,
    available_from_ms: u64,
}

impl<R> Recorder for RecorderFacade<R>
where
    R: CaptureRuntimePort,
{
    fn available_from_ms(&self) -> u64 {
        self.available_from_ms
    }

    fn request_save(&self, provenance: SaveProvenance) -> Result<SaveDisposition, String> {
        self.capture
            .lock()
            .map_err(|_| "capture runtime lock poisoned".to_owned())?
            .request_save(provenance)
    }
}

type LiveCoordinator<E, R, C, S> = Coordinator<E, RecorderFacade<R>, C, S>;

enum Gate<E, R, C, S> {
    Ready(Box<Mutex<LiveCoordinator<E, R, C, S>>>),
    Unavailable(UnavailableReason),
}

/// Thread-safe daemon facade for live evidence and replay capture coordination.
pub struct LiveRuntime<E, R, C, S> {
    gate: Gate<E, R, C, S>,
    diagnostics: Mutex<VecDeque<DiagnosticRecord>>,
}

impl<R, C, S> LiveRuntime<StorageEvidenceStore, R, C, S>
where
    R: CaptureRuntimePort + 'static,
    C: Clock + 'static,
    S: Scheduler + 'static,
{
    #[must_use]
    pub fn durable(
        storage: Storage,
        local_steam_id: &str,
        capture: R,
        clock: C,
        scheduler: S,
    ) -> Self {
        match StorageEvidenceStore::new(storage, local_steam_id) {
            Ok(store) => {
                let session = store.capture_session_id().to_owned();
                Self::with_store(session, store, capture, clock, scheduler)
            }
            Err(error) => Self::unavailable(UnavailableReason::Storage(error.to_string())),
        }
    }
}

impl<E, R, C, S> LiveRuntime<E, R, C, S>
where
    E: EvidenceStore + 'static,
    R: CaptureRuntimePort + 'static,
    C: Clock + 'static,
    S: Scheduler + 'static,
{
    #[must_use]
    pub fn with_store(
        capture_session_id: impl Into<String>,
        store: E,
        capture: R,
        clock: C,
        scheduler: S,
    ) -> Self {
        if let Err(reason) = capture.readiness() {
            return Self::unavailable(UnavailableReason::Capture(reason));
        }
        let available_from_ms = capture.available_from_ms();
        let capture = Arc::new(Mutex::new(capture));
        let recorder = Arc::new(RecorderFacade {
            capture,
            available_from_ms,
        });
        let coordinator = Coordinator::new(
            capture_session_id.into(),
            Arc::new(store),
            recorder,
            Arc::new(clock),
            Arc::new(scheduler),
        );
        Self {
            gate: Gate::Ready(Box::new(Mutex::new(coordinator))),
            diagnostics: Mutex::new(VecDeque::new()),
        }
    }

    #[must_use]
    pub fn status(&self) -> LiveRuntimeStatus {
        match &self.gate {
            Gate::Ready(_) => LiveRuntimeStatus::Ready,
            Gate::Unavailable(reason) => LiveRuntimeStatus::Unavailable(reason.clone()),
        }
    }

    pub fn ingest_receipt(
        &self,
        receipt: &EvidenceReceipt,
    ) -> Result<IngestOutcome, LiveRuntimeError> {
        self.run("receipt_ingest", |coordinator| coordinator.ingest(receipt))
    }

    pub fn dispatch_timer(&self, timer_id: &str) -> Result<TimerOutcome, LiveRuntimeError> {
        let timer = TimerId::new(timer_id);
        self.run("timer_dispatch", |coordinator| coordinator.on_timer(&timer))
    }

    pub fn manual_flag(&self) -> Result<String, LiveRuntimeError> {
        self.run("manual_flag", Coordinator::manual_flag)
    }

    pub fn retry_capture(&self, capture_id: &str) -> Result<TimerOutcome, LiveRuntimeError> {
        self.run("capture_retry", |coordinator| {
            coordinator.retry_capture(capture_id)
        })
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<DiagnosticRecord> {
        self.diagnostics
            .lock()
            .map_or_else(|_| Vec::new(), |records| records.iter().cloned().collect())
    }

    fn unavailable(reason: UnavailableReason) -> Self {
        Self {
            gate: Gate::Unavailable(reason),
            diagnostics: Mutex::new(VecDeque::new()),
        }
    }

    fn run<T>(
        &self,
        operation: &'static str,
        action: impl FnOnce(&mut LiveCoordinator<E, R, C, S>) -> Result<T, LiveDiagnostic>,
    ) -> Result<T, LiveRuntimeError> {
        let coordinator = match &self.gate {
            Gate::Ready(coordinator) => coordinator,
            Gate::Unavailable(reason) => {
                return Err(LiveRuntimeError::Unavailable(reason.clone()));
            }
        };
        let result = coordinator
            .lock()
            .map_err(|_| LiveRuntimeError::State("live coordinator lock poisoned".into()))
            .and_then(|mut coordinator| {
                action(&mut coordinator).map_err(LiveRuntimeError::Diagnostic)
            });
        if let Err(error) = &result {
            self.record_diagnostic(operation, format!("{error:?}"));
        }
        result
    }

    fn record_diagnostic(&self, operation: &'static str, message: String) {
        if let Ok(mut records) = self.diagnostics.lock() {
            records.push_back(DiagnosticRecord { operation, message });
            while records.len() > MAX_DIAGNOSTICS {
                records.pop_front();
            }
        }
    }
}

impl<E, R, C, S> EventSink for LiveRuntime<E, R, C, S>
where
    E: EvidenceStore + 'static,
    R: CaptureRuntimePort + 'static,
    C: Clock + 'static,
    S: Scheduler + 'static,
{
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String> {
        self.ingest_receipt(&receipt)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }
}
