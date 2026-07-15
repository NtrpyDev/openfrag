use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use openfrag_clips::{
    ArtifactProvenance, CancellationToken, Clip, ClipId, ClipOrigin, ClipRepository,
    FileOperationError, LocalFileSystem, MediaInfo, MediaProbe, MediaProbeError, RepositoryError,
    TranscodeError, TranscodeRequest, Transcoder, TrimRange,
};
use openfrag_storage::{DurableClipOriginInput, Layout, Storage};
use openfragd::clip_ports::{
    ClipClock, ClipDirectories, ClipMutationDependencies, ClipMutationPorts, ClipPortError,
    ClipStore,
};
use openfragd::{
    AppConfig,
    api::{ClipDecision, ClipTrimRequest, ClipUpdate},
    app,
    service::MutationPorts,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Clone)]
struct MemoryRepository(Arc<Mutex<Clip>>);

impl ClipStore for MemoryRepository {
    fn load(&mut self, clip_id: &str) -> Result<Clip, ClipPortError> {
        let clip = self.0.lock().unwrap().clone();
        (clip.id().as_str() == clip_id)
            .then_some(clip)
            .ok_or(ClipPortError::NotFound)
    }
}

impl ClipRepository for MemoryRepository {
    fn compare_and_swap(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError> {
        let mut current = self.0.lock().unwrap();
        if current.revision() != expected_revision {
            return Err(RepositoryError::new("stale revision"));
        }
        *current = updated.clone();
        Ok(())
    }
}

#[derive(Default)]
struct MemoryFileSystem {
    existing: BTreeSet<PathBuf>,
    attempts: Arc<Mutex<Vec<(PathBuf, PathBuf)>>>,
    failure: Option<FileOperationError>,
}

impl LocalFileSystem for MemoryFileSystem {
    fn atomic_copy(
        &mut self,
        source: &Path,
        destination: &Path,
    ) -> Result<u64, FileOperationError> {
        self.attempts
            .lock()
            .unwrap()
            .push((source.to_path_buf(), destination.to_path_buf()));
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if !self.existing.insert(destination.to_path_buf()) {
            return Err(FileOperationError::DestinationExists);
        }
        Ok(321)
    }
}

struct FixedClock(u64);

impl ClipClock for FixedClock {
    fn now_ms(&mut self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandCall {
    source: PathBuf,
    destination: PathBuf,
    start_ms: u64,
    end_ms: u64,
}

struct FakeTranscoder {
    calls: Arc<Mutex<Vec<CommandCall>>>,
    failure_after_write: bool,
}

impl Transcoder for FakeTranscoder {
    fn transcode(
        &mut self,
        request: &TranscodeRequest<'_>,
        _: &CancellationToken,
    ) -> Result<(), TranscodeError> {
        self.calls.lock().unwrap().push(CommandCall {
            source: request.source().to_path_buf(),
            destination: request.destination().to_path_buf(),
            start_ms: request.trim().start_ms(),
            end_ms: request.trim().end_ms(),
        });
        std::fs::write(request.destination(), b"trimmed media").unwrap();
        if self.failure_after_write {
            Err(TranscodeError::Failed {
                status: Some(1),
                stderr: "injected ffmpeg failure".into(),
                stderr_truncated: false,
            })
        } else {
            Ok(())
        }
    }
}

struct FakeProbe;

impl MediaProbe for FakeProbe {
    fn probe(&mut self, _: &Path, _: &CancellationToken) -> Result<MediaInfo, MediaProbeError> {
        MediaInfo::new(30_000, true)
    }
}

fn provisional(source: PathBuf) -> Clip {
    Clip::provisional(
        ClipId::new("clip-42").unwrap(),
        ArtifactProvenance::new("source-42", source, "source-hash", 42_000, 999).unwrap(),
        "session-1",
        ClipOrigin::manual_flag("flag-1", 1_000, BTreeSet::new()).unwrap(),
        "Mirage clutch",
    )
    .unwrap()
}

fn directories(root: &Path) -> ClipDirectories {
    ClipDirectories::new(
        root.join("captures"),
        root.join("derivatives"),
        root.join("exports"),
    )
    .unwrap()
}

fn update(decision: ClipDecision) -> ClipUpdate {
    ClipUpdate {
        title: "My clutch".into(),
        note: "B hold".into(),
        tags: vec!["mirage".into(), "clutch".into()],
        decision,
    }
}

#[test]
fn keep_and_reject_use_evidence_preserving_review_transitions() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("captures")).unwrap();
    let clip = provisional(root.path().join("captures/source.mp4"));
    let state = Arc::new(Mutex::new(clip));
    let ports = ClipMutationPorts::new(
        ClipMutationDependencies::new(
            MemoryRepository(Arc::clone(&state)),
            MemoryFileSystem::default(),
            FakeTranscoder {
                calls: Arc::default(),
                failure_after_write: false,
            },
            FakeProbe,
            FixedClock(50_000),
        ),
        directories(root.path()),
        CancellationToken::new(),
    );

    let kept = ports
        .update_clip("clip-42", update(ClipDecision::Keep))
        .unwrap();
    assert_eq!(kept.title.as_deref(), Some("My clutch"));
    assert_eq!(kept.note.as_deref(), Some("B hold"));
    assert_eq!(kept.tags, vec!["clutch", "mirage"]);
    let source = state.lock().unwrap().source_capture().clone();

    let rejected = ports
        .update_clip("clip-42", update(ClipDecision::Reject))
        .unwrap();
    assert_eq!(rejected.id, "clip-42");
    let current = state.lock().unwrap();
    assert_eq!(current.source_capture(), &source);
    assert!(matches!(
        current.review_state(),
        openfrag_clips::ReviewState::Rejected {
            rejected_at_ms: 50_000
        }
    ));
}

#[test]
fn trim_passes_a_bounded_direct_request_and_commits_the_derivative() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("captures")).unwrap();
    let source = root.path().join("captures/source.mp4");
    std::fs::write(&source, b"immutable source").unwrap();
    let state = Arc::new(Mutex::new(provisional(source.clone())));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let ports = ClipMutationPorts::new(
        ClipMutationDependencies::new(
            MemoryRepository(Arc::clone(&state)),
            MemoryFileSystem::default(),
            FakeTranscoder {
                calls: Arc::clone(&calls),
                failure_after_write: false,
            },
            FakeProbe,
            FixedClock(60_000),
        ),
        directories(root.path()),
        CancellationToken::new(),
    );

    let receipt = ports
        .trim(
            "clip-42",
            ClipTrimRequest {
                start_ms: 5_000,
                end_ms: 35_000,
            },
        )
        .unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source, source);
    assert_eq!((calls[0].start_ms, calls[0].end_ms), (5_000, 35_000));
    let current = state.lock().unwrap();
    let derivative = current.derivative().unwrap();
    assert_eq!(derivative.trim(), TrimRange::new(5_000, 35_000).unwrap());
    assert_eq!(receipt["start_ms"], 5_000);
    assert_eq!(
        std::fs::read(current.source_capture().path()).unwrap(),
        b"immutable source"
    );
}

#[test]
fn trim_rejects_a_range_past_the_source_duration_before_transcoding() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("captures")).unwrap();
    let state = Arc::new(Mutex::new(provisional(
        root.path().join("captures/source.mp4"),
    )));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let ports = ClipMutationPorts::new(
        ClipMutationDependencies::new(
            MemoryRepository(Arc::clone(&state)),
            MemoryFileSystem::default(),
            FakeTranscoder {
                calls: Arc::clone(&calls),
                failure_after_write: false,
            },
            FakeProbe,
            FixedClock(65_000),
        ),
        directories(root.path()),
        CancellationToken::new(),
    );

    let error = ports
        .trim(
            "clip-42",
            ClipTrimRequest {
                start_ms: 5_000,
                end_ms: 42_001,
            },
        )
        .unwrap_err();

    assert_eq!(
        error,
        openfragd::api::ApiError::Invalid("trim exceeds the source clip".into())
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn export_uses_the_deterministic_local_name_and_collision_fallback() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("captures")).unwrap();
    let state = Arc::new(Mutex::new(provisional(
        root.path().join("captures/source.mp4"),
    )));
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let first = root.path().join("exports/my-clutch-clip-42.mp4");
    let file_system = MemoryFileSystem {
        existing: BTreeSet::from([first.clone()]),
        attempts: Arc::clone(&attempts),
        failure: None,
    };
    let ports = ClipMutationPorts::new(
        ClipMutationDependencies::new(
            MemoryRepository(Arc::clone(&state)),
            file_system,
            FakeTranscoder {
                calls: Arc::default(),
                failure_after_write: false,
            },
            FakeProbe,
            FixedClock(70_000),
        ),
        directories(root.path()),
        CancellationToken::new(),
    );
    ports
        .update_clip("clip-42", update(ClipDecision::Keep))
        .unwrap();

    let receipt = ports.export("clip-42").unwrap();

    assert_eq!(
        receipt["path"],
        root.path()
            .join("exports/my-clutch-clip-42-2.mp4")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(attempts.lock().unwrap().len(), 2);
}

#[test]
fn failed_trim_removes_staging_output_and_leaves_repository_unchanged() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("captures")).unwrap();
    let original = provisional(root.path().join("captures/source.mp4"));
    let state = Arc::new(Mutex::new(original.clone()));
    let ports = ClipMutationPorts::new(
        ClipMutationDependencies::new(
            MemoryRepository(Arc::clone(&state)),
            MemoryFileSystem::default(),
            FakeTranscoder {
                calls: Arc::default(),
                failure_after_write: true,
            },
            FakeProbe,
            FixedClock(80_000),
        ),
        directories(root.path()),
        CancellationToken::new(),
    );

    assert!(
        ports
            .trim(
                "clip-42",
                ClipTrimRequest {
                    start_ms: 5_000,
                    end_ms: 35_000,
                },
            )
            .is_err()
    );

    assert_eq!(*state.lock().unwrap(), original);
    let derivative_directory = root.path().join("derivatives");
    assert_eq!(std::fs::read_dir(derivative_directory).unwrap().count(), 0);
    assert!(matches!(
        ClipPortError::Unavailable("offline".into()),
        ClipPortError::Unavailable(message) if message == "offline"
    ));
}

#[tokio::test]
async fn shipped_clip_ports_review_preview_trim_and_export_durable_media() {
    let root = tempfile::tempdir().unwrap();
    let data_directory = root.path().join("data");
    let clip_id = seed_durable_clip(&data_directory);
    let (ffmpeg, ffprobe) = fake_clip_tools(root.path());
    let router = app(&AppConfig::new(&data_directory).with_clip_tools(ffmpeg, ffprobe)).unwrap();

    let preview = router
        .clone()
        .oneshot(
            Request::get(format!("/api/clips/{clip_id}/preview"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(preview.into_body(), 1024).await.unwrap(),
        "immutable source media"
    );
    let preview_range = router
        .clone()
        .oneshot(
            Request::get(format!("/api/clips/{clip_id}/preview"))
                .header("range", "bytes=10-15")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview_range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(preview_range.headers()["content-range"], "bytes 10-15/22");
    assert_eq!(
        to_bytes(preview_range.into_body(), 1024).await.unwrap(),
        &b"immutable source media"[10..=15]
    );

    let reviewed = router
        .clone()
        .oneshot(
            Request::patch(format!("/api/clips/{clip_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"title":"My clutch","note":"B hold","tags":["mirage"],"decision":"keep"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reviewed.status(), StatusCode::OK);

    let trimmed = router
        .clone()
        .oneshot(
            Request::post(format!("/api/clips/{clip_id}/trim"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"start_ms":5000,"end_ms":15000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let trimmed_status = trimmed.status();
    let trimmed_body = to_bytes(trimmed.into_body(), 16 * 1024).await.unwrap();
    assert_eq!(
        trimmed_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&trimmed_body)
    );
    let trimmed: serde_json::Value = serde_json::from_slice(&trimmed_body).unwrap();
    let derived_id = trimmed["clip_id"].as_str().unwrap();
    assert_ne!(derived_id, clip_id);
    assert_eq!(
        Storage::open(Layout::at(&data_directory))
            .unwrap()
            .list_clips()
            .unwrap()
            .len(),
        2
    );

    let export = router
        .oneshot(
            Request::post(format!("/api/clips/{derived_id}/export"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let export: serde_json::Value =
        serde_json::from_slice(&to_bytes(export.into_body(), 16 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        std::fs::read(export["path"].as_str().unwrap()).unwrap(),
        b"trimmed media"
    );
}

#[tokio::test]
async fn shipped_clip_ports_clean_partial_output_after_failed_transcode() {
    let root = tempfile::tempdir().unwrap();
    let data_directory = root.path().join("data");
    let clip_id = seed_durable_clip(&data_directory);
    let (ffmpeg, ffprobe) = fake_clip_tools(root.path());
    std::fs::write(
        &ffmpeg,
        "#!/bin/sh\nfor output do :; done\nprintf 'partial media' > \"$output\"\nexit 1\n",
    )
    .unwrap();
    let router = app(&AppConfig::new(&data_directory).with_clip_tools(ffmpeg, ffprobe)).unwrap();

    let response = router
        .oneshot(
            Request::post(format!("/api/clips/{clip_id}/trim"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"start_ms":5000,"end_ms":15000}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        std::fs::read_dir(data_directory.join("staging/clip-derivatives"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        Storage::open(Layout::at(&data_directory))
            .unwrap()
            .list_clips()
            .unwrap()
            .len(),
        1
    );
}

fn seed_durable_clip(data_directory: &Path) -> String {
    let storage = Storage::open(Layout::at(data_directory)).unwrap();
    let artifact = storage
        .commit_artifact(
            storage.stage_artifact(b"immutable source media").unwrap(),
            "mkv",
            Some("video/x-matroska"),
        )
        .unwrap();
    let session = storage.create_capture_session("76561198000000000").unwrap();
    let attempt = storage
        .request_save(&session, "production-clip", 1_000)
        .unwrap();
    storage
        .complete_save(&attempt, &artifact, 1_000, 31_000)
        .unwrap();
    let clip = storage.create_clip(&attempt, "raw_manual").unwrap();
    storage
        .complete_clip_model_metadata(
            clip.as_str(),
            30_000,
            DurableClipOriginInput::Manual {
                flag_receipt_id: "manual-flag",
                flag_time_ms: 31_000,
                overlapping_auto_receipts: &[],
            },
        )
        .unwrap();
    clip.as_str().to_owned()
}

fn fake_clip_tools(root: &Path) -> (PathBuf, PathBuf) {
    let tools = root.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let ffmpeg = tools.join("ffmpeg");
    let ffprobe = tools.join("ffprobe");
    std::fs::write(
        &ffmpeg,
        "#!/bin/sh\nfor output do :; done\nprintf 'trimmed media' > \"$output\"\n",
    )
    .unwrap();
    std::fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"format\":{\"duration\":\"10.000\"},\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1280,\"height\":720,\"avg_frame_rate\":\"60/1\"}]}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    for executable in [&ffmpeg, &ffprobe] {
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    (ffmpeg, ffprobe)
}
