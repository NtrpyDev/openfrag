//! Local, transactional persistence for openfrag artifacts and analysis runs.
#![allow(clippy::missing_errors_doc)]

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");

#[derive(Debug)]
pub enum Error {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Invalid(&'static str),
    NotFound(&'static str),
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash)]
        pub struct $name(String);
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
typed_id!(CaptureSessionId);
typed_id!(SaveAttemptId);
typed_id!(ClipId);
typed_id!(MatchId);
typed_id!(AnalysisRunId);
typed_id!(RoundId);
typed_id!(ReconciliationId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveAttemptStatus {
    Requested,
    Acknowledged,
    Saved,
    Failed,
}
impl SaveAttemptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Acknowledged => "acknowledged",
            Self::Saved => "saved",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationStatus {
    Confirmed,
    Unconfirmed,
    Rejected,
}
impl ReconciliationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Unconfirmed => "unconfirmed",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
    pub database: PathBuf,
    pub artifacts: PathBuf,
    pub staging: PathBuf,
}
impl Layout {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            database: root.join("openfrag.sqlite3"),
            artifacts: root.join("artifacts/sha256"),
            staging: root.join("staging"),
            root,
        }
    }
    pub fn xdg_default() -> Result<Self> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/share")))
            .ok_or(Error::Invalid("XDG_DATA_HOME and HOME are unset"))?;
        Ok(Self::at(base.join("openfrag")))
    }
}

#[derive(Debug)]
pub struct StagedArtifact {
    pub sha256: String,
    pub byte_length: u64,
    path: PathBuf,
}

#[derive(Debug)]
pub struct Storage {
    connection: Connection,
    layout: Layout,
}

impl Storage {
    pub fn open_default() -> Result<Self> {
        Self::open(Layout::xdg_default()?)
    }
    pub fn open(layout: Layout) -> Result<Self> {
        fs::create_dir_all(&layout.artifacts)?;
        fs::create_dir_all(&layout.staging)?;
        set_private_permissions(&layout.root)?;
        set_private_permissions(&layout.staging)?;
        let connection = Connection::open(&layout.database)?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )?;
        let storage = Self { connection, layout };
        storage.migrate()?;
        storage.recover_staging()?;
        Ok(storage)
    }
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
    pub fn migrate(&self) -> Result<()> {
        let checksum = hex_sha256(INITIAL_MIGRATION.as_bytes());
        let migration_table_exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;
        if !migration_table_exists {
            self.connection.execute_batch(INITIAL_MIGRATION)?;
            self.connection.execute(
                "INSERT INTO schema_migrations(version, applied_at_ms, checksum) VALUES(1, ?, ?)",
                params![now_ms(), checksum],
            )?;
            return Ok(());
        }
        let exists: Option<String> = self
            .connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = exists {
            if found != checksum {
                return Err(Error::Invalid("migration checksum mismatch"));
            }
            return Ok(());
        }
        Err(Error::Invalid("migration history is incomplete"))
    }
    pub fn stage_artifact(&self, bytes: &[u8]) -> Result<StagedArtifact> {
        let sha256 = hex_sha256(bytes);
        let path = self.layout.staging.join(format!("{}.part", Uuid::now_v7()));
        let mut file = File::create(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(StagedArtifact {
            sha256,
            byte_length: bytes.len() as u64,
            path,
        })
    }
    pub fn commit_artifact(
        &self,
        staged: StagedArtifact,
        extension: &str,
        media_type: Option<&str>,
    ) -> Result<String> {
        if extension.contains('/') || extension.contains('\\') {
            return Err(Error::Invalid("unsafe extension"));
        }
        let prefix = &staged.sha256[..2];
        let relative = format!(
            "artifacts/sha256/{prefix}/{}.{}",
            staged.sha256,
            extension.trim_start_matches('.')
        );
        let target = self.layout.root.join(&relative);
        let parent = target.parent().ok_or(Error::Invalid("artifact path"))?;
        fs::create_dir_all(parent)?;
        if target.exists() {
            fs::remove_file(&staged.path)?;
        } else {
            fs::rename(&staged.path, &target)?;
        }
        let byte_length =
            i64::try_from(staged.byte_length).map_err(|_| Error::Invalid("artifact too large"))?;
        self.connection.execute("INSERT INTO artifacts(sha256,relative_path,byte_length,media_type,availability,created_at_ms) VALUES(?,?,?,?, 'present', ?) ON CONFLICT(sha256) DO UPDATE SET relative_path=excluded.relative_path, availability='present'", params![staged.sha256, relative, byte_length, media_type, now_ms()])?;
        Ok(staged.sha256)
    }
    pub fn recover_staging(&self) -> Result<()> {
        for entry in fs::read_dir(&self.layout.staging)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    pub fn create_capture_session(&self, local_steam_id: &str) -> Result<CaptureSessionId> {
        let id = CaptureSessionId::new();
        self.connection.execute("INSERT INTO capture_sessions(id,local_steam_id,started_at_ms,status) VALUES(?,?,?,'active')", params![id.as_str(), local_steam_id, now_ms()])?;
        Ok(id)
    }
    pub fn request_save(
        &self,
        session: &CaptureSessionId,
        recorder_request_id: &str,
        monotonic_ns: i64,
    ) -> Result<SaveAttemptId> {
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM recorder_save_attempts WHERE recorder_request_id=?",
                [recorder_request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(SaveAttemptId(id));
        }
        let id = SaveAttemptId::new();
        self.connection.execute("INSERT INTO recorder_save_attempts(id,recorder_request_id,capture_session_id,requested_monotonic_ns,status) VALUES(?,?,?,?,?)", params![id.as_str(), recorder_request_id, session.as_str(), monotonic_ns, SaveAttemptStatus::Requested.as_str()])?;
        Ok(id)
    }
    pub fn acknowledge_save(&self, attempt: &SaveAttemptId) -> Result<()> {
        self.connection.execute("UPDATE recorder_save_attempts SET acknowledgement_count=acknowledgement_count+1, status=CASE WHEN status='requested' THEN 'acknowledged' ELSE status END WHERE id=?", [attempt.as_str()])?;
        Ok(())
    }
    pub fn fail_save(&self, attempt: &SaveAttemptId, error_code: &str) -> Result<()> {
        self.connection.execute("UPDATE recorder_save_attempts SET status='failed', error_code=? WHERE id=? AND status <> 'saved'", params![error_code, attempt.as_str()])?;
        Ok(())
    }
    pub fn complete_save(
        &self,
        attempt: &SaveAttemptId,
        artifact_sha256: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<()> {
        if end_ns < start_ns {
            return Err(Error::Invalid("negative media range"));
        }
        let present: Option<String> = self
            .connection
            .query_row(
                "SELECT sha256 FROM artifacts WHERE sha256=? AND availability='present'",
                [artifact_sha256],
                |r| r.get(0),
            )
            .optional()?;
        if present.is_none() {
            return Err(Error::NotFound("verified artifact"));
        }
        self.connection.execute("UPDATE recorder_save_attempts SET status='saved', verified_artifact_sha256=?, actual_start_monotonic_ns=?, actual_end_monotonic_ns=? WHERE id=? AND status IN ('requested','acknowledged')", params![artifact_sha256, start_ns, end_ns, attempt.as_str()])?;
        Ok(())
    }
    pub fn create_clip(&self, attempt: &SaveAttemptId, provenance: &str) -> Result<ClipId> {
        let row: Option<(String, String)> = self.connection.query_row("SELECT capture_session_id, verified_artifact_sha256 FROM recorder_save_attempts WHERE id=? AND status='saved'", [attempt.as_str()], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let (session, artifact) = row.ok_or(Error::Invalid("save attempt is not verified"))?;
        let id = ClipId::new();
        self.connection.execute("INSERT INTO clips(id,artifact_sha256,capture_session_id,disposition,provenance,created_at_ms) VALUES(?,?,?,'saved',?,?)", params![id.as_str(), artifact, session, provenance, now_ms()])?;
        Ok(id)
    }
    pub fn import_match(
        &self,
        demo_sha256: &str,
        local_steam_id: &str,
        map: Option<&str>,
    ) -> Result<MatchId> {
        let id = MatchId::new();
        self.connection.execute("INSERT INTO matches(id,demo_sha256,local_steam_id,map_name,imported_at_ms,status) VALUES(?,?,?,?,?,'imported')", params![id.as_str(), demo_sha256, local_steam_id, map, now_ms()])?;
        Ok(id)
    }
    pub fn begin_analysis(
        &self,
        match_id: &MatchId,
        identity: &AnalysisIdentity,
    ) -> Result<AnalysisRunId> {
        let id = AnalysisRunId::new();
        self.connection.execute("INSERT INTO analysis_runs(id,match_id,parser_commit,parser_build,generated_proto_build,requested_schema_hash,metric_definition_version,formula_id,evidence_semantics_epoch,status,created_at_ms) VALUES(?,?,?,?,?,?,?,?,?, 'running', ?)", params![id.as_str(), match_id.as_str(), identity.parser_commit, identity.parser_build, identity.generated_proto_build, identity.requested_schema_hash, identity.metric_definition_version, identity.formula_id, identity.evidence_semantics_epoch, now_ms()])?;
        Ok(id)
    }
    pub fn complete_analysis(&mut self, run: &AnalysisRunId) -> Result<()> {
        let tx = self.connection.transaction()?;
        let match_id: String = tx.query_row(
            "SELECT match_id FROM analysis_runs WHERE id=?",
            [run.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE analysis_runs SET status='succeeded' WHERE id=?",
            [run.as_str()],
        )?;
        tx.execute(
            "UPDATE matches SET canonical_run_id=?, status='parsed' WHERE id=?",
            params![run.as_str(), match_id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn add_round(
        &self,
        run: &AnalysisRunId,
        number: i64,
        end_tick: i64,
        winner: Option<&str>,
    ) -> Result<RoundId> {
        let id = RoundId::new();
        self.connection.execute(
            "INSERT INTO rounds(id,analysis_run_id,round_number,end_tick,winner) VALUES(?,?,?,?,?)",
            params![id.as_str(), run.as_str(), number, end_tick, winner],
        )?;
        Ok(id)
    }
    pub fn reconcile(
        &self,
        candidate: &str,
        run: &AnalysisRunId,
        round: Option<&RoundId>,
        status: ReconciliationStatus,
        reason: &str,
    ) -> Result<ReconciliationId> {
        if status == ReconciliationStatus::Confirmed && round.is_none() {
            return Err(Error::Invalid("confirmed reconciliation needs a round"));
        }
        if let Some(round) = round {
            let belongs: Option<String> = self
                .connection
                .query_row(
                    "SELECT id FROM rounds WHERE id=? AND analysis_run_id=?",
                    params![round.as_str(), run.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            if belongs.is_none() {
                return Err(Error::Invalid("round belongs to another run"));
            }
        }
        let id = ReconciliationId::new();
        self.connection.execute("INSERT INTO candidate_reconciliations(id,candidate_id,analysis_run_id,round_id,status,reason_code,decided_at_ms) VALUES(?,?,?,?,?,?,?)", params![id.as_str(),candidate,run.as_str(),round.map(RoundId::as_str),status.as_str(),reason,now_ms()])?;
        Ok(id)
    }
}

#[derive(Debug)]
pub struct AnalysisIdentity<'a> {
    pub parser_commit: &'a str,
    pub parser_build: &'a str,
    pub generated_proto_build: &'a str,
    pub requested_schema_hash: &'a str,
    pub metric_definition_version: &'a str,
    pub formula_id: &'a str,
    pub evidence_semantics_epoch: &'a str,
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}
fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Layout::at(dir.path())).unwrap();
        (dir, storage)
    }
    #[test]
    fn migration_is_idempotent_and_configured() {
        let (_dir, storage) = store();
        storage.migrate().unwrap();
        assert_eq!(
            storage
                .connection
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn failed_save_cannot_create_clip() {
        let (_dir, storage) = store();
        let session = storage.create_capture_session("765").unwrap();
        let attempt = storage.request_save(&session, "request-1", 10).unwrap();
        storage.fail_save(&attempt, "recorder_exit").unwrap();
        assert!(matches!(
            storage.create_clip(&attempt, "raw_auto"),
            Err(Error::Invalid(_))
        ));
    }
    #[test]
    fn shared_artifact_can_back_multiple_clips() {
        let (_dir, storage) = store();
        let hash = storage
            .commit_artifact(
                storage.stage_artifact(b"media").unwrap(),
                "mkv",
                Some("video/x-matroska"),
            )
            .unwrap();
        let session = storage.create_capture_session("765").unwrap();
        for request in ["a", "b"] {
            let attempt = storage.request_save(&session, request, 1).unwrap();
            storage.complete_save(&attempt, &hash, 1, 2).unwrap();
            storage.create_clip(&attempt, "raw_auto").unwrap();
        }
        let count: i64 = storage
            .connection
            .query_row(
                "SELECT count(*) FROM clips WHERE artifact_sha256=?",
                [hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn reconciliation_is_per_run() {
        let (_dir, storage) = store();
        let demo = storage
            .commit_artifact(storage.stage_artifact(b"demo").unwrap(), "dem", None)
            .unwrap();
        let m = storage.import_match(&demo, "765", None).unwrap();
        let i = AnalysisIdentity {
            parser_commit: "a",
            parser_build: "b",
            generated_proto_build: "c",
            requested_schema_hash: "d",
            metric_definition_version: "e",
            formula_id: "f",
            evidence_semantics_epoch: "g",
        };
        let run = storage.begin_analysis(&m, &i).unwrap();
        let round = storage.add_round(&run, 1, 42, Some("T")).unwrap();
        storage
            .reconcile(
                "candidate",
                &run,
                Some(&round),
                ReconciliationStatus::Confirmed,
                "matched",
            )
            .unwrap();
        assert!(
            storage
                .reconcile(
                    "candidate",
                    &run,
                    Some(&round),
                    ReconciliationStatus::Confirmed,
                    "again"
                )
                .is_err()
        );
    }
}
