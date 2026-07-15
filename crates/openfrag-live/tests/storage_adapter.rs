use openfrag_capture::{
    MediaInfo, SaveAcknowledgement, SaveDisposition, SaveProvenance, SaveRequestOutcome,
};
use openfrag_domain::{
    AutoCaptureDecision, CandidateTrigger, HighlightCandidate, TimeRange, auto_capture_window,
    manual_capture_window, merge_candidates,
};
use openfrag_gsi::{EvidenceContext, EvidenceReceipt, PresenceBits, StateOutput, TransitionFact};
use openfrag_live::{
    CandidateRecord, CaptureKind, CaptureRecord, CaptureStatus, EvidenceStore, StorageEvidenceStore,
};
use openfrag_storage::{Layout, Storage};
use rusqlite::Connection;
use std::{collections::BTreeSet, time::Duration};

fn receipt() -> EvidenceReceipt {
    EvidenceReceipt {
        sequence: 7,
        received_at: Duration::from_secs(42),
        payload_hash: "a".repeat(64),
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
            observed_round: Some(4),
            provider_timestamp: Some(1_700_000_000),
            round_kills: Some(3),
            health: Some(87),
        },
        output: StateOutput::Healthy,
        facts: vec![TransitionFact::RoundKillDelta {
            previous: 2,
            current: 3,
            delta: 1,
        }],
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn receipt_candidate_and_save_metadata_survive_reopen() {
    let directory = tempfile::tempdir().expect("temp directory");
    let layout = Layout::at(directory.path().join("data"));
    let adapter = StorageEvidenceStore::new(
        Storage::open(layout.clone()).expect("storage"),
        "76561198000000000",
    )
    .expect("adapter");
    let evidence = receipt();
    adapter.persist_receipt(&evidence).expect("receipt");
    let receipt_id = format!("{}:{}", evidence.payload_hash, evidence.sequence);
    let candidate = HighlightCandidate::new(
        adapter.capture_session_id(),
        "maphash:4",
        TimeRange {
            start_ms: 40_000,
            end_ms: 42_000,
        },
        CandidateTrigger::KillMilestone,
        receipt_id,
    )
    .expect("candidate");
    adapter
        .upsert_candidate(&CandidateRecord {
            candidate: candidate.clone(),
            final_labels: BTreeSet::new(),
        })
        .expect("persist candidate");
    let mut round_end = evidence.clone();
    round_end.sequence = 8;
    round_end.received_at = Duration::from_secs(43);
    round_end.payload_hash = "b".repeat(64);
    round_end.context.observed_round = Some(5);
    round_end.context.round_kills = Some(0);
    round_end.facts = vec![TransitionFact::RoundEnd {
        completed: 4,
        next: 5,
    }];
    adapter.persist_receipt(&round_end).expect("round end");
    let official = HighlightCandidate::new(
        adapter.capture_session_id(),
        "maphash:4",
        TimeRange {
            start_ms: 43_000,
            end_ms: 43_000,
        },
        CandidateTrigger::OfficialRoundEndCapture,
        format!("{}:{}", round_end.payload_hash, round_end.sequence),
    )
    .expect("official candidate");
    let merged = merge_candidates(vec![candidate.clone(), official])
        .pop()
        .expect("merged candidate");
    let AutoCaptureDecision::Save(window) =
        auto_capture_window(candidate.range.start_ms, Some(42_000), 0)
    else {
        panic!("capture window");
    };
    adapter
        .upsert_capture(&CaptureRecord {
            id: "auto:maphash:4".into(),
            kind: CaptureKind::AutoRoundEnd,
            round_id: Some("maphash:4".into()),
            provenance: SaveProvenance::AutoRoundEnd,
            source_receipt_ids: merged.receipt_ids.clone(),
            candidate: Some(merged),
            final_labels: BTreeSet::new(),
            round_end_ms: Some(42_000),
            deadline_ms: Some(52_000),
            timer: None,
            window: Some(window),
            raw_coverage: Some(TimeRange {
                start_ms: 0,
                end_ms: 52_000,
            }),
            status: CaptureStatus::SaveRequested(SaveRequestOutcome {
                recorder_request_id: "auto:maphash:4".into(),
                disposition: SaveDisposition::Signalled,
            }),
        })
        .expect("persist save request");
    drop(adapter);

    Storage::open(layout.clone()).expect("reopen storage");
    let connection = Connection::open(layout.database).expect("read database");
    let snapshots: i64 = connection
        .query_row(
            "SELECT count(*) FROM gsi_snapshots WHERE arrival_ordinal=7 AND received_at_ms=42000 AND listener_version='1.0.0'",
            [],
            |row| row.get(0),
        )
        .expect("snapshot count");
    assert_eq!(snapshots, 1);
    let candidates: i64 = connection
        .query_row(
            "SELECT count(*) FROM live_candidates WHERE observed_kind='kill_milestone' AND observed_map='maphash' AND observed_round=4 AND candidate_start_monotonic_ns=40000000000 AND candidate_end_monotonic_ns=42000000000",
            [],
            |row| row.get(0),
        )
        .expect("candidate count");
    assert_eq!(candidates, 1);
    let triggers: i64 = connection
        .query_row(
            "SELECT count(*) FROM candidate_trigger_receipts WHERE transition_ordinal=7 AND transition_kind='kill_milestone' AND transition_monotonic_ns=42000000000",
            [],
            |row| row.get(0),
        )
        .expect("trigger count");
    assert_eq!(triggers, 1);
    let all_triggers: i64 = connection
        .query_row(
            "SELECT count(*) FROM candidate_trigger_receipts",
            [],
            |row| row.get(0),
        )
        .expect("all trigger count");
    assert_eq!(all_triggers, 2);
    let save: (String, i64, i64, i64) = connection
        .query_row(
            "SELECT a.status,a.requested_monotonic_ns,j.desired_start_monotonic_ns,j.desired_end_monotonic_ns FROM recorder_save_attempts a JOIN candidate_save_attempts j ON j.save_attempt_id=a.id WHERE a.recorder_request_id='auto:maphash:4'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("save metadata");
    assert_eq!(
        save,
        (
            "requested".into(),
            52_000_000_000,
            25_000_000_000,
            52_000_000_000
        )
    );
}

#[test]
fn scheduled_and_cancelled_states_are_idempotent_without_inventing_save_attempts() {
    let directory = tempfile::tempdir().expect("temp directory");
    let adapter = StorageEvidenceStore::new(
        Storage::open(Layout::at(directory.path())).expect("storage"),
        "76561198000000000",
    )
    .expect("adapter");
    let result = adapter.upsert_capture(&CaptureRecord {
        id: "scheduled".into(),
        kind: CaptureKind::AutoRoundEnd,
        round_id: Some("maphash:4".into()),
        provenance: SaveProvenance::AutoRoundEnd,
        source_receipt_ids: BTreeSet::new(),
        candidate: None,
        final_labels: BTreeSet::new(),
        round_end_ms: Some(42_000),
        deadline_ms: Some(52_000),
        timer: None,
        window: None,
        raw_coverage: None,
        status: CaptureStatus::Scheduled,
    });
    assert_eq!(result, Ok(()));
    let cancelled = CaptureRecord {
        status: CaptureStatus::Cancelled(openfrag_live::CancelReason::Stale),
        ..CaptureRecord {
            id: "scheduled".into(),
            kind: CaptureKind::AutoRoundEnd,
            round_id: Some("maphash:4".into()),
            provenance: SaveProvenance::AutoRoundEnd,
            source_receipt_ids: BTreeSet::new(),
            candidate: None,
            final_labels: BTreeSet::new(),
            round_end_ms: Some(42_000),
            deadline_ms: Some(52_000),
            timer: None,
            window: None,
            raw_coverage: None,
            status: CaptureStatus::Scheduled,
        }
    };
    assert_eq!(adapter.upsert_capture(&cancelled), Ok(()));
}

#[test]
fn retry_and_coalesced_transitions_wait_for_real_media_acknowledgement() {
    let directory = tempfile::tempdir().expect("temp directory");
    let layout = Layout::at(directory.path());
    let adapter = StorageEvidenceStore::new(
        Storage::open(layout.clone()).expect("storage"),
        "76561198000000000",
    )
    .expect("adapter");
    let window = manual_capture_window(42_000, 0);
    let mut capture = CaptureRecord {
        id: "auto:retry".into(),
        kind: CaptureKind::AutoRoundEnd,
        round_id: Some("maphash:4".into()),
        provenance: SaveProvenance::AutoRoundEnd,
        source_receipt_ids: BTreeSet::new(),
        candidate: None,
        final_labels: BTreeSet::new(),
        round_end_ms: Some(42_000),
        deadline_ms: Some(52_000),
        timer: None,
        window: Some(window),
        raw_coverage: Some(TimeRange {
            start_ms: 0,
            end_ms: 42_000,
        }),
        status: CaptureStatus::Retryable("recorder_busy".into()),
    };
    adapter.upsert_capture(&capture).expect("retryable");
    capture.status = CaptureStatus::SaveRequested(SaveRequestOutcome {
        recorder_request_id: "auto:retry:coalesced".into(),
        disposition: SaveDisposition::Coalesced,
    });
    adapter.upsert_capture(&capture).expect("coalesced retry");
    drop(adapter);
    let connection = Connection::open(layout.database).expect("database");
    let statuses = connection
        .prepare("SELECT status FROM recorder_save_attempts ORDER BY recorder_request_id")
        .expect("statement")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("statuses");
    assert_eq!(statuses, ["failed", "requested"]);
}

#[test]
fn manual_request_persists_flag_attempt_and_durable_join() {
    let directory = tempfile::tempdir().expect("temp directory");
    let layout = Layout::at(directory.path());
    let adapter = StorageEvidenceStore::new(
        Storage::open(layout.clone()).expect("storage"),
        "76561198000000000",
    )
    .expect("adapter");
    let window = manual_capture_window(42_000, 0);
    let mut capture = CaptureRecord {
        id: "manual:42000:1".into(),
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
        raw_coverage: Some(TimeRange {
            start_ms: 0,
            end_ms: 42_000,
        }),
        status: CaptureStatus::Requesting,
    };
    assert_eq!(adapter.upsert_capture(&capture), Ok(()));
    capture.status = CaptureStatus::SaveRequested(SaveRequestOutcome {
        recorder_request_id: "manual-recorder-save-1".into(),
        disposition: SaveDisposition::Signalled,
    });
    assert_eq!(adapter.upsert_capture(&capture), Ok(()));
    drop(adapter);
    let connection = Connection::open(layout.database).expect("database");
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM manual_flags", [], |row| row
                .get::<_, i64>(0))
            .expect("flags"),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM recorder_save_attempts", [], |row| row
                .get::<_, i64>(0))
            .expect("attempts"),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM manual_flag_save_attempts",
                [],
                |row| row.get::<_, i64>(0)
            )
            .expect("joins"),
        1
    );
}

#[test]
fn one_acknowledged_recorder_request_finalizes_one_durable_clip() {
    let directory = tempfile::tempdir().expect("temp directory");
    let layout = Layout::at(directory.path().join("data"));
    let output = directory.path().join("captured.mkv");
    std::fs::write(&output, b"verified captured media").expect("captured media");
    let adapter = StorageEvidenceStore::new(
        Storage::open(layout.clone()).expect("storage"),
        "76561198000000000",
    )
    .expect("adapter");
    let outcome = SaveRequestOutcome {
        recorder_request_id: "recorder-save-1".into(),
        disposition: SaveDisposition::Signalled,
    };
    let window = manual_capture_window(42_000, 0);
    for id in ["auto:first", "auto:coalesced"] {
        adapter
            .upsert_capture(&CaptureRecord {
                id: id.into(),
                kind: CaptureKind::AutoRoundEnd,
                round_id: None,
                provenance: SaveProvenance::AutoRoundEnd,
                source_receipt_ids: BTreeSet::new(),
                candidate: None,
                final_labels: BTreeSet::new(),
                round_end_ms: Some(42_000),
                deadline_ms: Some(52_000),
                timer: None,
                window: Some(window),
                raw_coverage: Some(TimeRange {
                    start_ms: 0,
                    end_ms: 52_000,
                }),
                status: CaptureStatus::SaveRequested(outcome.clone()),
            })
            .expect("save request");
    }

    let finalized = adapter
        .finalize_capture(&SaveAcknowledgement {
            path: output.clone(),
            recorder_request_id: "recorder-save-1".into(),
            provenance: SaveProvenance::AutoRoundEnd,
            requested_at_ms: 52_000,
            media: MediaInfo {
                duration_ms: 60_000,
                video_streams: 1,
            },
        })
        .expect("finalized Clip");
    assert!(!finalized.clip_id.is_empty());
    assert!(!output.exists());
    assert_eq!(
        Storage::open(layout)
            .expect("reopen storage")
            .list_clips()
            .expect("Clips")
            .len(),
        1
    );
}
