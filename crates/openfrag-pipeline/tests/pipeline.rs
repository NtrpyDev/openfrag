use openfrag_import::{ParsedOutput, Participant};
use openfrag_pipeline::{ImportOutcome, ImportRequest, ImportService, ParserBackend, PipelineError};
use openfrag_storage::{Layout, Storage};
use std::{fs, path::Path};

#[derive(Clone)]
struct FixtureParser(ParsedOutput);

impl ParserBackend for FixtureParser {
    fn parse(&self, _: &Path) -> Result<ParsedOutput, String> { Ok(self.0.clone()) }
}

fn fixture_output() -> ParsedOutput {
    let mut parsed = openfrag_analysis::fixtures::minimal_parsed_output_without_tick_rate();
    parsed.participants.push(Participant { steam_id: 76_561_197_964_020_430, name: Some("local".into()), team: Some(2) });
    parsed.metadata.map = Some("de_mirage".into());
    parsed
}

#[test]
fn unavailable_rating_still_commits_a_nonempty_match_and_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.dem");
    fs::write(&source, b"PBDEMS2\0fixture").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let outcome = ImportService::new(&mut storage, FixtureParser(fixture_output())).import(ImportRequest {
        source: &source, local_steam_id: 76_561_197_964_020_430, worker: "test-worker", lease_expires_at_ms: i64::MAX,
    }).unwrap();
    assert!(matches!(outcome, ImportOutcome::RatingUnavailable { reason: openfrag_analysis::AnalysisUnavailable::MissingTickRate, .. }));
    drop(storage);
    let db = rusqlite::Connection::open(dir.path().join("store/openfrag.sqlite3")).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM matches", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(db.query_row("SELECT count(*) FROM match_players", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(db.query_row("SELECT count(*) FROM analysis_runs WHERE status='succeeded'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(db.query_row("SELECT count(*) FROM receipts WHERE metric_key='rating_unavailable'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(db.query_row("SELECT count(*) FROM import_jobs WHERE status='succeeded'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn invalid_stamp_is_rejected_before_parser_or_storage_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bad.dem");
    fs::write(&source, b"HL2DEMO\0bad").unwrap();
    let mut storage = Storage::open(Layout::at(dir.path().join("store"))).unwrap();
    let result = ImportService::new(&mut storage, FixtureParser(fixture_output())).import(ImportRequest { source: &source, local_steam_id: 1, worker: "worker", lease_expires_at_ms: i64::MAX });
    assert!(matches!(result, Err(PipelineError::UnsupportedDemo)));
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
    let result = ImportService::new(&mut storage, FixtureParser(fixture_output())).import(ImportRequest { source: &link, local_steam_id: 1, worker: "worker", lease_expires_at_ms: i64::MAX });
    assert!(matches!(result, Err(PipelineError::InvalidSource)));
}
