use openfrag_import::{
    CalculationIdentity, DemoMetadata, EventReceipt, ParsedEvent, ParsedOutput, ParsedRound,
    Participant, PlayerSnapshot, SnapshotPhase,
};
use openfrag_pipeline::{
    ImportOutcome, ImportRequest, ImportService, ParserBackend, PipelineError,
    reconcile::{
        CandidateOrigin, ReconciliationCandidate, ReconciliationDecision,
        ReconciliationDisposition, ReconciliationPersistence, UnavailableEvidence,
    },
};
use openfrag_storage::{Layout, Storage};
use serde_json::json;
use std::collections::BTreeMap;
use std::{fs, path::Path};

#[derive(Clone)]
struct FixtureParser(ParsedOutput);

impl ParserBackend for FixtureParser {
    fn parse(&self, _: &Path) -> Result<ParsedOutput, String> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct MemoryReconciliation(Vec<ReconciliationDecision>);

impl ReconciliationPersistence for MemoryReconciliation {
    type Error = &'static str;

    fn persist_reconciliation(
        &mut self,
        decision: &ReconciliationDecision,
    ) -> Result<(), Self::Error> {
        self.0.push(decision.clone());
        Ok(())
    }
}

struct PostAnalysisPersistence {
    database: std::path::PathBuf,
    decisions: Vec<ReconciliationDecision>,
}

impl ReconciliationPersistence for PostAnalysisPersistence {
    type Error = &'static str;

    fn persist_reconciliation(
        &mut self,
        decision: &ReconciliationDecision,
    ) -> Result<(), Self::Error> {
        let connection = rusqlite::Connection::open(&self.database).map_err(|_| "open")?;
        let completed = connection
            .query_row(
                "SELECT count(*) FROM analysis_runs WHERE status='succeeded'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| "query")?;
        if completed == 0 {
            return Err("reconciliation ran before analysis commit");
        }
        self.decisions.push(decision.clone());
        Ok(())
    }
}

fn auto_candidate(id: &str, round: Option<u64>) -> ReconciliationCandidate {
    ReconciliationCandidate {
        candidate_id: id.into(),
        clip_id: format!("clip-{id}"),
        demo_round_number: round,
        origin: CandidateOrigin::ProvisionalAutoRound,
    }
}

fn fixture_output() -> ParsedOutput {
    let mut parsed = openfrag_analysis::fixtures::minimal_parsed_output_without_tick_rate();
    parsed.participants.push(Participant {
        steam_id: 76_561_197_964_020_430,
        name: Some("local".into()),
        team: Some(2),
    });
    parsed.metadata.map = Some("de_mirage".into());
    parsed
}

#[allow(clippy::too_many_lines)]
fn available_output() -> ParsedOutput {
    let local = 76_561_197_964_020_430_u64;
    let participants = (0_u64..10)
        .map(|index| Participant {
            steam_id: if index == 0 { local } else { local + index },
            name: Some(format!("p{index}")),
            team: Some(if index < 5 { 2 } else { 3 }),
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    let mut snapshots = Vec::new();
    let mut rounds = Vec::new();
    let mut ordinal = 0_u64;
    let mut snapshot_ordinal = 0_u64;
    for round in 0_i32..12 {
        let base = round * 1_000;
        for participant in &participants {
            snapshots.push(PlayerSnapshot {
                tick: base + 10,
                ingestion_ordinal: snapshot_ordinal,
                phase: SnapshotPhase::RequestedTick,
                steam_id: participant.steam_id,
                entity_id: None,
                team: participant.team,
                health: Some(100),
                alive: Some(true),
                life_state: Some(0),
                round_counter: Some(round),
                raw_properties: BTreeMap::new(),
            });
            snapshot_ordinal += 1;
        }
        let mut push = |name: &str, tick: i32, raw_fields: BTreeMap<String, serde_json::Value>| {
            events.push(ParsedEvent {
                name: name.into(),
                tick,
                ingestion_ordinal: ordinal,
                fields: raw_fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_string()))
                    .collect(),
                raw_fields,
            });
            ordinal += 1;
        };
        push(
            "round_freeze_end",
            base + 10,
            BTreeMap::from([("warmup".into(), json!(false))]),
        );
        push(
            "player_hurt",
            base + 20,
            BTreeMap::from([
                ("attacker_steamid".into(), json!(local.to_string())),
                ("user_steamid".into(), json!((local + 5).to_string())),
                ("dmg_health".into(), json!(80)),
                ("weapon".into(), json!("ak47")),
            ]),
        );
        push(
            "player_death",
            base + 30,
            BTreeMap::from([
                ("attacker_steamid".into(), json!((local + 6).to_string())),
                ("user_steamid".into(), json!((local + 1).to_string())),
                ("weapon".into(), json!("ak47")),
                ("assistedflash".into(), json!(false)),
            ]),
        );
        push(
            "round_end",
            base + 90,
            BTreeMap::from([("winner".into(), json!(2))]),
        );
        for participant in &participants {
            snapshots.push(PlayerSnapshot {
                tick: base + 90,
                ingestion_ordinal: snapshot_ordinal,
                phase: SnapshotPhase::RequestedTick,
                steam_id: participant.steam_id,
                entity_id: None,
                team: participant.team,
                health: Some(100),
                alive: Some(true),
                life_state: Some(0),
                round_counter: Some(round + 1),
                raw_properties: BTreeMap::new(),
            });
            snapshot_ordinal += 1;
        }
        rounds.push(ParsedRound {
            number: u64::try_from(round + 1).unwrap(),
            end_tick: base + 90,
            winner: Some("2".into()),
        });
    }
    let receipts = events
        .iter()
        .map(|event| EventReceipt {
            ingestion_ordinal: event.ingestion_ordinal,
            event_name: event.name.clone(),
            tick: event.tick,
            fields: event.fields.clone(),
            raw_fields: event.raw_fields.clone(),
            evidence_sha256: format!("evidence-{:04}", event.ingestion_ordinal),
        })
        .collect();
    ParsedOutput {
        metadata: DemoMetadata {
            map: Some("de_fixture".into()),
            patch_build: Some("fixture-build".into()),
            demo_stamp: None,
            server: None,
            game_directory: None,
            tick_rate: Some("64".into()),
            tick_rate_unavailable_reason: None,
        },
        participants,
        rounds,
        events,
        receipts,
        player_snapshots: snapshots,
        suspicious_empty: false,
        identity: CalculationIdentity {
            source_sha256: "available-demo".into(),
            parser_commit: "commit".into(),
            parser_build: "parser".into(),
            generated_proto_build: "proto".into(),
            requested_schema_hash: "query".into(),
            metric_definition_version: "metrics".into(),
            formula_version: "ofr-1.0.0".into(),
            evidence_semantics_epoch: "epoch".into(),
        },
    }
}

#[test]
fn unavailable_rating_still_commits_a_nonempty_match_and_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let outcome = ImportService::new(&mut storage, FixtureParser(fixture_output()))
        .import(ImportRequest {
            source: &source,
            local_steam_id: 76_561_197_964_020_430,
            worker: "test-worker",
            lease_expires_at_ms: i64::MAX,
        })
        .unwrap();
    assert!(matches!(
        outcome,
        ImportOutcome::RatingUnavailable {
            reason: openfrag_analysis::AnalysisUnavailable::MissingTickRate,
            ..
        }
    ));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM matches", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM match_players", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM analysis_runs WHERE status='succeeded'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM receipts WHERE metric_key='rating_unavailable'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM import_jobs WHERE status='succeeded'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn post_analysis_reconciliation_persists_in_deterministic_order() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("reconcile.dem");
    fs::write(&source, b"PBDEMS2\0reconcile").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let mut persistence = PostAnalysisPersistence {
        database: dir.path().join("store/openfrag.sqlite3"),
        decisions: Vec::new(),
    };
    let outcome = ImportService::new(&mut storage, FixtureParser(available_output()))
        .import_and_reconcile(
            ImportRequest {
                source: &source,
                local_steam_id: 76_561_197_964_020_430,
                worker: "reconcile-worker",
                lease_expires_at_ms: i64::MAX,
            },
            &[auto_candidate("z", Some(1)), auto_candidate("a", Some(1))],
            &mut persistence,
        )
        .unwrap();
    assert!(matches!(outcome.import, ImportOutcome::Ready { .. }));
    assert_eq!(
        outcome
            .reconciliation
            .iter()
            .map(|decision| decision.candidate_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(persistence.decisions, outcome.reconciliation);
    assert!(persistence.decisions.iter().all(|decision| matches!(
        decision.disposition,
        ReconciliationDisposition::RejectOrdinary
    )));
    drop(storage);
    let database = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM analysis_runs WHERE status='succeeded'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn post_analysis_missing_demo_evidence_persists_conservative_decision() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing-evidence.dem");
    fs::write(&source, b"PBDEMS2\0missing-evidence").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let mut persistence = MemoryReconciliation::default();
    let outcome = ImportService::new(&mut storage, FixtureParser(fixture_output()))
        .import_and_reconcile(
            ImportRequest {
                source: &source,
                local_steam_id: 76_561_197_964_020_430,
                worker: "missing-evidence-worker",
                lease_expires_at_ms: i64::MAX,
            },
            &[auto_candidate("unknown", Some(1))],
            &mut persistence,
        )
        .unwrap();
    assert!(matches!(
        outcome.import,
        ImportOutcome::RatingUnavailable { .. }
    ));
    assert_eq!(persistence.0, outcome.reconciliation);
    assert!(matches!(
        persistence.0[0].disposition,
        ReconciliationDisposition::EvidenceUnavailable(
            UnavailableEvidence::RoundMissingOrAmbiguous
        )
    ));
}

#[test]
fn identical_demo_and_full_identity_deduplicate_without_rewriting_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let request = ImportRequest {
        source: &source,
        local_steam_id: 76_561_197_964_020_430,
        worker: "worker",
        lease_expires_at_ms: i64::MAX,
    };
    ImportService::new(&mut storage, FixtureParser(fixture_output()))
        .import(request)
        .unwrap();
    let second = ImportService::new(&mut storage, FixtureParser(fixture_output()))
        .import(request)
        .unwrap();
    assert!(matches!(second, ImportOutcome::Deduplicated { .. }));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM matches", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM analysis_runs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM import_jobs WHERE status='succeeded'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
fn complete_synthetic_evidence_reaches_ready_with_six_component_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("available.dem");
    fs::write(&source, b"PBDEMS2\0available-fixture").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let request = ImportRequest {
        source: &source,
        local_steam_id: 76_561_197_964_020_430,
        worker: "worker",
        lease_expires_at_ms: i64::MAX,
    };
    let first = ImportService::new(&mut storage, FixtureParser(available_output()))
        .import(request)
        .unwrap();
    assert!(matches!(first, ImportOutcome::Ready { .. }));
    let second = ImportService::new(&mut storage, FixtureParser(available_output()))
        .import(request)
        .unwrap();
    assert!(matches!(second, ImportOutcome::Deduplicated { .. }));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM player_rating_vectors", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM player_rating_components", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        6
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM receipts WHERE metric_key LIKE 'component:%'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        6
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM analysis_runs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn invalid_stamp_is_rejected_before_parser_or_storage_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bad.dem");
    fs::write(&source, b"HL2DEMO\0bad").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result =
        ImportService::new(&mut storage, FixtureParser(fixture_output())).import(ImportRequest {
            source: &source,
            local_steam_id: 1,
            worker: "worker",
            lease_expires_at_ms: i64::MAX,
        });
    assert!(matches!(result, Err(PipelineError::UnsupportedDemo)));
}

#[test]
fn suspicious_empty_parser_output_is_quarantined_and_failed() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let mut parsed = fixture_output();
    parsed.participants.clear();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result = ImportService::new(&mut storage, FixtureParser(parsed)).import(ImportRequest {
        source: &source,
        local_steam_id: 1,
        worker: "worker",
        lease_expires_at_ms: i64::MAX,
    });
    assert!(matches!(result, Err(PipelineError::Parser(_))));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM artifacts WHERE availability='missing' AND missing_reason='quarantined:suspicious_empty_roster'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM import_jobs WHERE status='failed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn cancel_retry_and_expired_lease_recovery_follow_durable_state_machine() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let staged = storage.stage_artifact(b"demo").unwrap();
    let hash = storage.commit_artifact(staged, "dem", None).unwrap();
    let job = storage.enqueue_import(&hash).unwrap();
    storage.lease_import(&job, "worker", i64::MAX).unwrap();
    let service = ImportService::new(&mut storage, FixtureParser(fixture_output()));
    service.cancel(&job, "worker").unwrap();
    service.retry(&job).unwrap();
    storage.lease_import(&job, "crashed", 1).unwrap();
    let service = ImportService::new(&mut storage, FixtureParser(fixture_output()));
    assert_eq!(service.recover_expired(2).unwrap(), 1);
    assert_eq!(
        storage.import_job(&job).unwrap().phase,
        openfrag_storage::ImportPhase::Queued
    );
}

#[cfg(unix)]
#[test]
fn symlink_demo_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.dem");
    let link = dir.path().join("link.dem");
    fs::write(&real, b"PBDEMS2\0fixture").unwrap();
    symlink(real, &link).unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result =
        ImportService::new(&mut storage, FixtureParser(fixture_output())).import(ImportRequest {
            source: &link,
            local_steam_id: 1,
            worker: "worker",
            lease_expires_at_ms: i64::MAX,
        });
    assert!(matches!(result, Err(PipelineError::InvalidSource)));
}

#[cfg(feature = "demoparser")]
#[test]
fn pinned_real_fixture_commits_unavailable_rating_without_empty_match() {
    let Ok(path) = std::env::var("OPENFRAG_DEMOPARSER_FIXTURE") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let outcome = ImportService::new(&mut storage, openfrag_pipeline::PinnedParser)
        .import(ImportRequest {
            source: Path::new(&path),
            local_steam_id: 76_561_197_964_020_430,
            worker: "fixture-worker",
            lease_expires_at_ms: i64::MAX,
        })
        .unwrap();
    assert!(matches!(
        outcome,
        ImportOutcome::RatingUnavailable {
            demo_sha256,
            reason: openfrag_analysis::AnalysisUnavailable::MissingTickRate,
            ..
        }
        if demo_sha256 == "84a1a4191302bdd2a3bbb5a727842093744b1fb1a228aeec630369e44b622cb2"
    ));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    for (sql, expected) in [
        ("SELECT count(*) FROM match_players", 10_i64),
        ("SELECT count(*) FROM rounds", 10),
        ("SELECT count(*) FROM unavailable_rating_results", 1),
        (
            "SELECT count(*) FROM import_jobs WHERE status='succeeded' AND progress_bp=10000",
            1,
        ),
    ] {
        assert_eq!(
            db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap(),
            expected
        );
    }
}
