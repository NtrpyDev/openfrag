use openfrag_capture::{SaveAcknowledgement, SaveProvenance, SaveRequestOutcome};
use openfrag_gsi::{EventSink, EvidenceReceipt};
use openfrag_live::{
    Clock, Coordinator, EvidenceStore, FinalizedClip, IngestOutcome, LiveDiagnostic, Recorder,
    Scheduler, StorageEvidenceStore, TimerId, TimerOutcome,
};
use openfrag_storage::Storage;
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_DIAGNOSTICS: usize = 32;

/// Cloneable monotonic clock sharing one daemon-relative epoch.
#[derive(Clone)]
pub struct MonotonicClock {
    started: Arc<Instant>,
    started_wall_ms: u64,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            started: Arc::new(Instant::now()),
            started_wall_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                .unwrap_or(0),
        }
    }
}

impl openfrag_capture::Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        Clock::now_ms(self)
    }

    fn wall_ms(&self) -> u64 {
        self.started_wall_ms.saturating_add(Clock::now_ms(self))
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
    fn request_save(
        &mut self,
        recorder_request_id: &str,
        provenance: SaveProvenance,
    ) -> Result<SaveRequestOutcome, String>;
    fn poll(&mut self) -> Result<Option<i32>, String> {
        Ok(None)
    }
    fn discover_save(&mut self) -> Result<Option<SaveAcknowledgement>, String> {
        Ok(None)
    }
    fn has_save_in_flight(&self) -> bool {
        false
    }
    fn shutdown(&mut self) -> Result<(), String> {
        Ok(())
    }
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
    request_ordinal: AtomicU64,
}

impl<R> Recorder for RecorderFacade<R>
where
    R: CaptureRuntimePort,
{
    fn available_from_ms(&self) -> u64 {
        self.available_from_ms
    }

    fn request_save(
        &self,
        capture_id: &str,
        provenance: SaveProvenance,
    ) -> Result<SaveRequestOutcome, String> {
        let ordinal = self.request_ordinal.fetch_add(1, Ordering::SeqCst) + 1;
        let recorder_request_id = format!("{capture_id}:request:{ordinal}");
        self.capture
            .lock()
            .map_err(|_| "capture runtime lock poisoned".to_owned())?
            .request_save(&recorder_request_id, provenance)
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
    capture: Option<Arc<Mutex<R>>>,
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
            capture: capture.clone(),
            available_from_ms,
            request_ordinal: AtomicU64::new(0),
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
            capture: Some(capture),
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

    pub fn poll_capture(&self) -> Result<Option<i32>, LiveRuntimeError> {
        self.with_capture("capture_poll", CaptureRuntimePort::poll)
    }

    pub fn discover_save(&self) -> Result<Option<SaveAcknowledgement>, LiveRuntimeError> {
        self.with_capture("save_discovery", CaptureRuntimePort::discover_save)
    }

    pub fn finalize_discovered_save(&self) -> Result<Option<FinalizedClip>, LiveRuntimeError> {
        let has_save_in_flight = self
            .capture
            .as_ref()
            .and_then(|capture| capture.lock().ok())
            .is_some_and(|capture| capture.has_save_in_flight());
        if !has_save_in_flight {
            return Ok(None);
        }
        let Some(acknowledgement) = self.discover_save()? else {
            return Ok(None);
        };
        self.run("capture_finalize", |coordinator| {
            coordinator.finalize_capture(&acknowledgement)
        })
        .map(Some)
    }

    pub fn shutdown_capture(&self) -> Result<(), LiveRuntimeError> {
        self.with_capture("capture_shutdown", CaptureRuntimePort::shutdown)
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
            capture: None,
            diagnostics: Mutex::new(VecDeque::new()),
        }
    }

    fn with_capture<T>(
        &self,
        operation: &'static str,
        action: impl FnOnce(&mut R) -> Result<T, String>,
    ) -> Result<T, LiveRuntimeError> {
        let capture = self.capture.as_ref().ok_or_else(|| match &self.gate {
            Gate::Unavailable(reason) => LiveRuntimeError::Unavailable(reason.clone()),
            Gate::Ready(_) => LiveRuntimeError::State("capture runtime is unavailable".into()),
        })?;
        let result = capture
            .lock()
            .map_err(|_| LiveRuntimeError::State("capture runtime lock poisoned".into()))
            .and_then(|mut capture| {
                action(&mut capture).map_err(|message| {
                    LiveRuntimeError::Diagnostic(LiveDiagnostic::Recorder(message))
                })
            });
        if let Err(error) = &result {
            self.record_diagnostic(operation, format!("{error:?}"));
        }
        result
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeDriveReport {
    pub timers_dispatched: usize,
    pub recorder_exit: Option<i32>,
    pub finalized_clip: Option<FinalizedClip>,
}

/// Daemon-owned driver for deadlines, recorder supervision, and save finalization.
pub struct RuntimeDriver<E, R, C, S> {
    runtime: Arc<LiveRuntime<E, R, C, S>>,
    deadlines: Mutex<DeadlineDriver>,
    clock: C,
}

impl<E, R, C, S> RuntimeDriver<E, R, C, S>
where
    E: EvidenceStore + 'static,
    R: CaptureRuntimePort + 'static,
    C: Clock + Clone + 'static,
    S: Scheduler + 'static,
{
    #[must_use]
    pub fn new(runtime: Arc<LiveRuntime<E, R, C, S>>, deadlines: DeadlineDriver, clock: C) -> Self {
        Self {
            runtime,
            deadlines: Mutex::new(deadlines),
            clock,
        }
    }

    #[must_use]
    pub fn runtime(&self) -> Arc<LiveRuntime<E, R, C, S>> {
        self.runtime.clone()
    }

    pub fn drive_once(&self) -> Result<RuntimeDriveReport, LiveRuntimeError> {
        let due = self
            .deadlines
            .lock()
            .map_err(|_| LiveRuntimeError::State("deadline driver lock poisoned".into()))?
            .take_due(self.clock.now_ms());
        for timer in &due {
            self.runtime.dispatch_timer(timer.as_str())?;
        }
        let recorder_exit = self.runtime.poll_capture()?;
        let finalized_clip = self.runtime.finalize_discovered_save()?;
        Ok(RuntimeDriveReport {
            timers_dispatched: due.len(),
            recorder_exit,
            finalized_clip,
        })
    }

    pub fn shutdown(&self) -> Result<(), LiveRuntimeError> {
        self.runtime.shutdown_capture()
    }
}

/// Owns the daemon thread that advances live deadlines and recorder state.
pub struct RuntimeWorker {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl RuntimeWorker {
    pub fn start<E, R, C, S>(
        driver: RuntimeDriver<E, R, C, S>,
        gsi: openfrag_gsi::GsiService,
    ) -> std::io::Result<Self>
    where
        E: EvidenceStore + 'static,
        R: CaptureRuntimePort + 'static,
        C: Clock + Clone + 'static,
        S: Scheduler + 'static,
    {
        let (stop, receiver) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("openfrag-live-runtime".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(Duration::from_millis(25)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let _ = driver.drive_once();
                    let _ = gsi.poll_stale();
                }
                let _ = driver.shutdown();
            })?;
        Ok(Self {
            stop: Some(stop),
            join: Some(join),
        })
    }
}

impl Drop for RuntimeWorker {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub type ProductionCapture = crate::capture_runtime::CaptureRuntime<
    openfrag_capture::StdProcess,
    openfrag_capture::StdFilesystem,
    MonotonicClock,
    openfrag_capture::FfprobeMediaProbe,
>;
pub type ProductionLiveRuntime =
    LiveRuntime<StorageEvidenceStore, ProductionCapture, MonotonicClock, ChannelScheduler>;
pub type ProductionRuntimeDriver =
    RuntimeDriver<StorageEvidenceStore, ProductionCapture, MonotonicClock, ChannelScheduler>;

pub fn production_runtime(
    data_directory: &Path,
    local_steam_id: &str,
) -> Result<ProductionRuntimeDriver, LiveRuntimeError> {
    let configuration =
        openfrag_setup::read_capture_configuration(data_directory).map_err(|error| {
            LiveRuntimeError::Unavailable(UnavailableReason::Capture(format!("{error:?}")))
        })?;
    let clock = MonotonicClock::default();
    let probe = openfrag_capture::FfprobeMediaProbe {
        program: configuration.ffprobe_path().to_path_buf(),
        ..openfrag_capture::FfprobeMediaProbe::default()
    };
    let mut capture = crate::capture_runtime::CaptureRuntime::from_data_directory(
        data_directory,
        openfrag_capture::StdProcess::default(),
        openfrag_capture::StdFilesystem,
        clock.clone(),
        probe,
    );
    capture.start_at(clock.now_ms()).map_err(|error| {
        LiveRuntimeError::Unavailable(UnavailableReason::Capture(format!("{error:?}")))
    })?;
    let storage = Storage::open(openfrag_storage::Layout::at(data_directory)).map_err(|error| {
        LiveRuntimeError::Unavailable(UnavailableReason::Storage(format!("{error:?}")))
    })?;
    let (scheduler, deadlines) = deadline_channel();
    let runtime = Arc::new(LiveRuntime::durable(
        storage,
        local_steam_id,
        capture,
        clock.clone(),
        scheduler,
    ));
    if runtime.status() != LiveRuntimeStatus::Ready {
        return Err(LiveRuntimeError::State(
            "production live runtime did not become ready".into(),
        ));
    }
    Ok(RuntimeDriver::new(runtime, deadlines, clock))
}
