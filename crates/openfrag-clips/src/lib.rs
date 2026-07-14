//! Local-only Clip review and export workflow.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod derivative;

pub use derivative::{
    CancellationToken, DerivativeProfile, DerivativeReviewError, DerivativeReviewRequest,
    DiscordEncodeProfile, FfmpegTranscoder, FfprobeMediaProbe, MAX_CAPTURED_PROCESS_BYTES,
    MediaInfo, MediaProbe, MediaProbeError, ReviewMetadata, TranscodeError, TranscodeRequest,
    Transcoder, prepare_derivative_and_review,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelError {
    EmptyIdentifier,
    EmptyHash,
    ZeroDuration,
    ReversedOrEmptyTrim,
    MissingEvidence,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClipId(String);

impl ClipId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        if value.is_empty() {
            Err(ModelError::EmptyIdentifier)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactProvenance {
    artifact_id: String,
    path: PathBuf,
    sha256: String,
    duration_ms: u64,
    bytes: u64,
}

impl ArtifactProvenance {
    pub fn new(
        artifact_id: impl Into<String>,
        path: PathBuf,
        sha256: impl Into<String>,
        duration_ms: u64,
        bytes: u64,
    ) -> Result<Self, ModelError> {
        let artifact_id = artifact_id.into();
        let sha256 = sha256.into();
        if artifact_id.is_empty() {
            return Err(ModelError::EmptyIdentifier);
        }
        if sha256.is_empty() {
            return Err(ModelError::EmptyHash);
        }
        if duration_ms == 0 {
            return Err(ModelError::ZeroDuration);
        }
        Ok(Self {
            artifact_id,
            path,
            sha256,
            duration_ms,
            bytes,
        })
    }

    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrimRange {
    start_ms: u64,
    end_ms: u64,
}

impl TrimRange {
    pub const fn new(start_ms: u64, end_ms: u64) -> Result<Self, ModelError> {
        if start_ms >= end_ms {
            Err(ModelError::ReversedOrEmptyTrim)
        } else {
            Ok(Self { start_ms, end_ms })
        }
    }

    pub const fn start_ms(self) -> u64 {
        self.start_ms
    }

    pub const fn end_ms(self) -> u64 {
        self.end_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DerivativeProvenance {
    artifact: ArtifactProvenance,
    source_artifact_id: String,
    trim: TrimRange,
}

impl DerivativeProvenance {
    pub fn new(
        artifact: ArtifactProvenance,
        source_artifact_id: impl Into<String>,
        trim: TrimRange,
    ) -> Self {
        Self {
            artifact,
            source_artifact_id: source_artifact_id.into(),
            trim,
        }
    }

    pub fn artifact(&self) -> &ArtifactProvenance {
        &self.artifact
    }

    pub fn source_artifact_id(&self) -> &str {
        &self.source_artifact_id
    }

    pub const fn trim(&self) -> TrimRange {
        self.trim
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipOrigin {
    AutoHighlight {
        trigger_receipts: BTreeSet<String>,
        evidence_receipts: BTreeSet<String>,
    },
    ManualFlag {
        flag_receipt_id: String,
        flag_time_ms: u64,
        overlapping_auto_receipts: BTreeSet<String>,
    },
}

impl ClipOrigin {
    pub fn auto_highlight(
        trigger_receipts: BTreeSet<String>,
        evidence_receipts: BTreeSet<String>,
    ) -> Result<Self, ModelError> {
        if trigger_receipts.is_empty() || evidence_receipts.is_empty() {
            Err(ModelError::MissingEvidence)
        } else {
            Ok(Self::AutoHighlight {
                trigger_receipts,
                evidence_receipts,
            })
        }
    }

    pub fn manual_flag(
        flag_receipt_id: impl Into<String>,
        flag_time_ms: u64,
        overlapping_auto_receipts: BTreeSet<String>,
    ) -> Result<Self, ModelError> {
        let flag_receipt_id = flag_receipt_id.into();
        if flag_receipt_id.is_empty() {
            Err(ModelError::MissingEvidence)
        } else {
            Ok(Self::ManualFlag {
                flag_receipt_id,
                flag_time_ms,
                overlapping_auto_receipts,
            })
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewState {
    Provisional,
    Reviewed { reviewed_at_ms: u64 },
    Rejected { rejected_at_ms: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Clip {
    id: ClipId,
    revision: u64,
    source_capture: ArtifactProvenance,
    capture_session_id: String,
    origin: ClipOrigin,
    review_state: ReviewState,
    title: String,
    note: String,
    tags: BTreeSet<String>,
    favorite: bool,
    derivative: Option<DerivativeProvenance>,
}

impl Clip {
    pub fn provisional(
        id: ClipId,
        source_capture: ArtifactProvenance,
        capture_session_id: impl Into<String>,
        origin: ClipOrigin,
        default_title: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let capture_session_id = capture_session_id.into();
        if capture_session_id.is_empty() {
            return Err(ModelError::EmptyIdentifier);
        }
        Ok(Self {
            id,
            revision: 0,
            source_capture,
            capture_session_id,
            origin,
            review_state: ReviewState::Provisional,
            title: default_title.into(),
            note: String::new(),
            tags: BTreeSet::new(),
            favorite: false,
            derivative: None,
        })
    }

    pub fn id(&self) -> &ClipId {
        &self.id
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn source_capture(&self) -> &ArtifactProvenance {
        &self.source_capture
    }

    pub fn capture_session_id(&self) -> &str {
        &self.capture_session_id
    }

    pub fn origin(&self) -> &ClipOrigin {
        &self.origin
    }

    pub fn review_state(&self) -> &ReviewState {
        &self.review_state
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn note(&self) -> &str {
        &self.note
    }

    pub fn tags(&self) -> &BTreeSet<String> {
        &self.tags
    }

    pub const fn favorite(&self) -> bool {
        self.favorite
    }

    pub fn derivative(&self) -> Option<&DerivativeProvenance> {
        self.derivative.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewEdit {
    pub title: String,
    pub note: String,
    pub tags: BTreeSet<String>,
    pub favorite: bool,
    pub derivative: Option<DerivativeProvenance>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryError {
    pub message: String,
}

impl RepositoryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait ClipRepository {
    fn compare_and_swap(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewError {
    TrimOutsideSource,
    DerivativeHasWrongSource,
    InvalidTransition,
    Repository(RepositoryError),
}

pub fn review_clip(
    repository: &mut impl ClipRepository,
    clip: &Clip,
    edit: ReviewEdit,
    reviewed_at_ms: u64,
) -> Result<Clip, ReviewError> {
    if matches!(clip.review_state, ReviewState::Rejected { .. }) {
        return Err(ReviewError::InvalidTransition);
    }
    if let Some(derivative) = &edit.derivative {
        if derivative.source_artifact_id != clip.source_capture.artifact_id {
            return Err(ReviewError::DerivativeHasWrongSource);
        }
        if derivative.trim.end_ms > clip.source_capture.duration_ms {
            return Err(ReviewError::TrimOutsideSource);
        }
    }
    let mut updated = clip.clone();
    updated.revision = updated.revision.saturating_add(1);
    updated.review_state = ReviewState::Reviewed { reviewed_at_ms };
    updated.title = edit.title;
    updated.note = edit.note;
    updated.tags = edit.tags;
    updated.favorite = edit.favorite;
    updated.derivative = edit.derivative;
    repository
        .compare_and_swap(clip.revision, &updated)
        .map_err(ReviewError::Repository)?;
    Ok(updated)
}

pub fn reject_clip(
    repository: &mut impl ClipRepository,
    clip: &Clip,
    rejected_at_ms: u64,
) -> Result<Clip, ReviewError> {
    if matches!(clip.review_state, ReviewState::Rejected { .. }) {
        return Err(ReviewError::InvalidTransition);
    }
    let mut updated = clip.clone();
    updated.revision = updated.revision.saturating_add(1);
    updated.review_state = ReviewState::Rejected { rejected_at_ms };
    repository
        .compare_and_swap(clip.revision, &updated)
        .map_err(ReviewError::Repository)?;
    Ok(updated)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileOperationError {
    DestinationExists,
    SourceMissing,
    Io(String),
}

pub trait LocalFileSystem {
    fn atomic_copy(&mut self, source: &Path, destination: &Path)
    -> Result<u64, FileOperationError>;
}

#[derive(Debug, Default)]
pub struct StdLocalFileSystem;

impl LocalFileSystem for StdLocalFileSystem {
    fn atomic_copy(
        &mut self,
        source: &Path,
        destination: &Path,
    ) -> Result<u64, FileOperationError> {
        let mut source_file = fs::File::open(source).map_err(|error| map_source_error(&error))?;
        let destination_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FileOperationError::Io("destination has no file name".into()))?;
        let destination_directory = destination
            .parent()
            .ok_or_else(|| FileOperationError::Io("destination has no parent directory".into()))?;
        let temporary_path = temporary_path(destination_directory, destination_name);
        let mut temporary_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| map_io_error(&error))?;
        let copied = match io::copy(&mut source_file, &mut temporary_file) {
            Ok(copied) => copied,
            Err(error) => {
                drop(temporary_file);
                let _ = fs::remove_file(&temporary_path);
                return Err(map_io_error(&error));
            }
        };
        if let Err(error) = temporary_file.sync_all() {
            drop(temporary_file);
            let _ = fs::remove_file(&temporary_path);
            return Err(map_io_error(&error));
        }
        drop(temporary_file);
        let publish_result = fs::hard_link(&temporary_path, destination);
        let _ = fs::remove_file(&temporary_path);
        publish_result.map_err(|error| map_destination_error(&error))?;
        Ok(copied)
    }
}

fn temporary_path(directory: &Path, destination_name: &str) -> PathBuf {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
    let temporary_id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        ".{destination_name}.openfrag-tmp-{}-{temporary_id}",
        std::process::id()
    ))
}

fn map_source_error(error: &io::Error) -> FileOperationError {
    if error.kind() == io::ErrorKind::NotFound {
        FileOperationError::SourceMissing
    } else {
        map_io_error(error)
    }
}

fn map_destination_error(error: &io::Error) -> FileOperationError {
    if error.kind() == io::ErrorKind::AlreadyExists {
        FileOperationError::DestinationExists
    } else {
        map_io_error(error)
    }
}

fn map_io_error(error: &io::Error) -> FileOperationError {
    FileOperationError::Io(error.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportError {
    NotReviewed,
    NameSpaceExhausted,
    File(FileOperationError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportReceipt {
    clip: Clip,
    destination_path: PathBuf,
    bytes: u64,
}

impl ExportReceipt {
    pub fn clip(&self) -> &Clip {
        &self.clip
    }

    pub fn destination_path(&self) -> &Path {
        &self.destination_path
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

pub fn sanitize_export_filename(title: &str, clip_id: &ClipId) -> String {
    format!(
        "{}-{}.mp4",
        sanitize_filename_component(title, "clip"),
        sanitize_filename_component(clip_id.as_str(), "id")
    )
}

pub fn export_clip(
    file_system: &mut impl LocalFileSystem,
    clip: &Clip,
    destination_directory: &Path,
) -> Result<ExportReceipt, ExportError> {
    if !matches!(clip.review_state, ReviewState::Reviewed { .. }) {
        return Err(ExportError::NotReviewed);
    }
    let artifact = clip
        .derivative
        .as_ref()
        .map_or(&clip.source_capture, DerivativeProvenance::artifact);
    let filename = sanitize_export_filename(&clip.title, &clip.id);
    let stem = filename.strip_suffix(".mp4").unwrap_or(&filename);
    let mut collision_index = 1_u32;
    loop {
        let candidate = if collision_index == 1 {
            filename.clone()
        } else {
            format!("{stem}-{collision_index}.mp4")
        };
        let destination_path = destination_directory.join(candidate);
        match file_system.atomic_copy(artifact.path(), &destination_path) {
            Ok(bytes) => {
                return Ok(ExportReceipt {
                    clip: clip.clone(),
                    destination_path,
                    bytes,
                });
            }
            Err(FileOperationError::DestinationExists) => {
                collision_index = collision_index
                    .checked_add(1)
                    .ok_or(ExportError::NameSpaceExhausted)?;
            }
            Err(error) => return Err(ExportError::File(error)),
        }
    }
}

fn sanitize_filename_component(value: &str, fallback: &str) -> String {
    let mut sanitized = String::new();
    let mut pending_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !sanitized.is_empty() {
                sanitized.push('-');
            }
            sanitized.push(character.to_ascii_lowercase());
            pending_separator = false;
        } else {
            pending_separator = true;
        }
    }
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}
