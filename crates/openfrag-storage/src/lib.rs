//! Local, transactional persistence for openfrag artifacts and analysis runs.
#![allow(clippy::missing_errors_doc)]

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");
const CONTRACT_MIGRATION: &str = include_str!("../migrations/0002_contract.sql");
const ENFORCEMENT_MIGRATION: &str = include_str!("../migrations/0003_enforcement.sql");
const IMPORT_LIFECYCLE_MIGRATION: &str = include_str!("../migrations/0004_import_lifecycle.sql");
const RATING_AVAILABILITY_MIGRATION: &str =
    include_str!("../migrations/0005_rating_availability.sql");
const CLIP_REVIEW_MIGRATION: &str = include_str!("../migrations/0006_clip_review.sql");
const CLIP_MODEL_MIGRATION: &str = include_str!("../migrations/0007_clip_model.sql");
const CLIP_REVIEW_TIME_MIGRATION: &str = include_str!("../migrations/0008_clip_review_time.sql");

#[derive(Debug)]
pub enum Error {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Invalid(&'static str),
    NotFound(&'static str),
    IllegalTransition(&'static str),
    Conflict(&'static str),
    Unavailable(&'static str),
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
typed_id!(RatingVectorId);

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
pub enum ClipReviewDecision {
    Pending,
    Keep,
    Reject,
}
impl ClipReviewDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Keep => "keep",
            Self::Reject => "reject",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "keep" => Ok(Self::Keep),
            "reject" => Ok(Self::Reject),
            _ => Err(Error::Invalid("clip review decision")),
        }
    }
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
    Quarantined,
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
pub enum StoredRatingAvailability {
    Pending,
    Available {
        formula_id: String,
        rating_bp: i64,
        receipt_id: String,
    },
    Unavailable {
        formula_id: String,
        reason_code: String,
        receipt_id: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchSummaryRecord {
    pub id: String,
    pub map_name: Option<String>,
    pub imported_at_ms: i64,
    pub status: String,
    pub rating: StoredRatingAvailability,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RatingComponentRecord {
    pub component_key: String,
    pub numerator: i64,
    pub denominator: i64,
    pub value_bp: i64,
    pub weight_bp: i64,
    pub receipt_id: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchDetailRecord {
    pub summary: MatchSummaryRecord,
    pub demo_sha256: String,
    pub local_steam_id: String,
    pub game_build: Option<String>,
    pub canonical_run_id: Option<String>,
    pub components: Vec<RatingComponentRecord>,
    pub receipt_ids: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptRecord {
    pub id: String,
    pub analysis_run_id: String,
    pub round_id: Option<String>,
    pub metric_key: String,
    pub event_tick: Option<i64>,
    pub ingestion_ordinal: Option<i64>,
    pub participant_steam_ids_json: String,
    pub raw_payload_json: String,
    pub snapshots_json: String,
    pub parameters_json: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipSummaryRecord {
    pub id: String,
    pub title: Option<String>,
    pub disposition: String,
    pub recorded_at_ms: i64,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipDetailRecord {
    pub summary: ClipSummaryRecord,
    pub artifact_sha256: String,
    pub artifact_availability: ArtifactAvailability,
    pub provenance: String,
    pub favorite: bool,
    pub review_decision: ClipReviewDecision,
    pub pre_roll_truncated: bool,
    pub retention_class: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipArtifactRecord {
    pub clip_id: String,
    pub sha256: String,
    pub path: PathBuf,
    pub byte_length: i64,
    pub media_type: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DurableClipOriginRecord {
    Auto {
        trigger_receipts: Vec<String>,
        evidence_receipts: Vec<String>,
    },
    Manual {
        flag_receipt_id: String,
        flag_time_ms: u64,
        overlapping_auto_receipts: Vec<String>,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum DurableClipOriginInput<'a> {
    Auto {
        trigger_receipts: &'a [String],
        evidence_receipts: &'a [String],
    },
    Manual {
        flag_receipt_id: &'a str,
        flag_time_ms: u64,
        overlapping_auto_receipts: &'a [String],
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableClipModelRecord {
    pub detail: ClipDetailRecord,
    pub artifact: ClipArtifactRecord,
    pub capture_session_id: String,
    pub duration_ms: u64,
    pub revision: u64,
    pub reviewed_at_ms: Option<u64>,
    pub origin: DurableClipOriginRecord,
}
#[derive(Debug)]
pub struct DerivativeClipCommit<'a> {
    pub source_clip_id: &'a str,
    pub staged: StagedArtifact,
    pub extension: &'a str,
    pub media_type: Option<&'a str>,
    pub duration_ms: u64,
    pub trim_start_ms: u64,
    pub trim_end_ms: u64,
    pub title: &'a str,
    pub note: &'a str,
    pub tags: &'a [String],
    pub favorite: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipExportStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipExportRecord {
    pub id: String,
    pub clip_id: String,
    pub target_path: String,
    pub status: ClipExportStatus,
    pub error_code: Option<String>,
    pub output_sha256: Option<String>,
    pub output_byte_length: Option<u64>,
    pub output_media_profile: Option<String>,
}
impl ClipExportStatus {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(Error::Invalid("clip export status")),
        }
    }
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualFlagSaveLink {
    pub manual_flag_id: ManualFlagId,
    pub save_attempt_id: SaveAttemptId,
    pub desired_start_monotonic_ns: i64,
    pub desired_end_monotonic_ns: i64,
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
    #[allow(clippy::too_many_lines)]
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
        let checksum = hex_sha256(RATING_AVAILABILITY_MIGRATION.as_bytes());
        let exists: Option<String> = self
            .connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=5",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(found) = exists {
            if found != checksum {
                return Err(Error::Invalid("migration checksum mismatch"));
            }
        } else {
            apply_migration(
                &self.connection,
                5,
                RATING_AVAILABILITY_MIGRATION,
                &checksum,
            )?;
        }
        ensure_migration(&self.connection, 6, CLIP_REVIEW_MIGRATION)?;
        ensure_migration(&self.connection, 7, CLIP_MODEL_MIGRATION)?;
        ensure_migration(&self.connection, 8, CLIP_REVIEW_TIME_MIGRATION)?;
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
        let row:Option<(String,String,Option<String>,i64,Option<String>)>=self.connection.query_row("SELECT sha256,availability,relative_path,byte_length,missing_reason FROM artifacts WHERE sha256=?",[sha256],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (sha256, availability, relative_path, byte_length, reason) =
            row.ok_or(Error::NotFound("artifact"))?;
        Ok(ArtifactInfo {
            sha256,
            availability: if availability == "missing"
                && reason
                    .as_deref()
                    .is_some_and(|value| value.starts_with("quarantined:"))
            {
                ArtifactAvailability::Quarantined
            } else {
                ArtifactAvailability::parse(&availability)?
            },
            relative_path,
            byte_length,
        })
    }
    pub fn quarantine_artifact(&mut self, sha256: &str, reason: &str) -> Result<PathBuf> {
        if reason.is_empty() {
            return Err(Error::Invalid("quarantine reason"));
        }
        let artifact = self.artifact(sha256)?;
        if artifact.availability != ArtifactAvailability::Present {
            return Err(Error::Invalid("artifact is not present"));
        }
        let referenced: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM clips WHERE artifact_sha256=? UNION ALL SELECT 1 FROM matches WHERE demo_sha256=? UNION ALL SELECT 1 FROM recorder_save_attempts WHERE verified_artifact_sha256=? UNION ALL SELECT 1 FROM gsi_snapshots WHERE sha256=?)", params![sha256, sha256, sha256, sha256], |row| row.get(0))?;
        if referenced {
            return Err(Error::Conflict("referenced artifact quarantine"));
        }
        let relative = artifact
            .relative_path
            .ok_or(Error::Invalid("artifact path"))?;
        let source = self.layout.root.join(&relative);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::Invalid("artifact symlink"));
        }
        let destination = self
            .layout
            .quarantine
            .join(format!("{sha256}-{}.dem", Uuid::now_v7()));
        fs::rename(&source, &destination)?;
        let relative_destination = destination
            .strip_prefix(&self.layout.root)
            .map_err(|_| Error::Invalid("quarantine path"))?
            .to_string_lossy()
            .into_owned();
        let result = self.connection.execute("UPDATE artifacts SET availability='missing',relative_path=?,missing_reason=? WHERE sha256=? AND availability='present'", params![relative_destination, format!("quarantined:{reason}"), sha256]);
        match result {
            Ok(1) => {
                sync_parent(&destination)?;
                Ok(destination)
            }
            Ok(_) => {
                let _ = fs::rename(&destination, &source);
                Err(Error::Conflict("artifact quarantine"))
            }
            Err(error) => {
                let _ = fs::rename(&destination, &source);
                Err(Error::Database(error))
            }
        }
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
    #[allow(clippy::needless_pass_by_value)]
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
        let other_live_clips: i64 = self.connection.query_row(
            "SELECT count(*) FROM clips WHERE artifact_sha256=? AND id<>? AND deleted_at_ms IS NULL",
            params![artifact, clip.as_str()],
            |row| row.get(0),
        )?;
        let artifact_path = if other_live_clips == 0 {
            let info = self.artifact(&artifact)?;
            if info.availability == ArtifactAvailability::Present {
                let relative = info
                    .relative_path
                    .as_deref()
                    .ok_or(Error::Invalid("artifact path"))?;
                Some(self.safe_artifact_path(&artifact, relative, info.byte_length)?)
            } else {
                None
            }
        } else {
            None
        };
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
        if let Some(path) = artifact_path
            && path.exists()
        {
            fs::remove_file(&path)?;
            sync_parent(&path)?;
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
    pub fn join_manual_flag_save(
        &self,
        manual_flag: &ManualFlagId,
        save_attempt: &SaveAttemptId,
        desired_start_ns: i64,
        desired_end_ns: i64,
    ) -> Result<()> {
        if desired_start_ns < 0 || desired_end_ns < desired_start_ns {
            return Err(Error::Invalid("manual flag save range"));
        }
        let flag_session: Option<String> = self
            .connection
            .query_row(
                "SELECT capture_session_id FROM manual_flags WHERE id=?",
                [manual_flag.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let flag_session = flag_session.ok_or(Error::NotFound("manual flag"))?;
        let attempt_session: Option<String> = self
            .connection
            .query_row(
                "SELECT capture_session_id FROM recorder_save_attempts WHERE id=?",
                [save_attempt.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let attempt_session = attempt_session.ok_or(Error::NotFound("save attempt"))?;
        if flag_session != attempt_session {
            return Err(Error::Invalid("manual flag save session mismatch"));
        }
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO manual_flag_save_attempts(manual_flag_id,save_attempt_id,desired_start_monotonic_ns,desired_end_monotonic_ns) VALUES(?,?,?,?)",
            params![manual_flag.as_str(), save_attempt.as_str(), desired_start_ns, desired_end_ns],
        )?;
        if changed == 1 {
            return Ok(());
        }
        let existing = self.manual_flag_save_link(manual_flag, save_attempt)?;
        if existing.desired_start_monotonic_ns == desired_start_ns
            && existing.desired_end_monotonic_ns == desired_end_ns
        {
            Ok(())
        } else {
            Err(Error::Conflict("manual flag save join"))
        }
    }
    pub fn manual_flag_save_link(
        &self,
        manual_flag: &ManualFlagId,
        save_attempt: &SaveAttemptId,
    ) -> Result<ManualFlagSaveLink> {
        self.connection
            .query_row(
                "SELECT desired_start_monotonic_ns,desired_end_monotonic_ns FROM manual_flag_save_attempts WHERE manual_flag_id=? AND save_attempt_id=?",
                params![manual_flag.as_str(), save_attempt.as_str()],
                |row| {
                    Ok(ManualFlagSaveLink {
                        manual_flag_id: manual_flag.clone(),
                        save_attempt_id: save_attempt.clone(),
                        desired_start_monotonic_ns: row.get(0)?,
                        desired_end_monotonic_ns: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(Error::NotFound("manual flag save join"))
    }
    pub fn complete_clip_model_metadata(
        &self,
        clip_id: &str,
        duration_ms: u64,
        origin: DurableClipOriginInput<'_>,
    ) -> Result<()> {
        let duration_ms = positive_i64(duration_ms, "clip duration")?;
        validate_origin(origin)?;
        let transaction = self.connection.unchecked_transaction()?;
        let artifact: Option<String> = transaction
            .query_row(
                "SELECT artifact_sha256 FROM clips WHERE id=? AND disposition<>'deleted'",
                [clip_id],
                |row| row.get(0),
            )
            .optional()?;
        let artifact = artifact.ok_or(Error::NotFound("live clip"))?;
        let changed = transaction.execute(
            "UPDATE artifacts SET media_duration_ms=? WHERE sha256=? AND (media_duration_ms IS NULL OR media_duration_ms=?)",
            params![duration_ms, artifact, duration_ms],
        )?;
        if changed == 0 {
            return Err(Error::Conflict("verified media duration"));
        }
        let (kind, flag_time) = match origin {
            DurableClipOriginInput::Auto { .. } => ("auto", None),
            DurableClipOriginInput::Manual { flag_time_ms, .. } => (
                "manual",
                Some(i64::try_from(flag_time_ms).map_err(|_| Error::Invalid("flag time"))?),
            ),
        };
        transaction.execute(
            "UPDATE clips SET origin_kind=?,manual_flag_time_ms=?,review_revision=COALESCE(review_revision,0) WHERE id=? AND disposition<>'deleted'",
            params![kind, flag_time, clip_id],
        )?;
        transaction.execute(
            "DELETE FROM clip_origin_receipts WHERE clip_id=?",
            [clip_id],
        )?;
        insert_origin_receipts(&transaction, clip_id, origin)?;
        transaction.commit()?;
        Ok(())
    }
    pub fn set_clip_review(
        &self,
        clip: &ClipId,
        disposition: ClipDisposition,
        title: Option<&str>,
        favorite: bool,
    ) -> Result<()> {
        let review_decision = if disposition == ClipDisposition::Kept {
            ClipReviewDecision::Keep
        } else {
            ClipReviewDecision::Pending
        };
        let changed=self.connection.execute("UPDATE clips SET disposition=?, title=?, favorite=?, review_decision=?, review_revision=CASE WHEN review_revision IS NULL THEN NULL ELSE review_revision+1 END, reviewed_at_ms=? WHERE id=? AND disposition <> 'deleted'",params![disposition.as_str(),title,i64::from(favorite),review_decision.as_str(),now_ms(),clip.as_str()])?;
        if changed == 0 {
            return Err(Error::IllegalTransition("clip missing or deleted"));
        }
        Ok(())
    }
    pub fn update_clip_review(
        &self,
        clip_id: &str,
        title: Option<&str>,
        note: Option<&str>,
        tags: &[String],
        decision: ClipReviewDecision,
        favorite: bool,
    ) -> Result<()> {
        if title.is_some_and(|value| value.len() > 200)
            || note.is_some_and(|value| value.len() > 4_000)
            || tags.len() > 32
        {
            return Err(Error::Invalid("clip review metadata limits"));
        }
        let mut unique = HashSet::with_capacity(tags.len());
        if tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64 || !unique.insert(tag.as_str()))
        {
            return Err(Error::Invalid("clip tags"));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let disposition = match decision {
            ClipReviewDecision::Keep => ClipDisposition::Kept,
            ClipReviewDecision::Pending | ClipReviewDecision::Reject => ClipDisposition::InReview,
        };
        let changed = transaction.execute(
            "UPDATE clips SET disposition=?,title=?,note=?,favorite=?,review_decision=?,review_revision=CASE WHEN review_revision IS NULL THEN NULL ELSE review_revision+1 END,reviewed_at_ms=? WHERE id=? AND disposition<>'deleted'",
            params![
                disposition.as_str(),
                title,
                note,
                i64::from(favorite),
                decision.as_str(), now_ms(),
                clip_id
            ],
        )?;
        if changed == 0 {
            return Err(Error::NotFound("live clip"));
        }
        transaction.execute("DELETE FROM clip_tags WHERE clip_id=?", [clip_id])?;
        for (position, tag) in tags.iter().enumerate() {
            let position =
                i64::try_from(position).map_err(|_| Error::Invalid("clip tag position"))?;
            transaction.execute(
                "INSERT INTO clip_tags(clip_id,position,tag) VALUES(?,?,?)",
                params![clip_id, position, tag],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn compare_and_swap_clip_review(
        &self,
        clip_id: &str,
        expected_revision: u64,
        title: Option<&str>,
        note: Option<&str>,
        tags: &[String],
        decision: ClipReviewDecision,
        favorite: bool,
    ) -> Result<u64> {
        validate_review_metadata(title, note, tags)?;
        let expected =
            i64::try_from(expected_revision).map_err(|_| Error::Invalid("clip review revision"))?;
        let next = expected
            .checked_add(1)
            .ok_or(Error::Invalid("clip review revision"))?;
        let transaction = self.connection.unchecked_transaction()?;
        let disposition = match decision {
            ClipReviewDecision::Keep => ClipDisposition::Kept,
            ClipReviewDecision::Pending | ClipReviewDecision::Reject => ClipDisposition::InReview,
        };
        let changed = transaction.execute(
            "UPDATE clips SET disposition=?,title=?,note=?,favorite=?,review_decision=?,review_revision=?,reviewed_at_ms=? WHERE id=? AND disposition<>'deleted' AND review_revision=?",
            params![
                disposition.as_str(), title, note, i64::from(favorite), decision.as_str(), next, now_ms(),
                clip_id, expected
            ],
        )?;
        if changed == 0 {
            let revision: Option<Option<i64>> = transaction
                .query_row(
                    "SELECT review_revision FROM clips WHERE id=? AND disposition<>'deleted'",
                    [clip_id],
                    |row| row.get(0),
                )
                .optional()?;
            return match revision {
                None => Err(Error::NotFound("live clip")),
                Some(None) => Err(Error::Unavailable("clip review revision")),
                Some(Some(_)) => Err(Error::Conflict("clip review revision")),
            };
        }
        replace_clip_tags(&transaction, clip_id, tags)?;
        transaction.commit()?;
        u64::try_from(next).map_err(|_| Error::Invalid("clip review revision"))
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
    #[allow(clippy::needless_pass_by_value)]
    pub fn commit_clip_derivative(&self, request: DerivativeClipCommit<'_>) -> Result<ClipId> {
        validate_review_metadata(Some(request.title), Some(request.note), request.tags)?;
        if request.trim_start_ms >= request.trim_end_ms {
            return Err(Error::Invalid("clip derivative trim"));
        }
        let source = self.durable_clip_model(request.source_clip_id)?;
        if request.trim_end_ms > source.duration_ms {
            return Err(Error::Invalid("clip derivative trim"));
        }
        if self.artifact(&request.staged.sha256).is_ok() {
            return Err(Error::Conflict("derivative artifact already exists"));
        }
        if request.extension.contains('/') || request.extension.contains('\\') {
            return Err(Error::Invalid("unsafe extension"));
        }
        let duration = positive_i64(request.duration_ms, "clip duration")?;
        let start = i64::try_from(request.trim_start_ms)
            .map_err(|_| Error::Invalid("clip derivative trim"))?;
        let end = i64::try_from(request.trim_end_ms)
            .map_err(|_| Error::Invalid("clip derivative trim"))?;
        let bytes = i64::try_from(request.staged.byte_length)
            .map_err(|_| Error::Invalid("artifact too large"))?;
        let prefix = &request.staged.sha256[..2];
        let relative = format!(
            "artifacts/sha256/{prefix}/{}.{}",
            request.staged.sha256,
            request.extension.trim_start_matches('.')
        );
        let target = self.layout.root.join(&relative);
        let parent = target.parent().ok_or(Error::Invalid("artifact path"))?;
        fs::create_dir_all(parent)?;
        if target.exists() {
            return Err(Error::Conflict("derivative artifact target"));
        }
        fs::rename(&request.staged.path, &target)?;
        set_file_private_permissions(&target)?;
        sync_parent(&target)?;
        let result = (|| -> Result<ClipId> {
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute(
                "INSERT INTO artifacts(sha256,relative_path,byte_length,media_type,availability,created_at_ms,media_duration_ms) VALUES(?,?,?,?, 'present', ?, ?)",
                params![request.staged.sha256, relative, bytes, request.media_type, now_ms(), duration],
            )?;
            let derived = ClipId::new();
            let (origin_kind, flag_time) = match &source.origin {
                DurableClipOriginRecord::Auto { .. } => ("auto", None),
                DurableClipOriginRecord::Manual { flag_time_ms, .. } => (
                    "manual",
                    Some(i64::try_from(*flag_time_ms).map_err(|_| Error::Invalid("flag time"))?),
                ),
            };
            transaction.execute(
                "INSERT INTO clips(id,artifact_sha256,capture_session_id,disposition,title,favorite,provenance,created_at_ms,recorded_at_ms,pre_roll_truncated,retention_class,note,review_decision,review_revision,origin_kind,manual_flag_time_ms,reviewed_at_ms) VALUES(?,?,?,'kept',?,?,'trim_derivative',?,?,?,?,?,'keep',0,?,?,?)",
                params![
                    derived.as_str(), request.staged.sha256, source.capture_session_id,
                    request.title, i64::from(request.favorite), now_ms(),
                    source.detail.summary.recorded_at_ms, i64::from(source.detail.pre_roll_truncated),
                    source.detail.retention_class, request.note, origin_kind, flag_time, now_ms()
                ],
            )?;
            replace_clip_tags(&transaction, derived.as_str(), request.tags)?;
            transaction.execute(
                "INSERT INTO clip_origin_receipts(clip_id,kind,position,receipt_id) SELECT ?,kind,position,receipt_id FROM clip_origin_receipts WHERE clip_id=?",
                params![derived.as_str(), request.source_clip_id],
            )?;
            transaction.execute(
                "INSERT INTO clip_derivations(derived_clip_id,source_clip_id,operation,trim_start_ms,trim_end_ms,created_at_ms) VALUES(?,?,'trim',?,?,?)",
                params![derived.as_str(), request.source_clip_id, start, end, now_ms()],
            )?;
            transaction.commit()?;
            Ok(derived)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&target);
            let _ = sync_parent(&target);
        }
        result
    }
    pub fn enqueue_export(&self, clip: &ClipId, target: &str) -> Result<ExportId> {
        if target.is_empty() {
            return Err(Error::Invalid("clip export target"));
        }
        let id = ExportId::new();
        self.connection.execute("INSERT INTO clip_exports(id,clip_id,target_path,requested_at_ms,status) VALUES(?,?,?,?, 'queued')",params![id.as_str(),clip.as_str(),target,now_ms()])?;
        Ok(id)
    }
    pub fn begin_clip_export(&self, export: &ExportId) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE clip_exports SET status='running' WHERE id=? AND status='queued'",
            [export.as_str()],
        )?;
        if changed == 0 {
            return Err(Error::IllegalTransition("clip export begin"));
        }
        Ok(())
    }
    pub fn succeed_clip_export(
        &self,
        export: &ExportId,
        output_sha256: &str,
        output_byte_length: u64,
        output_media_profile: &str,
    ) -> Result<()> {
        if output_sha256.len() != 64 || output_media_profile.is_empty() {
            return Err(Error::Invalid("clip export output"));
        }
        let output_byte_length =
            i64::try_from(output_byte_length).map_err(|_| Error::Invalid("clip export output"))?;
        let changed = self.connection.execute(
            "UPDATE clip_exports SET status='succeeded',completed_at_ms=?,error_code=NULL,output_sha256=?,output_byte_length=?,output_media_profile=? WHERE id=? AND status='running'",
            params![now_ms(), output_sha256, output_byte_length, output_media_profile, export.as_str()],
        )?;
        if changed == 0 {
            return Err(Error::IllegalTransition("clip export succeed"));
        }
        Ok(())
    }
    pub fn fail_clip_export(&self, export: &ExportId, error_code: &str) -> Result<()> {
        if error_code.is_empty() {
            return Err(Error::Invalid("clip export error"));
        }
        let changed = self.connection.execute(
            "UPDATE clip_exports SET status='failed',completed_at_ms=?,error_code=? WHERE id=? AND status IN ('queued','running')",
            params![now_ms(), error_code, export.as_str()],
        )?;
        if changed == 0 {
            return Err(Error::IllegalTransition("clip export fail"));
        }
        Ok(())
    }
    #[allow(clippy::type_complexity)]
    pub fn clip_export(&self, export: &ExportId) -> Result<ClipExportRecord> {
        let row: Option<(String,String,String,String,Option<String>,Option<String>,Option<i64>,Option<String>)> = self.connection.query_row(
            "SELECT id,clip_id,target_path,status,error_code,output_sha256,output_byte_length,output_media_profile FROM clip_exports WHERE id=?",
            [export.as_str()],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
        ).optional()?;
        let (id, clip_id, target_path, status, error_code, output_sha256, bytes, profile) =
            row.ok_or(Error::NotFound("clip export"))?;
        Ok(ClipExportRecord {
            id,
            clip_id,
            target_path,
            status: ClipExportStatus::parse(&status)?,
            error_code,
            output_sha256,
            output_byte_length: bytes
                .map(u64::try_from)
                .transpose()
                .map_err(|_| Error::Invalid("clip export output"))?,
            output_media_profile: profile,
        })
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
    pub fn match_for_demo(&self, demo_sha256: &str) -> Result<Option<MatchId>> {
        self.connection
            .query_row(
                "SELECT id FROM matches WHERE demo_sha256=?",
                [demo_sha256],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.map(MatchId))
            .map_err(Error::from)
    }
    pub fn completed_analysis_for_identity(
        &self,
        match_id: &MatchId,
        identity: &AnalysisIdentity,
    ) -> Result<bool> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM analysis_runs WHERE match_id=? AND parser_commit=? AND parser_build=? AND generated_proto_build=? AND requested_schema_hash=? AND metric_definition_version=? AND formula_id=? AND evidence_semantics_epoch=? AND status='succeeded')", params![match_id.as_str(), identity.parser_commit, identity.parser_build, identity.generated_proto_build, identity.requested_schema_hash, identity.metric_definition_version, identity.formula_id, identity.evidence_semantics_epoch], |row| row.get(0)).map_err(Error::from)
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
    pub fn persist_available_rating(
        &self,
        run: &AnalysisRunId,
        steam_id: &str,
        formula_id: &str,
        rating_bp: i64,
        receipt: &ReceiptId,
    ) -> Result<RatingVectorId> {
        let id = RatingVectorId::new();
        self.connection.execute("INSERT INTO player_rating_vectors(id,analysis_run_id,steam_id,formula_id,rating_bp,receipt_id) VALUES(?,?,?,?,?,?)", params![id.as_str(), run.as_str(), steam_id, formula_id, rating_bp, receipt.as_str()])?;
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn persist_rating_component(
        &self,
        vector: &RatingVectorId,
        key: &str,
        numerator: i64,
        denominator: i64,
        value_bp: i64,
        weight_bp: i64,
        receipt: &ReceiptId,
    ) -> Result<()> {
        self.connection.execute("INSERT INTO player_rating_components(rating_vector_id,component_key,numerator,denominator,value_bp,weight_bp,receipt_id) VALUES(?,?,?,?,?,?,?)", params![vector.as_str(), key, numerator, denominator, value_bp, weight_bp, receipt.as_str()])?;
        Ok(())
    }
    pub fn persist_unavailable_rating(
        &self,
        run: &AnalysisRunId,
        steam_id: &str,
        formula_id: &str,
        reason_code: &str,
        receipt: &ReceiptId,
    ) -> Result<()> {
        self.connection.execute("INSERT INTO unavailable_rating_results(analysis_run_id,steam_id,formula_id,reason_code,receipt_id) VALUES(?,?,?,?,?)", params![run.as_str(), steam_id, formula_id, reason_code, receipt.as_str()])?;
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
    pub fn import_job_by_id(&self, id: &str) -> Result<ImportJob> {
        self.import_job(&ImportJobId(id.to_owned()))
    }
    pub fn list_matches(&self) -> Result<Vec<MatchSummaryRecord>> {
        let mut statement = self.connection.prepare("SELECT id,map_name,imported_at_ms,status,canonical_run_id,local_steam_id FROM matches WHERE status<>'deleted' ORDER BY imported_at_ms DESC,id DESC")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut matches = Vec::new();
        for row in rows {
            let (id, map_name, imported_at_ms, status, run, local) = row?;
            let rating = self.rating_availability(run.as_deref(), &local)?;
            matches.push(MatchSummaryRecord {
                id,
                map_name,
                imported_at_ms,
                status,
                rating,
            });
        }
        Ok(matches)
    }
    #[allow(clippy::type_complexity)]
    pub fn match_detail(&self, id: &str) -> Result<MatchDetailRecord> {
        let row: Option<(String, Option<String>, i64, String, String, String, Option<String>, Option<String>)> = self.connection.query_row("SELECT id,map_name,imported_at_ms,status,demo_sha256,local_steam_id,game_build,canonical_run_id FROM matches WHERE id=? AND status<>'deleted'", [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
        let (
            id,
            map_name,
            imported_at_ms,
            status,
            demo_sha256,
            local_steam_id,
            game_build,
            canonical_run_id,
        ) = row.ok_or(Error::NotFound("match"))?;
        let rating = self.rating_availability(canonical_run_id.as_deref(), &local_steam_id)?;
        let components = if let Some(run) = &canonical_run_id {
            let mut statement = self.connection.prepare("SELECT c.component_key,c.numerator,c.denominator,c.value_bp,c.weight_bp,c.receipt_id FROM player_rating_components c JOIN player_rating_vectors v ON v.id=c.rating_vector_id WHERE v.analysis_run_id=? AND v.steam_id=? ORDER BY c.component_key")?;
            statement
                .query_map(params![run, local_steam_id], |row| {
                    Ok(RatingComponentRecord {
                        component_key: row.get(0)?,
                        numerator: row.get(1)?,
                        denominator: row.get(2)?,
                        value_bp: row.get(3)?,
                        weight_bp: row.get(4)?,
                        receipt_id: row.get(5)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let receipt_ids = if let Some(run) = &canonical_run_id {
            let mut statement = self.connection.prepare("SELECT id FROM receipts WHERE analysis_run_id=? ORDER BY COALESCE(event_tick,-1),COALESCE(ingestion_ordinal,-1),id")?;
            statement
                .query_map([run], |row| row.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?
        } else {
            Vec::new()
        };
        Ok(MatchDetailRecord {
            summary: MatchSummaryRecord {
                id,
                map_name,
                imported_at_ms,
                status,
                rating,
            },
            demo_sha256,
            local_steam_id,
            game_build,
            canonical_run_id,
            components,
            receipt_ids,
        })
    }
    pub fn receipt_by_id(&self, id: &str) -> Result<ReceiptRecord> {
        self.connection.query_row("SELECT id,analysis_run_id,round_id,metric_key,event_tick,ingestion_ordinal,participant_steam_ids_json,raw_payload_json,snapshots_json,parameters_json FROM receipts WHERE id=?", [id], |row| Ok(ReceiptRecord { id: row.get(0)?, analysis_run_id: row.get(1)?, round_id: row.get(2)?, metric_key: row.get(3)?, event_tick: row.get(4)?, ingestion_ordinal: row.get(5)?, participant_steam_ids_json: row.get(6)?, raw_payload_json: row.get(7)?, snapshots_json: row.get(8)?, parameters_json: row.get(9)? })).optional()?.ok_or(Error::NotFound("receipt"))
    }
    pub fn list_clips(&self) -> Result<Vec<ClipSummaryRecord>> {
        let mut statement = self.connection.prepare("SELECT id,title,disposition,recorded_at_ms,created_at_ms FROM clips WHERE disposition<>'deleted' ORDER BY recorded_at_ms DESC,created_at_ms DESC,id DESC")?;
        Ok(statement
            .query_map([], |row| {
                Ok(ClipSummaryRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    disposition: row.get(2)?,
                    recorded_at_ms: row.get(3)?,
                    created_at_ms: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    #[allow(clippy::type_complexity)]
    pub fn clip_detail(&self, id: &str) -> Result<ClipDetailRecord> {
        let row: Option<(String,Option<String>,String,i64,i64,String,String,i64,String,i64,String,Option<String>,String,Option<String>)> = self.connection.query_row("SELECT c.id,c.title,c.disposition,c.recorded_at_ms,c.created_at_ms,c.artifact_sha256,c.provenance,c.favorite,c.review_decision,c.pre_roll_truncated,c.retention_class,c.note,a.availability,a.missing_reason FROM clips c JOIN artifacts a ON a.sha256=c.artifact_sha256 WHERE c.id=? AND c.disposition<>'deleted'", [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?,row.get(12)?,row.get(13)?))).optional()?;
        let (
            id,
            title,
            disposition,
            recorded_at_ms,
            created_at_ms,
            artifact_sha256,
            provenance,
            favorite,
            review_decision,
            pre_roll_truncated,
            retention_class,
            note,
            availability,
            reason,
        ) = row.ok_or(Error::NotFound("clip"))?;
        let artifact_availability = if availability == "missing"
            && reason
                .as_deref()
                .is_some_and(|value| value.starts_with("quarantined:"))
        {
            ArtifactAvailability::Quarantined
        } else {
            ArtifactAvailability::parse(&availability)?
        };
        let mut tags = self
            .connection
            .prepare("SELECT tag FROM clip_tags WHERE clip_id=? ORDER BY position")?
            .query_map([&id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        tags.shrink_to_fit();
        Ok(ClipDetailRecord {
            summary: ClipSummaryRecord {
                id,
                title,
                disposition,
                recorded_at_ms,
                created_at_ms,
            },
            artifact_sha256,
            artifact_availability,
            provenance,
            favorite: favorite != 0,
            review_decision: ClipReviewDecision::parse(&review_decision)?,
            pre_roll_truncated: pre_roll_truncated != 0,
            retention_class,
            note,
            tags,
        })
    }

    pub fn clip_artifact_file(&self, id: &str) -> Result<ClipArtifactRecord> {
        let row: Option<(String, String, i64, Option<String>)> = self
            .connection
            .query_row(
                "SELECT a.sha256,a.relative_path,a.byte_length,a.media_type FROM clips c JOIN artifacts a ON a.sha256=c.artifact_sha256 WHERE c.id=? AND c.disposition<>'deleted' AND a.availability='present'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (sha256, relative_path, byte_length, media_type) =
            row.ok_or(Error::NotFound("present clip artifact"))?;
        let path = self.safe_artifact_path(&sha256, &relative_path, byte_length)?;
        Ok(ClipArtifactRecord {
            clip_id: id.to_owned(),
            sha256,
            path,
            byte_length,
            media_type,
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn durable_clip_model(&self, id: &str) -> Result<DurableClipModelRecord> {
        let detail = self.clip_detail(id)?;
        let artifact = self.clip_artifact_file(id)?;
        let row: Option<(String, Option<i64>, Option<i64>, Option<String>, Option<i64>)> = self
            .connection
            .query_row(
                "SELECT c.capture_session_id,c.review_revision,a.media_duration_ms,c.origin_kind,c.reviewed_at_ms FROM clips c JOIN artifacts a ON a.sha256=c.artifact_sha256 WHERE c.id=? AND c.disposition<>'deleted'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
        let (capture_session_id, revision, duration, origin_kind, reviewed_at_ms) =
            row.ok_or(Error::NotFound("live clip"))?;
        let revision = revision.ok_or(Error::Unavailable("clip review revision"))?;
        let duration = duration.ok_or(Error::Unavailable("verified media duration"))?;
        let origin_kind = origin_kind.ok_or(Error::Unavailable("clip origin evidence"))?;
        if detail.review_decision != ClipReviewDecision::Pending && reviewed_at_ms.is_none() {
            return Err(Error::Unavailable("clip review time"));
        }
        let origin = match origin_kind.as_str() {
            "auto" => {
                let trigger_receipts = self.origin_receipts(id, "trigger")?;
                let evidence_receipts = self.origin_receipts(id, "evidence")?;
                if trigger_receipts.is_empty() || evidence_receipts.is_empty() {
                    return Err(Error::Unavailable("clip origin evidence"));
                }
                DurableClipOriginRecord::Auto {
                    trigger_receipts,
                    evidence_receipts,
                }
            }
            "manual" => {
                let manual = self.origin_receipts(id, "manual_flag")?;
                let [flag_receipt_id] = manual.as_slice() else {
                    return Err(Error::Unavailable("clip origin evidence"));
                };
                let flag_time_ms: Option<i64> = self.connection.query_row(
                    "SELECT manual_flag_time_ms FROM clips WHERE id=?",
                    [id],
                    |row| row.get(0),
                )?;
                let flag_time_ms = flag_time_ms.ok_or(Error::Unavailable("clip flag time"))?;
                DurableClipOriginRecord::Manual {
                    flag_receipt_id: flag_receipt_id.clone(),
                    flag_time_ms: u64::try_from(flag_time_ms)
                        .map_err(|_| Error::Invalid("clip flag time"))?,
                    overlapping_auto_receipts: self.origin_receipts(id, "overlapping_auto")?,
                }
            }
            _ => return Err(Error::Invalid("clip origin kind")),
        };
        Ok(DurableClipModelRecord {
            detail,
            artifact,
            capture_session_id,
            duration_ms: u64::try_from(duration)
                .map_err(|_| Error::Invalid("verified media duration"))?,
            revision: u64::try_from(revision)
                .map_err(|_| Error::Invalid("clip review revision"))?,
            reviewed_at_ms: reviewed_at_ms
                .map(u64::try_from)
                .transpose()
                .map_err(|_| Error::Invalid("clip review time"))?,
            origin,
        })
    }

    fn origin_receipts(&self, clip_id: &str, kind: &str) -> Result<Vec<String>> {
        Ok(self
            .connection
            .prepare(
                "SELECT receipt_id FROM clip_origin_receipts WHERE clip_id=? AND kind=? ORDER BY position",
            )?
            .query_map(params![clip_id, kind], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn safe_artifact_path(
        &self,
        sha256: &str,
        relative_path: &str,
        byte_length: i64,
    ) -> Result<PathBuf> {
        if sha256.len() != 64 {
            return Err(Error::Invalid("artifact sha256"));
        }
        let relative = Path::new(relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(Error::Invalid("unsafe artifact path"));
        }
        let expected_prefix = Path::new("artifacts").join("sha256").join(&sha256[..2]);
        if !relative.starts_with(&expected_prefix)
            || relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !name.starts_with(&format!("{sha256}.")))
        {
            return Err(Error::Invalid("artifact catalog path mismatch"));
        }
        let mut current = self.layout.root.clone();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(Error::Invalid("unsafe artifact path"));
            };
            current.push(component);
            if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                return Err(Error::Invalid("artifact path symlink"));
            }
        }
        let metadata = fs::metadata(&current)?;
        if !metadata.is_file()
            || i64::try_from(metadata.len()).map_err(|_| Error::Invalid("artifact too large"))?
                != byte_length
        {
            return Err(Error::Invalid("artifact file metadata mismatch"));
        }
        Ok(current)
    }

    fn rating_availability(
        &self,
        run: Option<&str>,
        local: &str,
    ) -> Result<StoredRatingAvailability> {
        let Some(run) = run else {
            return Ok(StoredRatingAvailability::Pending);
        };
        let available: Option<(String,i64,String)> = self.connection.query_row("SELECT formula_id,rating_bp,receipt_id FROM player_rating_vectors WHERE analysis_run_id=? AND steam_id=?", params![run,local], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((formula_id, rating_bp, receipt_id)) = available {
            return Ok(StoredRatingAvailability::Available {
                formula_id,
                rating_bp,
                receipt_id,
            });
        }
        let unavailable: Option<(String,String,String)> = self.connection.query_row("SELECT formula_id,reason_code,receipt_id FROM unavailable_rating_results WHERE analysis_run_id=? AND steam_id=?", params![run,local], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        Ok(unavailable.map_or(
            StoredRatingAvailability::Pending,
            |(formula_id, reason_code, receipt_id)| StoredRatingAvailability::Unavailable {
                formula_id,
                reason_code,
                receipt_id,
            },
        ))
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

fn positive_i64(value: u64, field: &'static str) -> Result<i64> {
    if value == 0 {
        return Err(Error::Invalid(field));
    }
    i64::try_from(value).map_err(|_| Error::Invalid(field))
}

fn validate_review_metadata(
    title: Option<&str>,
    note: Option<&str>,
    tags: &[String],
) -> Result<()> {
    if title.is_some_and(|value| value.len() > 200)
        || note.is_some_and(|value| value.len() > 4_000)
        || tags.len() > 32
    {
        return Err(Error::Invalid("clip review metadata limits"));
    }
    let mut unique = HashSet::with_capacity(tags.len());
    if tags
        .iter()
        .any(|tag| tag.trim().is_empty() || tag.len() > 64 || !unique.insert(tag.as_str()))
    {
        return Err(Error::Invalid("clip tags"));
    }
    Ok(())
}

fn replace_clip_tags(
    transaction: &rusqlite::Transaction<'_>,
    clip_id: &str,
    tags: &[String],
) -> Result<()> {
    transaction.execute("DELETE FROM clip_tags WHERE clip_id=?", [clip_id])?;
    for (position, tag) in tags.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| Error::Invalid("clip tag position"))?;
        transaction.execute(
            "INSERT INTO clip_tags(clip_id,position,tag) VALUES(?,?,?)",
            params![clip_id, position, tag],
        )?;
    }
    Ok(())
}

fn validate_origin(origin: DurableClipOriginInput<'_>) -> Result<()> {
    let validate_group = |receipts: &[String], required: bool| -> Result<()> {
        if required && receipts.is_empty() {
            return Err(Error::Invalid("clip origin evidence"));
        }
        let mut unique = HashSet::with_capacity(receipts.len());
        if receipts
            .iter()
            .any(|receipt| receipt.trim().is_empty() || !unique.insert(receipt.as_str()))
        {
            return Err(Error::Invalid("clip origin evidence"));
        }
        Ok(())
    };
    match origin {
        DurableClipOriginInput::Auto {
            trigger_receipts,
            evidence_receipts,
        } => {
            validate_group(trigger_receipts, true)?;
            validate_group(evidence_receipts, true)
        }
        DurableClipOriginInput::Manual {
            flag_receipt_id,
            overlapping_auto_receipts,
            ..
        } => {
            if flag_receipt_id.trim().is_empty() {
                return Err(Error::Invalid("clip origin evidence"));
            }
            validate_group(overlapping_auto_receipts, false)
        }
    }
}

fn insert_receipt_group(
    transaction: &rusqlite::Transaction<'_>,
    clip_id: &str,
    kind: &str,
    receipts: &[String],
) -> Result<()> {
    for (position, receipt) in receipts.iter().enumerate() {
        let position =
            i64::try_from(position).map_err(|_| Error::Invalid("clip receipt position"))?;
        transaction.execute(
            "INSERT INTO clip_origin_receipts(clip_id,kind,position,receipt_id) VALUES(?,?,?,?)",
            params![clip_id, kind, position, receipt],
        )?;
    }
    Ok(())
}

fn insert_origin_receipts(
    transaction: &rusqlite::Transaction<'_>,
    clip_id: &str,
    origin: DurableClipOriginInput<'_>,
) -> Result<()> {
    match origin {
        DurableClipOriginInput::Auto {
            trigger_receipts,
            evidence_receipts,
        } => {
            insert_receipt_group(transaction, clip_id, "trigger", trigger_receipts)?;
            insert_receipt_group(transaction, clip_id, "evidence", evidence_receipts)
        }
        DurableClipOriginInput::Manual {
            flag_receipt_id,
            overlapping_auto_receipts,
            ..
        } => {
            transaction.execute(
                "INSERT INTO clip_origin_receipts(clip_id,kind,position,receipt_id) VALUES(?,'manual_flag',0,?)",
                params![clip_id, flag_receipt_id],
            )?;
            insert_receipt_group(
                transaction,
                clip_id,
                "overlapping_auto",
                overlapping_auto_receipts,
            )
        }
    }
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
fn ensure_migration(connection: &Connection, version: i64, sql: &str) -> Result<()> {
    let checksum = hex_sha256(sql.as_bytes());
    let found: Option<String> = connection
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version=?",
            [version],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(found) = found {
        if found != checksum {
            return Err(Error::Invalid("migration checksum mismatch"));
        }
        return Ok(());
    }
    apply_migration(connection, version, sql, &checksum)
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
    fn stored_clip(storage: &Storage, request: &str, bytes: &[u8]) -> (ClipId, String) {
        let hash = storage
            .commit_artifact(
                storage.stage_artifact(bytes).unwrap(),
                "mkv",
                Some("video/x-matroska"),
            )
            .unwrap();
        let session = storage.create_capture_session("765").unwrap();
        let attempt = storage.request_save(&session, request, 1).unwrap();
        storage.complete_save(&attempt, &hash, 1, 2).unwrap();
        (storage.create_clip(&attempt, "raw_manual").unwrap(), hash)
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
    fn manual_flag_save_join_is_idempotent_and_survives_reopen() {
        let (dir, storage) = store();
        let session = storage.create_capture_session("765").unwrap();
        let flag = storage
            .create_manual_flag(&session, 42_000_000_000)
            .unwrap();
        let attempt = storage
            .request_save(&session, "manual:42000:1", 42_000_000_000)
            .unwrap();
        storage
            .join_manual_flag_save(&flag, &attempt, 27_000_000_000, 42_000_000_000)
            .unwrap();
        storage
            .join_manual_flag_save(&flag, &attempt, 27_000_000_000, 42_000_000_000)
            .unwrap();
        drop(storage);

        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        assert_eq!(
            reopened.manual_flag_save_link(&flag, &attempt).unwrap(),
            ManualFlagSaveLink {
                manual_flag_id: flag,
                save_attempt_id: attempt,
                desired_start_monotonic_ns: 27_000_000_000,
                desired_end_monotonic_ns: 42_000_000_000,
            }
        );
    }
    #[test]
    fn manual_flag_save_join_rejects_range_mutation_and_reversed_ranges() {
        let (_dir, storage) = store();
        let session = storage.create_capture_session("765").unwrap();
        let flag = storage.create_manual_flag(&session, 42).unwrap();
        let attempt = storage.request_save(&session, "manual-range", 42).unwrap();
        storage
            .join_manual_flag_save(&flag, &attempt, 27, 42)
            .unwrap();
        assert!(matches!(
            storage.join_manual_flag_save(&flag, &attempt, 26, 42),
            Err(Error::Conflict("manual flag save join"))
        ));
        assert!(matches!(
            storage.join_manual_flag_save(&flag, &attempt, 43, 42),
            Err(Error::Invalid("manual flag save range"))
        ));
        assert_eq!(
            storage
                .manual_flag_save_link(&flag, &attempt)
                .unwrap()
                .desired_start_monotonic_ns,
            27
        );
    }
    #[test]
    fn manual_flag_save_join_rejects_cross_session_relationships() {
        let (_dir, storage) = store();
        let flag_session = storage.create_capture_session("765").unwrap();
        let attempt_session = storage.create_capture_session("765").unwrap();
        let flag = storage.create_manual_flag(&flag_session, 42).unwrap();
        let attempt = storage
            .request_save(&attempt_session, "wrong-session", 42)
            .unwrap();
        assert!(matches!(
            storage.join_manual_flag_save(&flag, &attempt, 27, 42),
            Err(Error::Invalid("manual flag save session mismatch"))
        ));
    }
    #[test]
    fn clip_review_metadata_and_rejection_survive_reopen() {
        let (dir, storage) = store();
        let (clip, hash) = stored_clip(&storage, "review", b"review-media");
        storage
            .update_clip_review(
                clip.as_str(),
                Some("Three-kill hold"),
                Some("Good crosshair placement"),
                &["mirage".to_owned(), "rifle".to_owned()],
                ClipReviewDecision::Reject,
                true,
            )
            .unwrap();
        drop(storage);

        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        let detail = reopened.clip_detail(clip.as_str()).unwrap();
        assert_eq!(detail.summary.title.as_deref(), Some("Three-kill hold"));
        assert_eq!(detail.note.as_deref(), Some("Good crosshair placement"));
        assert_eq!(detail.tags, ["mirage", "rifle"]);
        assert_eq!(detail.review_decision, ClipReviewDecision::Reject);
        assert!(detail.favorite);
        assert_eq!(
            reopened.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Present
        );
    }
    #[test]
    fn invalid_review_update_is_atomic() {
        let (_dir, storage) = store();
        let (clip, _) = stored_clip(&storage, "atomic-review", b"atomic-media");
        storage
            .update_clip_review(
                clip.as_str(),
                Some("Original"),
                Some("Original note"),
                &["first".to_owned()],
                ClipReviewDecision::Keep,
                false,
            )
            .unwrap();
        assert!(matches!(
            storage.update_clip_review(
                clip.as_str(),
                Some("Replacement"),
                None,
                &["duplicate".to_owned(), "duplicate".to_owned()],
                ClipReviewDecision::Reject,
                true,
            ),
            Err(Error::Invalid("clip tags"))
        ));
        let detail = storage.clip_detail(clip.as_str()).unwrap();
        assert_eq!(detail.summary.title.as_deref(), Some("Original"));
        assert_eq!(detail.note.as_deref(), Some("Original note"));
        assert_eq!(detail.tags, ["first"]);
        assert_eq!(detail.review_decision, ClipReviewDecision::Keep);
        assert!(!detail.favorite);
    }
    #[test]
    fn clip_artifact_lookup_returns_only_verified_catalog_file() {
        let (_dir, storage) = store();
        let (clip, hash) = stored_clip(&storage, "lookup", b"lookup-media");
        let file = storage.clip_artifact_file(clip.as_str()).unwrap();
        assert_eq!(file.sha256, hash);
        assert_eq!(file.byte_length, 12);
        assert_eq!(file.media_type.as_deref(), Some("video/x-matroska"));
        assert_eq!(fs::read(file.path).unwrap(), b"lookup-media");
    }
    #[cfg(unix)]
    #[test]
    fn clip_artifact_lookup_rejects_catalog_path_symlinks() {
        use std::os::unix::fs::symlink;
        let (dir, storage) = store();
        let (clip, hash) = stored_clip(&storage, "lookup-symlink", b"media");
        let relative = storage.artifact(&hash).unwrap().relative_path.unwrap();
        let path = storage.layout().root.join(relative);
        fs::remove_file(&path).unwrap();
        let outside = dir.path().join("outside.mkv");
        fs::write(&outside, b"media").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(matches!(
            storage.clip_artifact_file(clip.as_str()),
            Err(Error::Invalid("artifact path symlink"))
        ));
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
    fn available_rating_public_seam_satisfies_canonical_invariant() {
        let (_dir, mut storage) = store();
        let demo = storage
            .commit_artifact(storage.stage_artifact(b"rated-demo").unwrap(), "dem", None)
            .unwrap();
        storage.upsert_player("765", None).unwrap();
        let match_id = storage.import_match(&demo, "765", None).unwrap();
        storage
            .add_match_participant(&match_id, "765", "full")
            .unwrap();
        let identity = AnalysisIdentity {
            parser_commit: "a",
            parser_build: "b",
            generated_proto_build: "c",
            requested_schema_hash: "d",
            metric_definition_version: "e",
            formula_id: "ofr-1.0.0",
            evidence_semantics_epoch: "g",
        };
        let run = storage.begin_analysis(&match_id, &identity).unwrap();
        let receipt = storage
            .add_receipt(
                &run,
                None,
                "ofr-1.0.0",
                None,
                None,
                "[\"765\"]",
                "{}",
                "[]",
                "{}",
            )
            .unwrap();
        let vector = storage
            .persist_available_rating(&run, "765", "ofr-1.0.0", 10_220, &receipt)
            .unwrap();
        storage
            .persist_rating_component(&vector, "direct_damage", 1, 2, 5_000, 3_000, &receipt)
            .unwrap();
        storage.complete_analysis(&run).unwrap();
    }
    #[test]
    fn unavailable_rating_public_seam_satisfies_canonical_invariant_without_zero() {
        let (_dir, mut storage) = store();
        let demo = storage
            .commit_artifact(
                storage.stage_artifact(b"unavailable-demo").unwrap(),
                "dem",
                None,
            )
            .unwrap();
        storage.upsert_player("765", None).unwrap();
        let match_id = storage.import_match(&demo, "765", None).unwrap();
        storage
            .add_match_participant(&match_id, "765", "full")
            .unwrap();
        let identity = AnalysisIdentity {
            parser_commit: "a",
            parser_build: "b",
            generated_proto_build: "c",
            requested_schema_hash: "d",
            metric_definition_version: "e",
            formula_id: "ofr-1.0.0",
            evidence_semantics_epoch: "g",
        };
        let run = storage.begin_analysis(&match_id, &identity).unwrap();
        let receipt = storage
            .add_receipt(
                &run,
                None,
                "rating_unavailable",
                None,
                None,
                "[\"765\"]",
                "{}",
                "[]",
                "{}",
            )
            .unwrap();
        storage
            .persist_unavailable_rating(&run, "765", "ofr-1.0.0", "missing_tick_rate", &receipt)
            .unwrap();
        storage.complete_analysis(&run).unwrap();
        let summary = storage.list_matches().unwrap().pop().unwrap();
        assert!(
            matches!(summary.rating, StoredRatingAvailability::Unavailable { ref reason_code, .. } if reason_code == "missing_tick_rate")
        );
    }
    #[test]
    fn dashboard_read_models_return_jobs_matches_receipts_components_and_clips() {
        let (_dir, mut storage) = store();
        let demo = storage
            .commit_artifact(storage.stage_artifact(b"read-demo").unwrap(), "dem", None)
            .unwrap();
        let job = storage.enqueue_import(&demo).unwrap();
        assert_eq!(
            storage.import_job_by_id(job.as_str()).unwrap().phase,
            ImportPhase::Queued
        );
        storage.upsert_player("765", Some("Local")).unwrap();
        let match_id = storage
            .import_match(&demo, "765", Some("de_mirage"))
            .unwrap();
        storage
            .add_match_participant(&match_id, "765", "full")
            .unwrap();
        let identity = AnalysisIdentity {
            parser_commit: "a",
            parser_build: "b",
            generated_proto_build: "c",
            requested_schema_hash: "d",
            metric_definition_version: "e",
            formula_id: "ofr-1.0.0",
            evidence_semantics_epoch: "g",
        };
        let run = storage.begin_analysis(&match_id, &identity).unwrap();
        let receipt = storage
            .add_receipt(
                &run,
                None,
                "ofr-1.0.0",
                None,
                None,
                "[\"765\"]",
                "{\"rating_bp\":10220}",
                "[]",
                "{}",
            )
            .unwrap();
        let vector = storage
            .persist_available_rating(&run, "765", "ofr-1.0.0", 10_220, &receipt)
            .unwrap();
        storage
            .persist_rating_component(&vector, "direct_damage", 1, 2, 5_000, 3_000, &receipt)
            .unwrap();
        storage.complete_analysis(&run).unwrap();
        let summary = storage.list_matches().unwrap().pop().unwrap();
        assert!(matches!(
            summary.rating,
            StoredRatingAvailability::Available {
                rating_bp: 10_220,
                ..
            }
        ));
        let detail = storage.match_detail(match_id.as_str()).unwrap();
        assert_eq!(detail.components.len(), 1);
        assert_eq!(detail.receipt_ids, vec![receipt.as_str().to_owned()]);
        assert_eq!(
            storage.receipt_by_id(receipt.as_str()).unwrap().metric_key,
            "ofr-1.0.0"
        );

        let media = storage
            .commit_artifact(storage.stage_artifact(b"media").unwrap(), "mkv", None)
            .unwrap();
        let session = storage.create_capture_session("765").unwrap();
        let attempt = storage.request_save(&session, "read-clip", 1).unwrap();
        storage.complete_save(&attempt, &media, 1, 2).unwrap();
        let clip = storage.create_clip(&attempt, "raw_manual").unwrap();
        storage
            .set_clip_review(&clip, ClipDisposition::Kept, Some("Ace"), true)
            .unwrap();
        assert_eq!(
            storage.list_clips().unwrap()[0].title.as_deref(),
            Some("Ace")
        );
        let clip_detail = storage.clip_detail(clip.as_str()).unwrap();
        assert!(clip_detail.favorite);
        assert_eq!(clip_detail.note, None);
        assert!(clip_detail.tags.is_empty());
    }
    #[test]
    fn quarantine_moves_unreferenced_artifact_and_records_reasoned_state() {
        let (_dir, mut storage) = store();
        let hash = storage
            .commit_artifact(
                storage.stage_artifact(b"corrupt-demo").unwrap(),
                "dem",
                None,
            )
            .unwrap();
        let destination = storage
            .quarantine_artifact(&hash, "parser_corrupt")
            .unwrap();
        assert!(destination.is_file());
        assert_eq!(
            storage.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Quarantined
        );
    }
    #[test]
    fn quarantine_rejects_referenced_artifact_without_moving_it() {
        let (_dir, mut storage) = store();
        let hash = storage
            .commit_artifact(
                storage.stage_artifact(b"referenced-demo").unwrap(),
                "dem",
                None,
            )
            .unwrap();
        storage.import_match(&hash, "765", None).unwrap();
        assert!(matches!(
            storage.quarantine_artifact(&hash, "bad"),
            Err(Error::Conflict(_))
        ));
        assert_eq!(
            storage.artifact(&hash).unwrap().availability,
            ArtifactAvailability::Present
        );
    }
    #[cfg(unix)]
    #[test]
    fn quarantine_rejects_artifact_symlink() {
        use std::os::unix::fs::symlink;
        let (dir, mut storage) = store();
        let hash = storage
            .commit_artifact(
                storage.stage_artifact(b"symlink-demo").unwrap(),
                "dem",
                None,
            )
            .unwrap();
        let relative = storage.artifact(&hash).unwrap().relative_path.unwrap();
        let path = storage.layout.root.join(relative);
        fs::remove_file(&path).unwrap();
        symlink(dir.path().join("outside"), &path).unwrap();
        assert!(matches!(
            storage.quarantine_artifact(&hash, "bad"),
            Err(Error::Invalid("artifact symlink"))
        ));
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

    #[test]
    fn legacy_clip_stays_unavailable_until_verified_model_metadata_is_supplied() {
        let (dir, storage) = store();
        let (clip, _) = stored_clip(&storage, "legacy-model", b"legacy media");
        assert!(matches!(
            storage.durable_clip_model(clip.as_str()),
            Err(Error::Unavailable(_))
        ));
        storage
            .complete_clip_model_metadata(
                clip.as_str(),
                42_000,
                DurableClipOriginInput::Manual {
                    flag_receipt_id: "flag-receipt",
                    flag_time_ms: 12_000,
                    overlapping_auto_receipts: &["auto-overlap".into()],
                },
            )
            .unwrap();
        drop(storage);

        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        let model = reopened.durable_clip_model(clip.as_str()).unwrap();
        assert_eq!(model.duration_ms, 42_000);
        assert_eq!(model.revision, 0);
        assert!(!model.capture_session_id.is_empty());
        assert_eq!(
            model.origin,
            DurableClipOriginRecord::Manual {
                flag_receipt_id: "flag-receipt".into(),
                flag_time_ms: 12_000,
                overlapping_auto_receipts: vec!["auto-overlap".into()],
            }
        );
    }

    #[test]
    fn clip_review_compare_and_swap_is_durable_and_rejects_stale_revision() {
        let (dir, storage) = store();
        let (clip, _) = stored_clip(&storage, "revisioned-review", b"review media");
        storage
            .complete_clip_model_metadata(
                clip.as_str(),
                10_000,
                DurableClipOriginInput::Auto {
                    trigger_receipts: &["trigger".into()],
                    evidence_receipts: &["evidence".into()],
                },
            )
            .unwrap();
        let revision = storage
            .compare_and_swap_clip_review(
                clip.as_str(),
                0,
                Some("Kept"),
                Some("Reviewed"),
                &["clutch".into()],
                ClipReviewDecision::Keep,
                true,
            )
            .unwrap();
        assert_eq!(revision, 1);
        assert!(matches!(
            storage.compare_and_swap_clip_review(
                clip.as_str(),
                0,
                Some("Stale"),
                None,
                &[],
                ClipReviewDecision::Reject,
                false,
            ),
            Err(Error::Conflict("clip review revision"))
        ));
        drop(storage);

        let reopened = Storage::open(Layout::at(dir.path())).unwrap();
        let model = reopened.durable_clip_model(clip.as_str()).unwrap();
        assert_eq!(model.revision, 1);
        assert!(model.reviewed_at_ms.is_some());
        assert_eq!(model.detail.review_decision, ClipReviewDecision::Keep);
        assert_eq!(model.detail.tags, ["clutch"]);
    }

    #[test]
    fn derivative_commit_publishes_artifact_clip_origin_and_derivation_together() {
        let (_dir, storage) = store();
        let (source, _) = stored_clip(&storage, "derivative-source", b"source media");
        storage
            .complete_clip_model_metadata(
                source.as_str(),
                42_000,
                DurableClipOriginInput::Auto {
                    trigger_receipts: &["trigger".into()],
                    evidence_receipts: &["evidence".into()],
                },
            )
            .unwrap();
        let derived = storage
            .commit_clip_derivative(DerivativeClipCommit {
                source_clip_id: source.as_str(),
                staged: storage.stage_artifact(b"trimmed media").unwrap(),
                extension: "mp4",
                media_type: Some("video/mp4"),
                duration_ms: 30_000,
                trim_start_ms: 5_000,
                trim_end_ms: 35_000,
                title: "Trimmed clutch",
                note: "Local derivative",
                tags: &["trimmed".into()],
                favorite: true,
            })
            .unwrap();

        let model = storage.durable_clip_model(derived.as_str()).unwrap();
        assert_eq!(model.duration_ms, 30_000);
        assert_eq!(model.revision, 0);
        assert!(model.reviewed_at_ms.is_some());
        assert_eq!(model.detail.provenance, "trim_derivative");
        assert_eq!(model.detail.tags, ["trimmed"]);
        assert_eq!(
            model.origin,
            DurableClipOriginRecord::Auto {
                trigger_receipts: vec!["trigger".into()],
                evidence_receipts: vec!["evidence".into()],
            }
        );
        let derivations: i64 = storage
            .connection
            .query_row(
                "SELECT count(*) FROM clip_derivations WHERE derived_clip_id=? AND source_clip_id=? AND operation='trim' AND trim_start_ms=5000 AND trim_end_ms=35000",
                params![derived.as_str(), source.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derivations, 1);
    }

    #[test]
    fn derivative_transaction_failure_removes_published_file_and_database_rows() {
        let (_dir, storage) = store();
        let (source, _) = stored_clip(&storage, "failed-derivative", b"source media");
        storage
            .complete_clip_model_metadata(
                source.as_str(),
                42_000,
                DurableClipOriginInput::Manual {
                    flag_receipt_id: "manual-flag",
                    flag_time_ms: 2_000,
                    overlapping_auto_receipts: &[],
                },
            )
            .unwrap();
        let staged = storage.stage_artifact(b"do not publish").unwrap();
        let hash = staged.sha256.clone();
        let target = storage
            .layout()
            .artifacts
            .join(&hash[..2])
            .join(format!("{hash}.mp4"));
        storage
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_test_derivative BEFORE INSERT ON clip_derivations BEGIN SELECT RAISE(ABORT,'injected derivative failure'); END;",
            )
            .unwrap();

        assert!(matches!(
            storage.commit_clip_derivative(DerivativeClipCommit {
                source_clip_id: source.as_str(),
                staged,
                extension: "mp4",
                media_type: Some("video/mp4"),
                duration_ms: 10_000,
                trim_start_ms: 1_000,
                trim_end_ms: 11_000,
                title: "Failed derivative",
                note: "Must roll back",
                tags: &[],
                favorite: false,
            }),
            Err(Error::Database(_))
        ));
        assert!(!target.exists());
        assert!(matches!(storage.artifact(&hash), Err(Error::NotFound(_))));
        let derived_rows: i64 = storage
            .connection
            .query_row(
                "SELECT count(*) FROM clips WHERE artifact_sha256=?",
                [&hash],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_rows, 0);
    }

    #[test]
    fn clip_export_lifecycle_records_success_and_failure_terminal_states() {
        let (_dir, storage) = store();
        let (clip, _) = stored_clip(&storage, "export-source", b"export media");
        let succeeded = storage.enqueue_export(&clip, "/tmp/clip.mp4").unwrap();
        storage.begin_clip_export(&succeeded).unwrap();
        storage
            .succeed_clip_export(&succeeded, &"a".repeat(64), 123, "discord-h264-aac")
            .unwrap();
        let record = storage.clip_export(&succeeded).unwrap();
        assert_eq!(record.status, ClipExportStatus::Succeeded);
        assert_eq!(record.output_byte_length, Some(123));
        assert!(matches!(
            storage.fail_clip_export(&succeeded, "too_late"),
            Err(Error::IllegalTransition("clip export fail"))
        ));

        let failed = storage.enqueue_export(&clip, "/tmp/fail.mp4").unwrap();
        storage.fail_clip_export(&failed, "disk_full").unwrap();
        let record = storage.clip_export(&failed).unwrap();
        assert_eq!(record.status, ClipExportStatus::Failed);
        assert_eq!(record.error_code.as_deref(), Some("disk_full"));
    }
}
