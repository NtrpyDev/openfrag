use openfrag_capture::{SaveDisposition, SaveProvenance};
use openfrag_gsi::{EventSink, EvidenceReceipt};
use openfrag_live::{
    Clock, Coordinator, EvidenceStore, IngestOutcome, LiveDiagnostic, Recorder, Scheduler,
    StorageEvidenceStore, TimerId, TimerOutcome,
};
use openfrag_storage::Storage;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const MAX_DIAGNOSTICS: usize = 32;

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
