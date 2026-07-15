//! Local clip review, trim, and export mutation ports.
#![allow(clippy::missing_errors_doc)]

use crate::{
    api::{ApiError, Clip as ApiClip, ClipDecision, ClipPreview, ClipTrimRequest, ClipUpdate},
    service::MutationPorts,
    storage_clip_repository::StorageClipRepository,
};
use openfrag_clips::{
    CancellationToken, Clip, ClipRepository, DerivativeProfile, DerivativeReviewError,
    DerivativeReviewRequest, ExportError, FfmpegTranscoder, FfprobeMediaProbe, LocalFileSystem,
    MediaProbe, ReviewEdit, ReviewError, ReviewMetadata, StdLocalFileSystem, Transcoder, TrimRange,
    export_clip, prepare_derivative_and_review, reject_clip, review_clip,
};
use openfrag_storage::Storage;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Persistence seam used by the local mutation adapter.
pub trait ClipStore: ClipRepository {
    fn load(&mut self, clip_id: &str) -> Result<Clip, ClipPortError>;

    fn commit_derivative(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<Clip, ClipPortError> {
        self.compare_and_swap(expected_revision, updated)
            .map_err(|error| ClipPortError::Unavailable(error.message))?;
        Ok(updated.clone())
    }
}

/// Supplies timestamps while keeping review transitions deterministic in tests.
pub trait ClipClock {
    fn now_ms(&mut self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipPortError {
    NotFound,
    Invalid(String),
    Unavailable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipDirectories {
    capture: PathBuf,
    derivatives: PathBuf,
    exports: PathBuf,
}

impl ClipDirectories {
    pub fn new(
        capture: PathBuf,
        derivatives: PathBuf,
        exports: PathBuf,
    ) -> Result<Self, ClipPortError> {
        for (label, path) in [
            ("capture", &capture),
            ("derivative", &derivatives),
            ("export", &exports),
        ] {
            if !is_local_absolute(path) {
                return Err(ClipPortError::Invalid(format!(
                    "{label} directory must be an absolute local path without traversal"
                )));
            }
        }
        Ok(Self {
            capture,
            derivatives,
            exports,
        })
    }
}

pub struct ClipMutationDependencies<R, F, T, P, C> {
    repository: R,
    file_system: F,
    transcoder: T,
    probe: P,
    clock: C,
}

impl<R, F, T, P, C> ClipMutationDependencies<R, F, T, P, C> {
    #[must_use]
    pub fn new(repository: R, file_system: F, transcoder: T, probe: P, clock: C) -> Self {
        Self {
            repository,
            file_system,
            transcoder,
            probe,
            clock,
        }
    }
}

type DependenciesGuard<'a, R, F, T, P, C> =
    std::sync::MutexGuard<'a, ClipMutationDependencies<R, F, T, P, C>>;

/// `MutationPorts` adapter whose side effects are all injected and locally scoped.
pub struct ClipMutationPorts<R, F, T, P, C> {
    dependencies: Mutex<ClipMutationDependencies<R, F, T, P, C>>,
    directories: ClipDirectories,
    cancellation: CancellationToken,
}

impl<R, F, T, P, C> ClipMutationPorts<R, F, T, P, C> {
    #[must_use]
    pub fn new(
        dependencies: ClipMutationDependencies<R, F, T, P, C>,
        directories: ClipDirectories,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            dependencies: Mutex::new(dependencies),
            directories,
            cancellation,
        }
    }
}

impl<R, F, T, P, C> MutationPorts for ClipMutationPorts<R, F, T, P, C>
where
    R: ClipStore + Send + 'static,
    F: LocalFileSystem + Send + 'static,
    T: Transcoder + Send + 'static,
    P: MediaProbe + Send + 'static,
    C: ClipClock + Send + 'static,
{
    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<ApiClip, ApiError> {
        let mut dependencies = self.lock()?;
        let clip = dependencies.repository.load(id).map_err(api_error)?;
        ensure_capture_path(&clip, &self.directories.capture)?;
        let updated = match update.decision {
            ClipDecision::Keep => {
                let reviewed_at_ms = dependencies.clock.now_ms();
                let favorite = clip.favorite();
                let derivative = clip.derivative().cloned();
                review_clip(
                    &mut dependencies.repository,
                    &clip,
                    ReviewEdit {
                        title: update.title,
                        note: update.note,
                        tags: update.tags.into_iter().collect(),
                        favorite,
                        derivative,
                    },
                    reviewed_at_ms,
                )
                .map_err(review_error)?
            }
            ClipDecision::Reject => {
                let rejected_at_ms = dependencies.clock.now_ms();
                reject_clip(&mut dependencies.repository, &clip, rejected_at_ms)
                    .map_err(review_error)?
            }
        };
        Ok(api_clip(&updated))
    }

    fn preview_clip(&self, id: &str) -> Result<ClipPreview, ApiError> {
        let mut dependencies = self.lock()?;
        let clip = dependencies.repository.load(id).map_err(api_error)?;
        ensure_capture_path(&clip, &self.directories.capture)?;
        let artifact = clip
            .derivative()
            .map_or(clip.source_capture(), |derivative| derivative.artifact());
        if !artifact.path().starts_with(&self.directories.capture)
            && !artifact.path().starts_with(&self.directories.derivatives)
        {
            return Err(ApiError::Invalid(
                "clip preview is outside private local storage".into(),
            ));
        }
        Ok(ClipPreview {
            path: artifact.path().to_path_buf(),
            media_type: media_type(artifact.path()).into(),
        })
    }

    fn trim(&self, id: &str, request: ClipTrimRequest) -> Result<Value, ApiError> {
        let mut dependencies = self.lock()?;
        let clip = dependencies.repository.load(id).map_err(api_error)?;
        ensure_capture_path(&clip, &self.directories.capture)?;
        let trim = TrimRange::new(request.start_ms, request.end_ms)
            .map_err(|_| ApiError::Invalid("trim end must be greater than trim start".into()))?;
        if trim.end_ms() > clip.source_capture().duration_ms() {
            return Err(ApiError::Invalid("trim exceeds the source clip".into()));
        }
        let reviewed_at_ms = dependencies.clock.now_ms();
        let artifact_id = format!("{}-trim-r{}", clip.id().as_str(), clip.revision() + 1);
        let request = DerivativeReviewRequest {
            artifact_id,
            destination_directory: self.directories.derivatives.clone(),
            trim: Some(trim),
            profile: DerivativeProfile::Review,
            metadata: ReviewMetadata {
                title: clip.title().to_owned(),
                note: clip.note().to_owned(),
                tags: clip.tags().clone(),
                favorite: clip.favorite(),
            },
            reviewed_at_ms,
        };
        let mut prepared_repository = PreparedRepository;
        let prepared = {
            let ClipMutationDependencies {
                transcoder, probe, ..
            } = &mut *dependencies;
            prepare_derivative_and_review(
                &mut prepared_repository,
                transcoder,
                probe,
                &clip,
                request,
                &self.cancellation,
            )
            .map_err(derivative_error)?
        };
        let committed = dependencies
            .repository
            .commit_derivative(clip.revision(), &prepared)
            .inspect_err(|_| remove_derivative(&prepared))
            .map_err(api_error)?;
        let artifact = committed
            .derivative()
            .map_or(committed.source_capture(), |derivative| {
                derivative.artifact()
            });
        Ok(json!({
            "clip_id": committed.id().as_str(),
            "path": artifact.path(),
            "start_ms": trim.start_ms(),
            "end_ms": trim.end_ms(),
        }))
    }

    fn export(&self, id: &str) -> Result<Value, ApiError> {
        let mut dependencies = self.lock()?;
        let clip = dependencies.repository.load(id).map_err(api_error)?;
        ensure_capture_path(&clip, &self.directories.capture)?;
        let receipt = export_clip(
            &mut dependencies.file_system,
            &clip,
            &self.directories.exports,
        )
        .map_err(export_error)?;
        Ok(json!({
            "clip_id": receipt.clip().id().as_str(),
            "path": receipt.destination_path(),
            "bytes": receipt.bytes(),
        }))
    }
}

#[derive(Default)]
struct PreparedRepository;

impl ClipRepository for PreparedRepository {
    fn compare_and_swap(
        &mut self,
        _: u64,
        _: &Clip,
    ) -> Result<(), openfrag_clips::RepositoryError> {
        Ok(())
    }
}

fn remove_derivative(clip: &Clip) {
    if let Some(derivative) = clip.derivative() {
        let _ = fs::remove_file(derivative.artifact().path());
    }
}

impl<R, F, T, P, C> ClipMutationPorts<R, F, T, P, C> {
    fn lock(&self) -> Result<DependenciesGuard<'_, R, F, T, P, C>, ApiError> {
        self.dependencies
            .lock()
            .map_err(|_| ApiError::Unavailable("clip mutation lock is unavailable".into()))
    }
}

fn is_local_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn ensure_capture_path(clip: &Clip, capture_directory: &Path) -> Result<(), ApiError> {
    let path = clip.source_capture().path();
    if !is_local_absolute(path) || !path.starts_with(capture_directory) {
        return Err(ApiError::Invalid(
            "clip source is outside the local capture directory".into(),
        ));
    }
    Ok(())
}

fn media_type(path: &Path) -> &'static str {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some(extension) if extension.eq_ignore_ascii_case("mp4") => "video/mp4",
        Some(extension) if extension.eq_ignore_ascii_case("webm") => "video/webm",
        Some(extension) if extension.eq_ignore_ascii_case("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

pub struct SystemClipClock;

impl ClipClock for SystemClipClock {
    fn now_ms(&mut self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0)
    }
}

pub type ProductionClipPorts = ClipMutationPorts<
    StorageClipRepository,
    StdLocalFileSystem,
    FfmpegTranscoder,
    FfprobeMediaProbe,
    SystemClipClock,
>;

pub fn production_clip_ports(
    storage: Arc<Mutex<Storage>>,
    data_directory: &Path,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
) -> Result<ProductionClipPorts, ClipPortError> {
    let directories = ClipDirectories::new(
        data_directory.join("artifacts/sha256"),
        data_directory.join("staging/clip-derivatives"),
        data_directory.join("exports"),
    )?;
    for directory in [&directories.derivatives, &directories.exports] {
        fs::create_dir_all(directory).map_err(|error| {
            ClipPortError::Unavailable(format!("cannot create private clip directory: {error}"))
        })?;
        set_private_directory(directory)?;
    }
    Ok(ClipMutationPorts::new(
        ClipMutationDependencies::new(
            StorageClipRepository::new(storage),
            StdLocalFileSystem,
            FfmpegTranscoder::new(ffmpeg, Duration::from_mins(2)),
            FfprobeMediaProbe::new(ffprobe, Duration::from_secs(30)),
            SystemClipClock,
        ),
        directories,
        CancellationToken::new(),
    ))
}

fn set_private_directory(path: &Path) -> Result<(), ClipPortError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            ClipPortError::Unavailable(format!("cannot protect clip directory: {error}"))
        })?;
    }
    Ok(())
}

fn api_clip(clip: &Clip) -> ApiClip {
    ApiClip {
        id: clip.id().as_str().to_owned(),
        title: Some(clip.title().to_owned()),
        note: Some(clip.note().to_owned()),
        tags: clip.tags().iter().cloned().collect(),
    }
}

fn api_error(error: ClipPortError) -> ApiError {
    match error {
        ClipPortError::NotFound => ApiError::NotFound,
        ClipPortError::Invalid(message) => ApiError::Invalid(message),
        ClipPortError::Unavailable(message) => ApiError::Unavailable(message),
    }
}

fn review_error(error: ReviewError) -> ApiError {
    match error {
        ReviewError::TrimOutsideSource | ReviewError::DerivativeHasWrongSource => {
            ApiError::Invalid(format!("invalid clip review: {error:?}"))
        }
        ReviewError::InvalidTransition => ApiError::Invalid("clip is already rejected".into()),
        ReviewError::Repository(error) => ApiError::Unavailable(error.message),
    }
}

fn derivative_error(error: DerivativeReviewError) -> ApiError {
    match error {
        DerivativeReviewError::TrimOutsideSource
        | DerivativeReviewError::Model(_)
        | DerivativeReviewError::Review(
            ReviewError::TrimOutsideSource
            | ReviewError::DerivativeHasWrongSource
            | ReviewError::InvalidTransition,
        ) => ApiError::Invalid(format!("invalid clip trim: {error:?}")),
        other => ApiError::Unavailable(format!("local clip trim failed: {other:?}")),
    }
}

fn export_error(error: ExportError) -> ApiError {
    match error {
        ExportError::NotReviewed => ApiError::Invalid("review the clip before exporting".into()),
        other => ApiError::Unavailable(format!("local clip export failed: {other:?}")),
    }
}
