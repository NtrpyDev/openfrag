use openfrag_import::{
    ParseDiagnostic, ParseDiagnosticCategory, ParseStage, ParsedOutput, ParserError,
    ParserProgress, ParserProgressPhase, Participant,
};
use openfrag_pipeline::{
    ImportOutcome, ImportRequest, ImportService, ParserBackend, PipelineError,
    reconcile::{
        CandidateOrigin, ReconciliationCandidate, ReconciliationDecision,
        ReconciliationDisposition, ReconciliationPersistence, UnavailableEvidence,
    },
};
use openfrag_storage::{Layout, Storage};
use std::collections::BTreeMap;
use std::{fs, path::Path};

#[derive(Clone)]
struct FixtureParser(ParsedOutput);

impl ParserBackend for FixtureParser {
    fn parse(
        &self,
        _: &Path,
        _: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, openfrag_import::ParserError> {
        Ok(self.0.clone())
    }
}

#[derive(Clone)]
struct FailingParser(ParseDiagnostic);

impl ParserBackend for FailingParser {
    fn parse(
        &self,
        _: &Path,
        _: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, ParserError> {
        Err(ParserError {
            diagnostic: Box::new(self.0.clone()),
        })
    }
}

#[derive(Clone)]
struct ProgressFixtureParser(ParsedOutput);

impl ParserBackend for ProgressFixtureParser {
    fn parse(
        &self,
        _: &Path,
        progress: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, ParserError> {
        for sample in [
            ParserProgress {
                phase: ParserProgressPhase::FirstPass,
                bytes_consumed: 50,
                total_bytes: 100,
                frames: 4,
                events_emitted: 0,
            },
            ParserProgress {
                phase: ParserProgressPhase::SecondPass,
                bytes_consumed: 75,
                total_bytes: 100,
                frames: 8,
                events_emitted: 9,
            },
            ParserProgress {
                phase: ParserProgressPhase::Finalize,
                bytes_consumed: 100,
                total_bytes: 100,
                frames: 8,
                events_emitted: 9,
            },
        ] {
            progress(sample);
        }
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
        steam_id: 76_561_197_964_020_430.into(),
        name: Some("local".into()),
        team: Some(2),
    });
    parsed.metadata.map = Some("de_mirage".into());
    parsed
}

fn available_output() -> ParsedOutput {
    openfrag_analysis::fixtures::complete_parsed_output()
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
fn parser_phase_progress_is_persisted_through_terminal_import() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    ImportService::new(&mut storage, ProgressFixtureParser(fixture_output()))
        .import(ImportRequest {
            source: &source,
            local_steam_id: 76_561_197_964_020_430,
            worker: "progress-worker",
            lease_expires_at_ms: i64::MAX,
        })
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    let progress = db
        .query_row(
            "SELECT work_phase,bytes_done,bytes_total,frames_done,events_emitted FROM import_jobs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(progress, ("finalize".into(), 100, 100, 8, 9));
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
fn parser_failure_persists_attempt_diagnostics_without_a_partial_match() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("schema-drift.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let diagnostic = ParseDiagnostic {
        category: ParseDiagnosticCategory::SchemaDrift,
        stage: ParseStage::SecondPass,
        game_build: Some("fixture-build".into()),
        byte_offset: Some(4_096),
        frame_index: Some(17),
        tick: Some(1_024.into()),
        command: Some(7),
        required_item: Some("CCSPlayerPawn.health".into()),
        observed_counts: BTreeMap::from([("events".into(), 12)]),
        upstream_source: "UnknownPropName(health)".into(),
    };
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result =
        ImportService::new(&mut storage, FailingParser(diagnostic)).import(ImportRequest {
            source: &source,
            local_steam_id: 1,
            worker: "diagnostic-worker",
            lease_expires_at_ms: i64::MAX,
        });
    assert!(matches!(
        result,
        Err(PipelineError::Parser(diagnostic))
            if diagnostic.category == ParseDiagnosticCategory::SchemaDrift
    ));
    drop(storage);

    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM matches", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let (job_error, remediation, job_diagnostic): (String, String, String) = db
        .query_row(
            "SELECT error_code,remediation_code,diagnostic_json FROM import_jobs WHERE status='failed'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(job_error, "schema_drift");
    assert_eq!(remediation, "update_parser_or_quarantine");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&job_diagnostic).unwrap()["byte_offset"],
        4_096
    );
    let (attempt_status, attempt_diagnostic): (String, String) = db
        .query_row(
            "SELECT status,diagnostic_json FROM import_attempts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(attempt_status, "failed");
    assert_eq!(attempt_diagnostic, job_diagnostic);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM artifacts WHERE availability='missing' AND missing_reason='quarantined:schema_drift'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
}

#[cfg(feature = "demoparser")]
#[test]
fn pinned_truncation_is_durable_and_never_creates_a_match() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("truncated.dem");
    let mut bytes = [b"PBDEMS2\0".as_slice(), &[0_u8; 8]].concat();
    bytes.extend([1, 5, 10, 0, 0]);
    fs::write(&source, bytes).unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result =
        ImportService::new(&mut storage, openfrag_pipeline::PinnedParser).import(ImportRequest {
            source: &source,
            local_steam_id: 1,
            worker: "truncation-worker",
            lease_expires_at_ms: i64::MAX,
        });
    assert!(matches!(
        result,
        Err(PipelineError::Parser(diagnostic))
            if diagnostic.category == ParseDiagnosticCategory::Truncated
                && diagnostic.stage == ParseStage::FrameScan
                && diagnostic.byte_offset == Some(16)
                && diagnostic.frame_index == Some(0)
    ));
    drop(storage);

    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM matches", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let diagnostic: String = db
        .query_row(
            "SELECT diagnostic_json FROM import_attempts WHERE status='failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let diagnostic: serde_json::Value = serde_json::from_str(&diagnostic).unwrap();
    assert_eq!(diagnostic["category"], "truncated");
    assert_eq!(diagnostic["byte_offset"], 16);
    assert_eq!(diagnostic["frame_index"], 0);
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
    let attempts = storage.import_attempts(&job).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[1].status, "failed");
    assert_eq!(attempts[1].error_code.as_deref(), Some("lease_expired"));
    assert_eq!(
        attempts[1].remediation_code.as_deref(),
        Some("automatic_retry")
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
