use openfrag_capture::{SaveDisposition, SaveProvenance};
use openfrag_domain::{
    AUTO_POST_ROLL_MS, CandidateTrigger, DESIRED_PRE_ROLL_MS, FinalHighlightLabel,
    REPLAY_BUFFER_MS, TimeRange,
};
use openfrag_gsi::{EvidenceContext, EvidenceReceipt, PresenceBits, StateOutput, TransitionFact};
use openfrag_live::{
    CancelReason, CaptureKind, CaptureRecord, CaptureStatus, Clock, Coordinator, EvidenceStore,
    LiveDiagnostic, Recorder, Scheduler, TimerId, TimerOutcome,
};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const MAP_HASH: &str = "f99f33e2882aa01d4d3060562bd3f088bc74086e344c1dbbf1862d0239ef4954";

#[derive(Default)]
struct MemoryStore {
    receipts: Mutex<Vec<EvidenceReceipt>>,
    candidates: Mutex<HashMap<String, openfrag_live::CandidateRecord>>,
    captures: Mutex<HashMap<String, CaptureRecord>>,
}

impl EvidenceStore for MemoryStore {
    fn persist_receipt(&self, receipt: &EvidenceReceipt) -> Result<(), LiveDiagnostic> {
        self.receipts
            .lock()
            .expect("receipts lock")
            .push(receipt.clone());
        Ok(())
    }

    fn upsert_candidate(
        &self,
        candidate: &openfrag_live::CandidateRecord,
    ) -> Result<(), LiveDiagnostic> {
        self.candidates
            .lock()
            .expect("candidates lock")
            .insert(candidate.candidate.round_id.clone(), candidate.clone());
        Ok(())
    }

    fn upsert_capture(&self, capture: &CaptureRecord) -> Result<(), LiveDiagnostic> {
        self.captures
            .lock()
            .expect("captures lock")
            .insert(capture.id.clone(), capture.clone());
        Ok(())
    }
}

#[derive(Default)]
struct ManualClock(AtomicU64);

impl ManualClock {
    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct FakeScheduler {
    scheduled: Mutex<Vec<(TimerId, u64)>>,
    cancelled: Mutex<Vec<TimerId>>,
}

impl Scheduler for FakeScheduler {
    fn schedule(&self, timer: &TimerId, deadline_ms: u64) -> Result<(), LiveDiagnostic> {
        self.scheduled
            .lock()
            .expect("scheduled lock")
            .push((timer.clone(), deadline_ms));
        Ok(())
    }

    fn cancel(&self, timer: &TimerId) -> Result<(), LiveDiagnostic> {
        self.cancelled
            .lock()
            .expect("cancelled lock")
            .push(timer.clone());
        Ok(())
    }
}

struct FakeRecorder {
    available_from_ms: u64,
    responses: Mutex<VecDeque<Result<SaveDisposition, String>>>,
    calls: Mutex<Vec<SaveProvenance>>,
}

impl FakeRecorder {
    fn new(available_from_ms: u64) -> Self {
        Self {
            available_from_ms,
            responses: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn respond(&self, response: Result<SaveDisposition, &str>) {
        self.responses
            .lock()
            .expect("responses lock")
            .push_back(response.map_err(str::to_owned));
    }
}

impl Recorder for FakeRecorder {
    fn available_from_ms(&self) -> u64 {
        self.available_from_ms
    }

    fn request_save(&self, provenance: SaveProvenance) -> Result<SaveDisposition, String> {
        self.calls.lock().expect("calls lock").push(provenance);
        self.responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .unwrap_or(Ok(SaveDisposition::Signalled))
    }
}

type TestCoordinator = Coordinator<MemoryStore, FakeRecorder, ManualClock, FakeScheduler>;

fn harness(
    recorder_available_from_ms: u64,
) -> (
    TestCoordinator,
    Arc<MemoryStore>,
    Arc<FakeRecorder>,
    Arc<ManualClock>,
    Arc<FakeScheduler>,
) {
    let store = Arc::new(MemoryStore::default());
    let recorder = Arc::new(FakeRecorder::new(recorder_available_from_ms));
    let clock = Arc::new(ManualClock::default());
    let scheduler = Arc::new(FakeScheduler::default());
    let coordinator = Coordinator::new(
        "capture-session-1",
        store.clone(),
        recorder.clone(),
        clock.clone(),
        scheduler.clone(),
    );
    (coordinator, store, recorder, clock, scheduler)
}

fn receipt(
    sequence: u64,
    at_ms: u64,
    output: StateOutput,
    observed_round: Option<u64>,
    round_kills: Option<i64>,
    facts: Vec<TransitionFact>,
) -> EvidenceReceipt {
    EvidenceReceipt {
        sequence,
        received_at: Duration::from_millis(at_ms),
        payload_hash: format!("payload-{sequence}"),
        presence: PresenceBits {
            provider: true,
            provider_timestamp: true,
            map: observed_round.is_some(),
            map_round: observed_round.is_some(),
            player: true,
            player_state: true,
            player_health: true,
            player_round_kills: round_kills.is_some(),
            player_weapons: false,
            match_stats: true,
            auth: true,
            auth_token: true,
            player_steamid: true,
        },
        context: EvidenceContext {
            map_hash: Some(MAP_HASH.into()),
            observed_round,
            provider_timestamp: Some(i64::try_from(sequence).expect("sequence fits")),
            round_kills,
            health: Some(100),
        },
        output,
        facts,
    }
}

fn round_kill_receipt(
    sequence: u64,
    at_ms: u64,
    round: u64,
    previous: i64,
    current: i64,
) -> EvidenceReceipt {
    receipt(
        sequence,
        at_ms,
        StateOutput::Healthy,
        Some(round),
        Some(current),
        vec![TransitionFact::RoundKillDelta {
            previous,
            current,
            delta: current - previous,
        }],
    )
}

fn round_end_receipt(sequence: u64, at_ms: u64, completed: u64) -> EvidenceReceipt {
    receipt(
        sequence,
        at_ms,
        StateOutput::Healthy,
        Some(completed + 1),
        Some(0),
        vec![TransitionFact::RoundEnd {
            completed,
            next: completed + 1,
        }],
    )
}

#[test]
fn persists_every_receipt_but_deduplicates_the_same_round_candidate() {
    let (mut coordinator, store, _recorder, _clock, _scheduler) = harness(0);
    let crossing = round_kill_receipt(1, 20_000, 3, 2, 3);

    coordinator.ingest(&crossing).expect("first receipt");
    coordinator.ingest(&crossing).expect("repeated receipt");

    assert_eq!(store.receipts.lock().expect("receipts lock").len(), 2);
    let candidates = store.candidates.lock().expect("candidates lock");
    assert_eq!(candidates.len(), 1);
    let candidate = candidates.values().next().expect("candidate");
    assert_eq!(
        candidate.candidate.triggers,
        BTreeSet::from([CandidateTrigger::KillMilestone])
    );
    assert!(candidate.final_labels.is_empty());
}

#[test]
fn jump_is_one_candidate_and_round_reset_does_not_invent_another() {
    let (mut coordinator, store, _recorder, _clock, _scheduler) = harness(0);
    let mut jump = round_kill_receipt(1, 20_000, 3, 1, 4);
    jump.facts.extend([
        TransitionFact::CumulativeKillDelta {
            previous: 1,
            current: 4,
            delta: 3,
        },
        TransitionFact::HealthDepleted {
            previous: 100,
            current: 0,
        },
    ]);
    coordinator.ingest(&jump).expect("jump receipt");
    coordinator
        .ingest(&receipt(
            2,
            30_000,
            StateOutput::Healthy,
            Some(4),
            Some(0),
            vec![],
        ))
        .expect("reset receipt");

    assert_eq!(store.candidates.lock().expect("candidates lock").len(), 1);
}

#[test]
fn every_round_end_schedules_a_capture_even_with_zero_kills() {
    let (mut coordinator, store, _recorder, _clock, scheduler) = harness(0);

    coordinator
        .ingest(&round_end_receipt(1, 40_000, 3))
        .expect("round end");

    let captures = store.captures.lock().expect("captures lock");
    assert_eq!(captures.len(), 1);
    let capture = captures.values().next().expect("capture");
    assert_eq!(capture.kind, CaptureKind::AutoRoundEnd);
    assert_eq!(capture.deadline_ms, Some(40_000 + AUTO_POST_ROLL_MS));
    assert_eq!(capture.status, CaptureStatus::Scheduled);
    assert!(capture.final_labels.is_empty());
    assert!(
        capture
            .candidate
            .as_ref()
            .expect("official candidate")
            .triggers
            .contains(&CandidateTrigger::OfficialRoundEndCapture)
    );
    assert_eq!(scheduler.scheduled.lock().expect("scheduled lock").len(), 1);
}

#[test]
fn missing_map_or_round_context_persists_receipt_without_live_inference() {
    let (mut coordinator, store, _recorder, _clock, scheduler) = harness(0);
    let mut missing = round_kill_receipt(1, 10_000, 3, 2, 3);
    missing.context.map_hash = None;
    missing.facts.push(TransitionFact::RoundEnd {
        completed: 3,
        next: 4,
    });

    coordinator.ingest(&missing).expect("receipt persists");

    assert_eq!(store.receipts.lock().expect("receipts lock").len(), 1);
    assert!(store.candidates.lock().expect("candidates lock").is_empty());
    assert!(store.captures.lock().expect("captures lock").is_empty());
    assert!(
        scheduler
            .scheduled
            .lock()
            .expect("scheduled lock")
            .is_empty()
    );
}

#[test]
fn timer_boundary_requests_one_save_with_honest_desired_and_raw_coverage() {
    let (mut coordinator, store, recorder, clock, _scheduler) = harness(30_000);
    coordinator
        .ingest(&round_kill_receipt(1, 20_000, 3, 2, 3))
        .expect("candidate");
    coordinator
        .ingest(&round_end_receipt(2, 70_000, 3))
        .expect("round end");
    let timer = store
        .captures
        .lock()
        .expect("captures lock")
        .values()
        .next()
        .expect("capture")
        .timer
        .clone()
        .expect("timer");

    clock.set(79_999);
    assert_eq!(coordinator.on_timer(&timer), Ok(TimerOutcome::NotDue));
    assert!(recorder.calls.lock().expect("calls lock").is_empty());
    clock.set(80_000);
    assert_eq!(
        coordinator.on_timer(&timer),
        Ok(TimerOutcome::SaveRequested)
    );
    assert_eq!(
        coordinator.on_timer(&timer),
        Ok(TimerOutcome::AlreadyHandled)
    );

    assert_eq!(
        recorder.calls.lock().expect("calls lock").as_slice(),
        &[SaveProvenance::AutoRoundEnd]
    );
    let captures = store.captures.lock().expect("captures lock");
    let capture = captures.values().next().expect("capture");
    let window = capture.window.expect("capture window");
    assert_eq!(
        window.desired,
        TimeRange {
            start_ms: 5_000,
            end_ms: 80_000
        }
    );
    assert_eq!(
        window.available,
        TimeRange {
            start_ms: 30_000,
            end_ms: 80_000
        }
    );
    assert!(window.pre_roll_truncated);
    assert_eq!(
        capture.raw_coverage,
        Some(TimeRange {
            start_ms: 30_000.max(80_000 - REPLAY_BUFFER_MS),
            end_ms: 80_000,
        })
    );
}

#[test]
fn overlapping_kill_and_round_end_candidates_merge_without_provisional_labels() {
    let (mut coordinator, store, _recorder, _clock, _scheduler) = harness(0);
    coordinator
        .ingest(&round_kill_receipt(1, 50_000, 3, 2, 3))
        .expect("candidate");
    coordinator
        .ingest(&round_end_receipt(2, 58_000, 3))
        .expect("round end");

    let captures = store.captures.lock().expect("captures lock");
    let capture = captures.values().next().expect("capture");
    let candidate = capture.candidate.as_ref().expect("merged candidate");
    assert_eq!(
        candidate.range,
        TimeRange {
            start_ms: 50_000,
            end_ms: 58_000
        }
    );
    assert_eq!(
        candidate.triggers,
        BTreeSet::from([
            CandidateTrigger::KillMilestone,
            CandidateTrigger::OfficialRoundEndCapture,
        ])
    );
    assert_eq!(candidate.receipt_ids.len(), 2);
    assert!(capture.final_labels.is_empty());
}

#[test]
fn stale_and_reset_cancel_unsaved_deadlines_but_retain_metadata() {
    let (mut coordinator, store, _recorder, _clock, scheduler) = harness(0);
    coordinator
        .ingest(&round_end_receipt(1, 40_000, 3))
        .expect("first round end");
    coordinator
        .ingest(&receipt(
            2,
            41_000,
            StateOutput::Stale,
            Some(4),
            Some(0),
            vec![],
        ))
        .expect("stale");
    let first = store
        .captures
        .lock()
        .expect("captures lock")
        .values()
        .next()
        .expect("capture")
        .clone();
    assert_eq!(first.status, CaptureStatus::Cancelled(CancelReason::Stale));

    coordinator
        .ingest(&round_end_receipt(3, 50_000, 4))
        .expect("second round end");
    coordinator
        .ingest(&receipt(
            4,
            51_000,
            StateOutput::SessionReset,
            Some(1),
            Some(0),
            vec![],
        ))
        .expect("reset");

    let captures = store.captures.lock().expect("captures lock");
    assert_eq!(captures.len(), 2);
    assert!(
        captures
            .values()
            .any(|capture| capture.status == CaptureStatus::Cancelled(CancelReason::SessionReset))
    );
    assert_eq!(scheduler.cancelled.lock().expect("cancelled lock").len(), 2);
}

#[test]
fn recorder_failure_remains_retryable_and_retry_preserves_capture_identity() {
    let (mut coordinator, store, recorder, clock, _scheduler) = harness(0);
    recorder.respond(Err("recorder unavailable"));
    recorder.respond(Ok(SaveDisposition::Signalled));
    coordinator
        .ingest(&round_end_receipt(1, 40_000, 3))
        .expect("round end");
    let capture = store
        .captures
        .lock()
        .expect("captures lock")
        .values()
        .next()
        .expect("capture")
        .clone();
    let timer = capture.timer.clone().expect("timer");
    clock.set(50_000);

    assert_eq!(
        coordinator.on_timer(&timer),
        Err(LiveDiagnostic::Recorder("recorder unavailable".into()))
    );
    assert!(matches!(
        store
            .captures
            .lock()
            .expect("captures lock")
            .get(&capture.id)
            .expect("capture")
            .status,
        CaptureStatus::Retryable(_)
    ));

    assert_eq!(
        coordinator.retry_capture(&capture.id),
        Ok(TimerOutcome::SaveRequested)
    );
    assert_eq!(recorder.calls.lock().expect("calls lock").len(), 2);
    assert!(matches!(
        store
            .captures
            .lock()
            .expect("captures lock")
            .get(&capture.id)
            .expect("capture")
            .status,
        CaptureStatus::SaveRequested(SaveDisposition::Signalled)
    ));
}

#[test]
fn manual_flag_is_immediate_and_retains_identity_when_recorder_coalesces() {
    let (mut coordinator, store, recorder, clock, _scheduler) = harness(40_000);
    recorder.respond(Ok(SaveDisposition::Signalled));
    recorder.respond(Ok(SaveDisposition::Coalesced));
    coordinator
        .ingest(&round_end_receipt(1, 40_000, 3))
        .expect("round end");
    let auto_timer = store
        .captures
        .lock()
        .expect("captures lock")
        .values()
        .next()
        .expect("auto capture")
        .timer
        .clone()
        .expect("timer");
    clock.set(50_000);
    coordinator.on_timer(&auto_timer).expect("auto save");

    let manual_id = coordinator.manual_flag().expect("manual flag");

    let captures = store.captures.lock().expect("captures lock");
    let manual = captures.get(&manual_id).expect("manual capture");
    assert_eq!(manual.kind, CaptureKind::ManualFlag);
    assert_eq!(manual.provenance, SaveProvenance::ManualFlag);
    assert_eq!(
        manual.status,
        CaptureStatus::SaveRequested(SaveDisposition::Coalesced)
    );
    assert_eq!(
        manual.window.expect("manual window").desired,
        TimeRange {
            start_ms: 50_000 - DESIRED_PRE_ROLL_MS,
            end_ms: 50_000,
        }
    );
    assert_eq!(
        manual.raw_coverage,
        Some(TimeRange {
            start_ms: 40_000,
            end_ms: 50_000,
        })
    );
    assert!(manual.final_labels.is_empty());
    assert_eq!(recorder.calls.lock().expect("calls lock").len(), 2);
}

#[test]
fn provisional_records_never_contain_demo_only_labels() {
    let forbidden = BTreeSet::from([
        FinalHighlightLabel::Ace,
        FinalHighlightLabel::Clutch,
        FinalHighlightLabel::Knife,
    ]);
    let (mut coordinator, store, _recorder, _clock, _scheduler) = harness(0);
    coordinator
        .ingest(&round_kill_receipt(1, 20_000, 3, 2, 3))
        .expect("candidate");
    coordinator
        .ingest(&round_end_receipt(2, 30_000, 3))
        .expect("capture");

    for candidate in store.candidates.lock().expect("candidates lock").values() {
        assert!(candidate.final_labels.is_disjoint(&forbidden));
    }
    for capture in store.captures.lock().expect("captures lock").values() {
        assert!(capture.final_labels.is_disjoint(&forbidden));
    }
}
