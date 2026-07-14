//! Local, transactional persistence for openfrag artifacts and analysis runs.
#![allow(clippy::missing_errors_doc)]

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");
const CONTRACT_MIGRATION: &str = include_str!("../migrations/0002_contract.sql");
const ENFORCEMENT_MIGRATION: &str = include_str!("../migrations/0003_enforcement.sql");
const IMPORT_LIFECYCLE_MIGRATION: &str = include_str!("../migrations/0004_import_lifecycle.sql");

#[derive(Debug)]
pub enum Error {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Invalid(&'static str),
    NotFound(&'static str),
    IllegalTransition(&'static str),
    Conflict(&'static str),
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
typed_id!(LiveCandidateId);
typed_id!(ManualFlagId);
typed_id!(ReceiptId);
typed_id!(ExportId);
typed_id!(ImportJobId);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipDisposition {
    Saved,
    InReview,
    Kept,
    Deleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportPhase {
    Queued,
    Leased,
    Succeeded,
    Failed,
    Cancelled,
}
impl ImportPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "leased" => Ok(Self::Leased),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(Error::Invalid("import phase")),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportJob {
    pub id: ImportJobId,
    pub phase: ImportPhase,
    pub progress_bp: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub error_code: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactAvailability {
    Expected,
    Present,
    Missing,
    Deleted,
}
impl ArtifactAvailability {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "expected" => Ok(Self::Expected),
            "present" => Ok(Self::Present),
            "missing" => Ok(Self::Missing),
            "deleted" => Ok(Self::Deleted),
            _ => Err(Error::Invalid("artifact availability")),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactInfo {
    pub sha256: String,
    pub availability: ArtifactAvailability,
    pub relative_path: Option<String>,
    pub byte_length: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryResult {
    pub removed_staging_files: usize,
    pub marked_missing: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteResult {
    pub clip_deleted: bool,
    pub artifact_deleted: bool,
}
impl ClipDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::InReview => "in_review",
            Self::Kept => "kept",
            Self::Deleted => "deleted",
        }
    }
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
    pub quarantine: PathBuf,
}
impl Layout {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            database: root.join("openfrag.sqlite3"),
            artifacts: root.join("artifacts/sha256"),
            staging: root.join("staging"),
            quarantine: root.join("quarantine"),
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
        fs::create_dir_all(&layout.quarantine)?;
        set_private_permissions(&layout.root)?;
        set_private_permissions(&layout.staging)?;
        set_private_permissions(&layout.quarantine)?;
        let connection = Connection::open(&layout.database)?;
        set_file_private_permissions(&layout.database)?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )?;
        let storage = Self { connection, layout };
        storage.migrate()?;
        storage.recover_staging()?;
        storage.recover_artifacts()?;
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
        } else {
            return Err(Error::Invalid("migration history is incomplete"));
        }
        let checksum = hex_sha256(CONTRACT_MIGRATION.as_bytes());
        let exists: Option<String> = self
            .connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=2",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = exists {
            if found != checksum {
                return Err(Error::Invalid("migration checksum mismatch"));
            }
        } else {
            apply_migration(&self.connection, 2, CONTRACT_MIGRATION, &checksum)?;
        }
        let checksum = hex_sha256(ENFORCEMENT_MIGRATION.as_bytes());
        let exists: Option<String> = self
            .connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=3",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = exists {
            if found != checksum {
                return Err(Error::Invalid("migration checksum mismatch"));
            }
        } else {
            apply_migration(&self.connection, 3, ENFORCEMENT_MIGRATION, &checksum)?;
        }
        let checksum = hex_sha256(IMPORT_LIFECYCLE_MIGRATION.as_bytes());
        let exists: Option<String> = self
            .connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=4",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = exists {
            if found != checksum {
                return Err(Error::Invalid("migration checksum mismatch"));
            }
        } else {
            apply_migration(&self.connection, 4, IMPORT_LIFECYCLE_MIGRATION, &checksum)?;
        }
        Ok(())
    }
    pub fn stage_artifact(&self, bytes: &[u8]) -> Result<StagedArtifact> {
        self.stage_from_reader(&mut std::io::Cursor::new(bytes))
    }
    pub fn stage_from_reader(&self, reader: &mut impl Read) -> Result<StagedArtifact> {
        let path = self.layout.staging.join(format!("{}.part", Uuid::now_v7()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        set_file_private_permissions(&path)?;
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
            length = length
                .checked_add(u64::try_from(count).map_err(|_| Error::Invalid("reader length"))?)
                .ok_or(Error::Invalid("artifact too large"))?;
        }
        file.sync_all()?;
        sync_parent(&path)?;
        Ok(StagedArtifact {
            sha256: format!("{:x}", hasher.finalize()),
            byte_length: length,
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
            if fs::symlink_metadata(&target)?.file_type().is_symlink() {
                return Err(Error::Invalid("artifact target symlink"));
            }
            fs::remove_file(&staged.path)?;
        } else {
            fs::rename(&staged.path, &target)?;
            set_file_private_permissions(&target)?;
            sync_parent(&target)?;
        }
        let byte_length =
            i64::try_from(staged.byte_length).map_err(|_| Error::Invalid("artifact too large"))?;
        self.connection.execute("INSERT INTO artifacts(sha256,relative_path,byte_length,media_type,availability,created_at_ms) VALUES(?,?,?,?, 'present', ?) ON CONFLICT(sha256) DO UPDATE SET relative_path=excluded.relative_path, availability='present'", params![staged.sha256, relative, byte_length, media_type, now_ms()])?;
        Ok(staged.sha256)
    }
    pub fn recover_staging(&self) -> Result<()> {
        for entry in fs::read_dir(&self.layout.staging)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                return Err(Error::Invalid("staging symlink"));
            }
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    #[allow(clippy::type_complexity)]
    pub fn artifact(&self, sha256: &str) -> Result<ArtifactInfo> {
        let row:Option<(String,String,Option<String>,i64)>=self.connection.query_row("SELECT sha256,availability,relative_path,byte_length FROM artifacts WHERE sha256=?",[sha256],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (sha256, availability, relative_path, byte_length) =
            row.ok_or(Error::NotFound("artifact"))?;
        Ok(ArtifactInfo {
            sha256,
            availability: ArtifactAvailability::parse(&availability)?,
            relative_path,
            byte_length,
        })
    }
    pub fn expect_artifact(
        &self,
        sha256: &str,
        byte_length: i64,
        media_type: Option<&str>,
    ) -> Result<()> {
        if sha256.len() != 64 || byte_length < 0 {
            return Err(Error::Invalid("expected artifact"));
        }
        let n=self.connection.execute("INSERT OR IGNORE INTO artifacts(sha256,byte_length,media_type,availability,created_at_ms) VALUES(?,?,?,'expected',?)",params![sha256,byte_length,media_type,now_ms()])?;
        if n == 0 {
            return Err(Error::Conflict("expected artifact"));
        }
        Ok(())
    }
    pub fn recover_artifacts(&self) -> Result<RecoveryResult> {
        let mut statement = self
            .connection
            .prepare("SELECT sha256,relative_path FROM artifacts WHERE availability='present'")?;
        let rows = statement.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        let mut missing = 0;
        for row in rows {
            let (sha, path) = row?;
            let Some(path) = path else {
                continue;
            };
            let target = self.layout.root.join(&path);
            if !target.exists() {
                self.connection.execute("UPDATE artifacts SET availability='missing',missing_reason='file_missing' WHERE sha256=? AND availability='present'",[sha])?;
                missing += 1;
            }
        }
        let mut removed = 0;
        for entry in fs::read_dir(&self.layout.staging)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                continue;
            }
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        for prefix in fs::read_dir(&self.layout.artifacts)? {
            let prefix = prefix?;
            if prefix.file_type()?.is_symlink() {
                return Err(Error::Invalid("artifact tree symlink"));
            }
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(prefix.path())? {
                let entry = entry?;
                if entry.file_type()?.is_symlink() {
                    continue;
                }
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let Some((sha, _extension)) = name.split_once('.') else {
                    self.quarantine_orphan(&entry.path())?;
                    continue;
                };
                if sha.len() != 64 || sha != hex_sha256(&fs::read(entry.path())?) {
                    self.quarantine_orphan(&entry.path())?;
                    continue;
                }
                let exists: Option<String> = self
                    .connection
                    .query_row("SELECT sha256 FROM artifacts WHERE sha256=?", [sha], |r| {
                        r.get(0)
                    })
                    .optional()?;
                let relative = entry
                    .path()
                    .strip_prefix(&self.layout.root)
                    .map_err(|_| Error::Invalid("orphan path"))?
                    .to_string_lossy()
                    .to_string();
                if exists.is_none() {
                    let byte_length = i64::try_from(fs::metadata(entry.path())?.len())
                        .map_err(|_| Error::Invalid("orphan too large"))?;
                    self.connection.execute("INSERT INTO artifacts(sha256,relative_path,byte_length,availability,created_at_ms) VALUES(?,?,?,'present',?)",params![sha,relative,byte_length,now_ms()])?;
                } else {
                    self.connection.execute("UPDATE artifacts SET availability='present',relative_path=?,missing_reason=NULL WHERE sha256=? AND availability IN ('expected','missing')",params![relative,sha])?;
                }
            }
        }
        Ok(RecoveryResult {
            removed_staging_files: removed,
            marked_missing: missing,
        })
    }
    fn quarantine_orphan(&self, path: &Path) -> Result<()> {
        let target = self.layout.quarantine.join(format!(
            "{}-{}",
            Uuid::now_v7(),
            path.file_name()
                .ok_or(Error::Invalid("orphan name"))?
                .to_string_lossy()
        ));
        fs::rename(path, target)?;
        Ok(())
    }
    pub fn integrity_check(&self) -> Result<bool> {
        Ok(self
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?
            == "ok")
    }
    #[cfg(test)]
    pub fn hold_write_lock_for_test(
        &mut self,
        ready: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Result<()> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        ready
            .send(())
            .map_err(|_| Error::Invalid("test lock receiver"))?;
        release
            .recv()
            .map_err(|_| Error::Invalid("test lock sender"))?;
        self.connection.execute_batch("COMMIT")?;
        Ok(())
    }
    pub fn delete_clip(&self, clip: &ClipId) -> Result<DeleteResult> {
        let artifact: Option<String> = self
            .connection
            .query_row(
                "SELECT artifact_sha256 FROM clips WHERE id=? AND deleted_at_ms IS NULL",
                [clip.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let artifact = artifact.ok_or(Error::NotFound("live clip"))?;
        let n=self.connection.execute("UPDATE clips SET disposition='deleted',deleted_at_ms=? WHERE id=? AND deleted_at_ms IS NULL",params![now_ms(),clip.as_str()])?;
        if n == 0 {
            return Err(Error::IllegalTransition("clip delete"));
        }
        let remaining: i64 = self.connection.query_row(
            "SELECT count(*) FROM clips WHERE artifact_sha256=? AND deleted_at_ms IS NULL",
            [&artifact],
            |r| r.get(0),
        )?;
        if remaining > 0 {
            return Ok(DeleteResult {
                clip_deleted: true,
                artifact_deleted: false,
            });
        }
        let info = self.artifact(&artifact)?;
        if let Some(relative) = info.relative_path {
            let path = self.layout.root.join(relative);
            if path.exists() {
                fs::remove_file(&path)?;
                sync_parent(&path)?;
            }
        }
        self.connection.execute(
            "UPDATE artifacts SET availability='deleted',deleted_at_ms=? WHERE sha256=?",
            params![now_ms(), artifact],
        )?;
        Ok(DeleteResult {
            clip_deleted: true,
            artifact_deleted: true,
        })
    }
    pub fn create_capture_session(&self, local_steam_id: &str) -> Result<CaptureSessionId> {
        let id = CaptureSessionId::new();
        self.connection.execute("INSERT INTO capture_sessions(id,local_steam_id,started_at_ms,status) VALUES(?,?,?,'active')", params![id.as_str(), local_steam_id, now_ms()])?;
        Ok(id)
    }
    pub fn record_gsi_snapshot(
        &self,
        session: &CaptureSessionId,
        artifact_sha256: &str,
        ordinal: i64,
        received_at_ms: i64,
        fields_json: &str,
        listener_version: &str,
    ) -> Result<()> {
        let changed = self.connection.execute("INSERT OR IGNORE INTO gsi_snapshots(sha256,capture_session_id,arrival_ordinal,received_at_ms,field_presence_json,byte_length,listener_version,http_status) SELECT ?,?,?,?, ?,byte_length,?,200 FROM artifacts WHERE sha256=? AND availability='present'", params![artifact_sha256,session.as_str(),ordinal,received_at_ms,fields_json,listener_version,artifact_sha256])?;
        if changed == 0 {
            return Err(Error::Conflict("snapshot hash, ordinal, or artifact"));
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn create_candidate(
        &self,
        session: &CaptureSessionId,
        rule_version: &str,
        kind: &str,
        map: Option<&str>,
        round: Option<i64>,
        first_ns: i64,
        last_ns: i64,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<LiveCandidateId> {
        if last_ns < first_ns || end_ns < start_ns {
            return Err(Error::Invalid("candidate range"));
        }
        let id = LiveCandidateId::new();
        let changed=self.connection.execute("INSERT OR IGNORE INTO live_candidates(id,capture_session_id,rule_version,observed_kind,observed_map,observed_round,first_transition_monotonic_ns,last_transition_monotonic_ns,candidate_start_monotonic_ns,candidate_end_monotonic_ns,status) VALUES(?,?,?,?,?,?,?,?,?,?,'provisional')",params![id.as_str(),session.as_str(),rule_version,kind,map,round,first_ns,last_ns,start_ns,end_ns])?;
        if changed == 0 {
            return Err(Error::Conflict("candidate dedupe"));
        }
        Ok(id)
    }
    pub fn add_candidate_trigger(
        &self,
        candidate: &LiveCandidateId,
        snapshot_sha256: &str,
        ordinal: i64,
        kind: &str,
        monotonic_ns: i64,
    ) -> Result<()> {
        let changed=self.connection.execute("INSERT OR IGNORE INTO candidate_trigger_receipts(candidate_id,snapshot_sha256,transition_ordinal,transition_kind,transition_monotonic_ns) VALUES(?,?,?,?,?)",params![candidate.as_str(),snapshot_sha256,ordinal,kind,monotonic_ns])?;
        if changed == 0 {
            return Err(Error::Conflict("candidate trigger"));
        }
        Ok(())
    }
    pub fn join_candidate_save(
        &self,
        candidate: &LiveCandidateId,
        attempt: &SaveAttemptId,
        desired_start_ns: i64,
        desired_end_ns: i64,
    ) -> Result<()> {
        let changed=self.connection.execute("INSERT OR IGNORE INTO candidate_save_attempts(candidate_id,save_attempt_id,desired_start_monotonic_ns,desired_end_monotonic_ns) VALUES(?,?,?,?)",params![candidate.as_str(),attempt.as_str(),desired_start_ns,desired_end_ns])?;
        if changed == 0 {
            return Err(Error::Conflict("candidate save join"));
        }
        Ok(())
    }
    pub fn create_manual_flag(
        &self,
        session: &CaptureSessionId,
        monotonic_ns: i64,
    ) -> Result<ManualFlagId> {
        let id = ManualFlagId::new();
        self.connection.execute("INSERT INTO manual_flags(id,capture_session_id,flagged_monotonic_ns,created_at_ms) VALUES(?,?,?,?)",params![id.as_str(),session.as_str(),monotonic_ns,now_ms()])?;
        Ok(id)
    }
    pub fn set_clip_review(
        &self,
        clip: &ClipId,
        disposition: ClipDisposition,
        title: Option<&str>,
        favorite: bool,
    ) -> Result<()> {
        let changed=self.connection.execute("UPDATE clips SET disposition=?, title=?, favorite=? WHERE id=? AND disposition <> 'deleted'",params![disposition.as_str(),title,i64::from(favorite),clip.as_str()])?;
        if changed == 0 {
            return Err(Error::IllegalTransition("clip missing or deleted"));
        }
        Ok(())
    }
    pub fn derive_clip(
        &self,
        derived: &ClipId,
        source: &ClipId,
        operation: &str,
        trim: Option<(i64, i64)>,
    ) -> Result<()> {
        let (start, end) = trim.map_or((None, None), |(a, b)| (Some(a), Some(b)));
        let changed=self.connection.execute("INSERT OR IGNORE INTO clip_derivations(derived_clip_id,source_clip_id,operation,trim_start_ms,trim_end_ms,created_at_ms) VALUES(?,?,?,?,?,?)",params![derived.as_str(),source.as_str(),operation,start,end,now_ms()])?;
        if changed == 0 {
            return Err(Error::Conflict("clip derivation"));
        }
        Ok(())
    }
    pub fn enqueue_export(&self, clip: &ClipId, target: &str) -> Result<ExportId> {
        let id = ExportId::new();
        self.connection.execute("INSERT INTO clip_exports(id,clip_id,target_path,requested_at_ms,status) VALUES(?,?,?,?, 'queued')",params![id.as_str(),clip.as_str(),target,now_ms()])?;
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
    pub fn upsert_player(&self, steam_id: &str, display_name: Option<&str>) -> Result<()> {
        self.connection.execute("INSERT INTO players(steam_id,display_name,first_seen_at_ms,last_seen_at_ms) VALUES(?,?,?,?) ON CONFLICT(steam_id) DO UPDATE SET display_name=excluded.display_name,last_seen_at_ms=excluded.last_seen_at_ms",params![steam_id,display_name,now_ms(),now_ms()])?;
        Ok(())
    }
    pub fn add_match_participant(
        &self,
        match_id: &MatchId,
        steam_id: &str,
        participation: &str,
    ) -> Result<()> {
        let changed=self.connection.execute("INSERT OR IGNORE INTO match_players(match_id,steam_id,participation_status) VALUES(?,?,?)",params![match_id.as_str(),steam_id,participation])?;
        if changed == 0 {
            return Err(Error::Conflict("match participant"));
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn add_receipt(
        &self,
        run: &AnalysisRunId,
        round: Option<&RoundId>,
        metric_key: &str,
        tick: Option<i64>,
        ordinal: Option<i64>,
        participants_json: &str,
        payload_json: &str,
        snapshots_json: &str,
        parameters_json: &str,
    ) -> Result<ReceiptId> {
        if tick.is_some() != ordinal.is_some() {
            return Err(Error::Invalid("tick and ordinal"));
        }
        let id = ReceiptId::new();
        self.connection.execute("INSERT INTO receipts(id,analysis_run_id,round_id,metric_key,event_tick,ingestion_ordinal,participant_steam_ids_json,raw_payload_json,snapshots_json,parameters_json) VALUES(?,?,?,?,?,?,?,?,?,?)",params![id.as_str(),run.as_str(),round.map(RoundId::as_str),metric_key,tick,ordinal,participants_json,payload_json,snapshots_json,parameters_json])?;
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn write_round_metric(
        &self,
        run: &AnalysisRunId,
        round: &RoundId,
        steam_id: &str,
        key: &str,
        numerator: i64,
        denominator: i64,
        receipt: Option<&ReceiptId>,
    ) -> Result<()> {
        let changed=self.connection.execute("INSERT OR IGNORE INTO round_player_metrics(analysis_run_id,round_id,steam_id,metric_key,numerator,denominator,receipt_id) VALUES(?,?,?,?,?,?,?)",params![run.as_str(),round.as_str(),steam_id,key,numerator,denominator,receipt.map(ReceiptId::as_str)])?;
        if changed == 0 {
            return Err(Error::Conflict("round metric"));
        }
        Ok(())
    }
    pub fn enqueue_import(&self, demo_sha256: &str) -> Result<ImportJobId> {
        let id = ImportJobId::new();
        let changed=self.connection.execute("INSERT OR IGNORE INTO import_jobs(id,demo_sha256,status,created_at_ms,updated_at_ms) VALUES(?,?,'queued',?,?)",params![id.as_str(),demo_sha256,now_ms(),now_ms()])?;
        if changed == 0 {
            return Err(Error::Conflict("import job"));
        }
        Ok(id)
    }
    pub fn lease_import(&self, job: &ImportJobId, owner: &str, expires_at_ms: i64) -> Result<()> {
        let changed=self.connection.execute("UPDATE import_jobs SET status='leased',lease_owner=?,lease_expires_at_ms=?,updated_at_ms=? WHERE id=? AND (status='queued' OR (status='leased' AND lease_expires_at_ms < ?))",params![owner,expires_at_ms,now_ms(),job.as_str(),now_ms()])?;
        if changed == 0 {
            return Err(Error::IllegalTransition("import lease"));
        }
        Ok(())
    }
    #[allow(clippy::type_complexity)]
    pub fn import_job(&self, id: &ImportJobId) -> Result<ImportJob> {
        let row: Option<(String, i64, Option<String>, Option<i64>, Option<String>)> = self.connection.query_row("SELECT status,progress_bp,lease_owner,lease_expires_at_ms,error_code FROM import_jobs WHERE id=?",[id.as_str()],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (phase, progress_bp, lease_owner, lease_expires_at_ms, error_code) =
            row.ok_or(Error::NotFound("import job"))?;
        Ok(ImportJob {
            id: id.clone(),
            phase: ImportPhase::parse(&phase)?,
            progress_bp,
            lease_owner,
            lease_expires_at_ms,
            error_code,
        })
    }
    pub fn update_import_progress(
        &self,
        id: &ImportJobId,
        owner: &str,
        done: Option<i64>,
        total: Option<i64>,
        progress: i64,
        heartbeat: i64,
    ) -> Result<()> {
        if !(0..=10_000).contains(&progress) {
            return Err(Error::Invalid("progress"));
        }
        let n=self.connection.execute("UPDATE import_jobs SET bytes_done=?,bytes_total=?,progress_bp=?,heartbeat_at_ms=?,updated_at_ms=? WHERE id=? AND status='leased' AND lease_owner=?",params![done,total,progress,heartbeat,now_ms(),id.as_str(),owner])?;
        if n == 0 {
            return Err(Error::IllegalTransition("import progress lease"));
        }
        Ok(())
    }
    pub fn finish_import(
        &self,
        id: &ImportJobId,
        owner: &str,
        phase: ImportPhase,
        error: Option<&str>,
        remediation: Option<&str>,
        retry_at: Option<i64>,
    ) -> Result<()> {
        if !matches!(
            phase,
            ImportPhase::Succeeded | ImportPhase::Failed | ImportPhase::Cancelled
        ) {
            return Err(Error::Invalid("terminal phase"));
        }
        let n=self.connection.execute("UPDATE import_jobs SET status=?,error_code=?,remediation_code=?,next_retry_at_ms=?,lease_owner=NULL,lease_expires_at_ms=NULL,updated_at_ms=? WHERE id=? AND status='leased' AND lease_owner=?",params![phase.as_str(),error,remediation,retry_at,now_ms(),id.as_str(),owner])?;
        if n == 0 {
            return Err(Error::IllegalTransition("finish import"));
        }
        Ok(())
    }
    pub fn reset_import_retry(&self, id: &ImportJobId) -> Result<()> {
        let n=self.connection.execute("UPDATE import_jobs SET status='queued',retry_budget=3,error_code=NULL,remediation_code=NULL,next_retry_at_ms=NULL,lease_owner=NULL,lease_expires_at_ms=NULL,updated_at_ms=? WHERE id=? AND status IN ('failed','cancelled')",params![now_ms(),id.as_str()])?;
        if n == 0 {
            return Err(Error::IllegalTransition("manual retry"));
        }
        Ok(())
    }
    pub fn recover_expired_imports(&self, now: i64) -> Result<usize> {
        Ok(self.connection.execute("UPDATE import_jobs SET status='queued',lease_owner=NULL,lease_expires_at_ms=NULL,updated_at_ms=? WHERE status='leased' AND lease_expires_at_ms < ?",params![now,now])?)
    }
    pub fn reconcile(
        &self,
        candidate: &LiveCandidateId,
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
        self.connection.execute("INSERT INTO candidate_reconciliations(id,candidate_id,analysis_run_id,round_id,status,reason_code,decided_at_ms) VALUES(?,?,?,?,?,?,?)", params![id.as_str(),candidate.as_str(),run.as_str(),round.map(RoundId::as_str),status.as_str(),reason,now_ms()])?;
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
fn apply_migration(connection: &Connection, version: i64, sql: &str, checksum: &str) -> Result<()> {
    let tx = connection.unchecked_transaction()?;
    tx.execute_batch(sql)?;
    tx.execute(
        "INSERT INTO schema_migrations(version, applied_at_ms, checksum) VALUES(?, ?, ?)",
        params![version, now_ms(), checksum],
    )?;
    tx.commit()?;
    Ok(())
}
fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn set_file_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
fn sync_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or(Error::Invalid("path parent"))?;
    File::open(parent)?.sync_all()?;
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
        storage.add_round(&run, 1, 42, Some("T")).unwrap();
        let session = storage.create_capture_session("765").unwrap();
        let candidate = storage
            .create_candidate(&session, "rule", "kill", None, Some(1), 1, 2, 1, 2)
            .unwrap();
        storage
            .reconcile(
                &candidate,
                &run,
                None,
                ReconciliationStatus::Unconfirmed,
                "matched",
            )
            .unwrap();
        assert!(
            storage
                .reconcile(
                    &candidate,
                    &run,
                    None,
                    ReconciliationStatus::Unconfirmed,
                    "again"
                )
                .is_err()
        );
    }
    #[test]
    fn import_lifecycle_persists_public_state() {
        let (_dir, storage) = store();
        let demo = storage
            .commit_artifact(storage.stage_artifact(b"import").unwrap(), "dem", None)
            .unwrap();
        let job = storage.enqueue_import(&demo).unwrap();
        assert_eq!(storage.import_job(&job).unwrap().phase, ImportPhase::Queued);
        storage.lease_import(&job, "worker-a", i64::MAX).unwrap();
        storage
            .update_import_progress(&job, "worker-a", Some(5), Some(10), 5000, 77)
            .unwrap();
        storage
            .finish_import(
                &job,
                "worker-a",
                ImportPhase::Failed,
                Some("network"),
                Some("retry"),
                Some(99),
            )
            .unwrap();
        assert_eq!(storage.import_job(&job).unwrap().phase, ImportPhase::Failed);
        storage.reset_import_retry(&job).unwrap();
        assert_eq!(storage.import_job(&job).unwrap().phase, ImportPhase::Queued);
    }
    #[test]
    fn import_lease_has_one_public_owner() {
        let (_dir, storage) = store();
        let demo = storage
            .commit_artifact(storage.stage_artifact(b"lease").unwrap(), "dem", None)
            .unwrap();
        let job = storage.enqueue_import(&demo).unwrap();
        storage.lease_import(&job, "first", i64::MAX).unwrap();
        assert!(matches!(
            storage.lease_import(&job, "second", i64::MAX),
            Err(Error::IllegalTransition(_))
        ));
        assert_eq!(
            storage.import_job(&job).unwrap().lease_owner.as_deref(),
            Some("first")
        );
    }
    #[test]
    fn streaming_artifact_is_private_and_queryable() {
        let (_dir, storage) = store();
        let mut reader = std::io::Cursor::new(vec![7_u8; 130_000]);
        let hash = storage
            .commit_artifact(storage.stage_from_reader(&mut reader).unwrap(), "dem", None)
            .unwrap();
        let artifact = storage.artifact(&hash).unwrap();
        assert_eq!(artifact.availability, ArtifactAvailability::Present);
        assert_eq!(artifact.byte_length, 130_000);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(storage.layout().database.clone())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn unsafe_extension_is_rejected_without_public_artifact() {
        let (_dir, storage) = store();
        let staged = storage.stage_artifact(b"unsafe").unwrap();
        let hash = staged.sha256.clone();
        assert!(matches!(
            storage.commit_artifact(staged, "../dem", None),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(storage.artifact(&hash), Err(Error::NotFound(_))));
    }
    #[test]
    fn shared_delete_retains_then_tombstones_artifact() {
        let (_dir, storage) = store();
        let hash = storage
            .commit_artifact(storage.stage_artifact(b"shared").unwrap(), "mkv", None)
            .unwrap();
        let session = storage.create_capture_session("765").unwrap();
        let a = storage.request_save(&session, "delete-a", 1).unwrap();
        storage.complete_save(&a, &hash, 1, 2).unwrap();
        let first = storage.create_clip(&a, "raw_auto").unwrap();
        let b = storage.request_save(&session, "delete-b", 1).unwrap();
        storage.complete_save(&b, &hash, 1, 2).unwrap();
        let second = storage.create_clip(&b, "raw_auto").unwrap();
        assert_eq!(
            storage.delete_clip(&first).unwrap(),
            DeleteResult {
                clip_deleted: true,
                artifact_deleted: false
            }
        );
        assert_eq!(
            storage.delete_clip(&second).unwrap(),
            DeleteResult {
                clip_deleted: true,
                artifact_deleted: true
            }
        );
        assert_eq!(
            storage.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Deleted
        );
    }
    #[test]
    fn reopen_marks_missing_referenced_artifact() {
        let (dir, storage) = store();
        let hash = storage
            .commit_artifact(storage.stage_artifact(b"missing").unwrap(), "dem", None)
            .unwrap();
        let path = storage
            .layout()
            .root
            .join(storage.artifact(&hash).unwrap().relative_path.unwrap());
        drop(storage);
        fs::remove_file(path).unwrap();
        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert_eq!(
            reopened.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Missing
        );
    }
    #[test]
    fn reopen_repairs_verified_rename_before_catalog_commit() {
        let (dir, storage) = store();
        let staged = storage.stage_artifact(b"crash-window").unwrap();
        let hash = staged.sha256.clone();
        let target = storage
            .layout()
            .artifacts
            .join(&hash[..2])
            .join(format!("{hash}.dem"));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::rename(staged.path, target).unwrap();
        drop(storage);
        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert_eq!(
            reopened.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Present
        );
    }
    #[test]
    fn reopen_quarantines_unrecognized_orphan_without_cataloging() {
        let (dir, storage) = store();
        let path = storage.layout().artifacts.join("aa/orphan.dem");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"untrusted").unwrap();
        drop(storage);
        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert!(!path.exists());
        assert_eq!(
            fs::read_dir(&reopened.layout().quarantine).unwrap().count(),
            1
        );
    }
    #[test]
    fn reopen_repairs_expected_artifact_at_verified_path() {
        let (dir, storage) = store();
        let staged = storage.stage_artifact(b"expected").unwrap();
        let hash = staged.sha256.clone();
        storage
            .expect_artifact(&hash, i64::try_from(staged.byte_length).unwrap(), None)
            .unwrap();
        let path = storage
            .layout()
            .artifacts
            .join(&hash[..2])
            .join(format!("{hash}.dem"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::rename(staged.path, path).unwrap();
        drop(storage);
        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert_eq!(
            reopened.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Present
        );
    }
    #[test]
    #[cfg(unix)]
    fn staging_symlink_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path());
        fs::create_dir_all(&layout.staging).unwrap();
        symlink("/etc/passwd", layout.staging.join("bad.part")).unwrap();
        assert!(matches!(
            Storage::open(layout),
            Err(Error::Invalid("staging symlink"))
        ));
    }
    #[test]
    fn two_open_handles_preserve_independent_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let first = Storage::open(Layout::at(dir.path())).unwrap();
        let second = Storage::open(Layout::at(dir.path())).unwrap();
        let a = first.create_capture_session("one").unwrap();
        let b = second.create_capture_session("two").unwrap();
        assert_ne!(a.as_str(), b.as_str());
        drop(first);
        drop(second);
        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert!(reopened.create_capture_session("three").is_ok());
    }
    #[test]
    #[cfg(unix)]
    fn artifact_tree_symlink_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path());
        fs::create_dir_all(&layout.artifacts).unwrap();
        symlink("/etc", layout.artifacts.join("aa")).unwrap();
        assert!(matches!(
            Storage::open(layout),
            Err(Error::Invalid("artifact tree symlink"))
        ));
    }
    #[test]
    fn independent_connections_wait_for_real_write_lock_and_reopen_cleanly() {
        use std::sync::mpsc;
        use std::thread;
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path());
        let mut locked = Storage::open(layout.clone()).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || locked.hold_write_lock_for_test(ready_tx, release_rx));
        ready_rx.recv().unwrap();
        let writer_layout = layout.clone();
        let writer = thread::spawn(move || {
            let storage = Storage::open(writer_layout).unwrap();
            storage.create_capture_session("contender")
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        release_tx.send(()).unwrap();
        holder.join().unwrap().unwrap();
        let id = writer.join().unwrap().unwrap();
        assert!(!id.as_str().is_empty());
        let reopened = Storage::open(layout).unwrap();
        assert!(reopened.integrity_check().unwrap());
        assert!(reopened.create_capture_session("after").is_ok());
    }
}
