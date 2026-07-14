#[path = "../src/live_runtime.rs"]
mod live_runtime;

use live_runtime::{
    CaptureRuntimePort, LiveRuntime, LiveRuntimeError, LiveRuntimeStatus, MonotonicClock,
    UnavailableReason, deadline_channel,
};
use openfrag_capture::{SaveDisposition, SaveProvenance};
use openfrag_gsi::{
    EventSink, EvidenceContext, EvidenceReceipt, PresenceBits, StateOutput, TransitionFact,
};
use openfrag_live::{
    CandidateRecord, CaptureRecord, Clock, EvidenceStore, LiveDiagnostic, Scheduler, TimerId,
    TimerOutcome,
};
use openfrag_storage::{Layout, Storage};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[test]
fn monotonic_clock_clones_share_one_non_decreasing_epoch() {
    let clock = MonotonicClock::default();
    let clone = clock.clone();
    let before = clock.now_ms();
    std::thread::sleep(Duration::from_millis(2));
    assert!(clone.now_ms() >= before);
    assert!(clock.now_ms() >= before);
}

#[test]
fn channel_scheduler_reschedules_cancels_and_yields_due_ids() {
    let (scheduler, mut driver) = deadline_channel();
    let late = TimerId::new("late");
    let first = TimerId::new("first");
    let cancelled = TimerId::new("cancelled");
    scheduler.schedule(&late, 300).expect("schedule late");
    scheduler.schedule(&first, 100).expect("schedule first");
    scheduler
        .schedule(&cancelled, 200)
        .expect("schedule cancelled");
    scheduler.cancel(&cancelled).expect("cancel timer");
    scheduler.schedule(&late, 150).expect("reschedule late");

    assert_eq!(driver.next_deadline_ms(), Some(100));
    assert_eq!(
        driver.take_due(150),
        vec![TimerId::new("first"), TimerId::new("late")]
    );
    assert!(driver.take_due(1_000).is_empty());
}

#[test]
fn deadline_driver_waits_without_owning_the_runtime() {
    let (scheduler, mut driver) = deadline_channel();
    let clock = MonotonicClock::default();
    let deadline = clock.now_ms();
    scheduler
        .schedule(&TimerId::new("immediate-b"), deadline)
        .expect("schedule immediate b");
    scheduler
        .schedule(&TimerId::new("immediate-a"), deadline)
        .expect("schedule immediate a");
    assert_eq!(
        driver.wait_next_due(&clock),
        Some(TimerId::new("immediate-a"))
    );
    assert_eq!(
        driver.wait_next_due(&clock),
        Some(TimerId::new("immediate-b"))
    );
    drop(driver);
    assert!(matches!(
        scheduler.schedule(&TimerId::new("orphaned"), 0),
        Err(LiveDiagnostic::Scheduler(_))
    ));
}

#[derive(Default)]
struct MemoryStore {
    receipts: Mutex<Vec<EvidenceReceipt>>,
    candidates: Mutex<Vec<CandidateRecord>>,
    captures: Mutex<Vec<CaptureRecord>>,
}

impl EvidenceStore for MemoryStore {
    fn persist_receipt(&self, receipt: &EvidenceReceipt) -> Result<(), LiveDiagnostic> {
        self.receipts
            .lock()
            .expect("receipts")
            .push(receipt.clone());
        Ok(())
    }

    fn upsert_candidate(&self, candidate: &CandidateRecord) -> Result<(), LiveDiagnostic> {
        self.candidates
            .lock()
            .expect("candidates")
            .push(candidate.clone());
        Ok(())
    }

    fn upsert_capture(&self, capture: &CaptureRecord) -> Result<(), LiveDiagnostic> {
        self.captures
            .lock()
            .expect("captures")
            .push(capture.clone());
        Ok(())
    }
}

struct FakeCapture {
    ready: Result<(), String>,
    available_from_ms: u64,
    saves: Arc<Mutex<Vec<SaveProvenance>>>,
}

impl CaptureRuntimePort for FakeCapture {
    fn readiness(&self) -> Result<(), String> {
        self.ready.clone()
    }

    fn available_from_ms(&self) -> u64 {
        self.available_from_ms
    }

    fn request_save(&mut self, provenance: SaveProvenance) -> Result<SaveDisposition, String> {
        self.saves.lock().expect("saves").push(provenance);
        Ok(SaveDisposition::Signalled)
    }
}

#[derive(Clone, Default)]
struct FakeClock(Arc<AtomicU64>);

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct FakeScheduler {
    scheduled: Mutex<HashMap<String, u64>>,
}

impl Scheduler for FakeScheduler {
    fn schedule(&self, timer: &TimerId, deadline_ms: u64) -> Result<(), LiveDiagnostic> {
        self.scheduled
            .lock()
            .expect("scheduled")
            .insert(timer.as_str().into(), deadline_ms);
        Ok(())
    }

    fn cancel(&self, timer: &TimerId) -> Result<(), LiveDiagnostic> {
        self.scheduled
            .lock()
            .expect("scheduled")
            .remove(timer.as_str());
        Ok(())
    }
}

fn receipt(sequence: u64, facts: Vec<TransitionFact>) -> EvidenceReceipt {
    EvidenceReceipt {
        sequence,
        received_at: Duration::from_secs(1),
        payload_hash: format!("{sequence:064x}"),
        presence: PresenceBits {
            provider: true,
            provider_timestamp: true,
            map: true,
            map_round: true,
            player: true,
            player_state: true,
            player_health: true,
            player_round_kills: true,
            player_weapons: false,
            match_stats: true,
            auth: false,
            auth_token: false,
            player_steamid: false,
        },
        context: EvidenceContext {
            map_hash: Some("maphash".into()),
            observed_round: Some(2),
            provider_timestamp: Some(100),
            round_kills: Some(0),
            health: Some(100),
        },
        output: StateOutput::Healthy,
        facts,
    }
}

fn fake_capture(saves: Arc<Mutex<Vec<SaveProvenance>>>) -> FakeCapture {
    FakeCapture {
        ready: Ok(()),
        available_from_ms: 0,
        saves,
    }
}

#[test]
fn event_sink_timer_and_manual_flag_share_one_coordinator() {
    let saves = Arc::new(Mutex::new(Vec::new()));
    let clock = FakeClock::default();
    let now = clock.0.clone();
    clock.0.store(1_000, Ordering::SeqCst);
    let runtime = LiveRuntime::with_store(
        "fake-session",
        MemoryStore::default(),
        fake_capture(saves.clone()),
        clock,
        FakeScheduler::default(),
    );
    EventSink::emit(
        &runtime,
        receipt(
            1,
            vec![TransitionFact::RoundEnd {
                completed: 1,
                next: 2,
            }],
        ),
    )
    .expect("event sink receipt");
    now.store(11_000, Ordering::SeqCst);
    assert_eq!(
        runtime.dispatch_timer("timer:auto:maphash:1"),
        Ok(TimerOutcome::SaveRequested)
    );
    let manual_id = runtime.manual_flag().expect("manual flag");
    assert!(manual_id.starts_with("manual:11000:"));
    assert_eq!(
        saves.lock().expect("saves").as_slice(),
        &[SaveProvenance::AutoRoundEnd, SaveProvenance::ManualFlag]
    );
}

#[test]
fn unavailable_capture_and_runtime_errors_are_explicit_diagnostics() {
    let runtime = LiveRuntime::with_store(
        "fake-session",
        MemoryStore::default(),
        FakeCapture {
            ready: Err("capture disabled".into()),
            available_from_ms: 0,
            saves: Arc::new(Mutex::new(Vec::new())),
        },
        FakeClock::default(),
        FakeScheduler::default(),
    );
    assert_eq!(
        runtime.status(),
        LiveRuntimeStatus::Unavailable(UnavailableReason::Capture("capture disabled".into()))
    );
    assert_eq!(
        runtime.manual_flag(),
        Err(LiveRuntimeError::Unavailable(UnavailableReason::Capture(
            "capture disabled".into()
        )))
    );

    let runtime = LiveRuntime::with_store(
        "fake-session",
        MemoryStore::default(),
        fake_capture(Arc::new(Mutex::new(Vec::new()))),
        FakeClock::default(),
        FakeScheduler::default(),
    );
    assert!(matches!(
        runtime.dispatch_timer("missing"),
        Err(LiveRuntimeError::Diagnostic(
            LiveDiagnostic::MissingCapture(_)
        ))
    ));
    assert_eq!(runtime.diagnostics()[0].operation, "timer_dispatch");
    assert!(matches!(
        runtime.retry_capture("missing"),
        Err(LiveRuntimeError::Diagnostic(
            LiveDiagnostic::MissingCapture(_)
        ))
    ));
    assert_eq!(runtime.diagnostics()[1].operation, "capture_retry");
}

#[test]
fn durable_constructor_composes_the_sqlite_evidence_store() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = LiveRuntime::durable(
        Storage::open(Layout::at(directory.path())).expect("storage"),
        "76561198000000000",
        fake_capture(Arc::new(Mutex::new(Vec::new()))),
        FakeClock::default(),
        FakeScheduler::default(),
    );
    assert_eq!(runtime.status(), LiveRuntimeStatus::Ready);
    runtime
        .ingest_receipt(&receipt(9, Vec::new()))
        .expect("durable receipt");
}
