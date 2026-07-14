pub use openfragd::{api, service};

#[path = "../src/clip_ports.rs"]
mod clip_ports;

use clip_ports::{
    ClipClock, ClipDirectories, ClipMutationPorts, ClipPortError, ClipStore, TrimSelection,
};
use openfrag_clips::{
    ArtifactProvenance, CancellationToken, Clip, ClipId, ClipOrigin, ClipRepository,
    FileOperationError, LocalFileSystem, MediaInfo, MediaProbe, MediaProbeError, RepositoryError,
    TranscodeError, TranscodeRequest, Transcoder, TrimRange,
};
use openfragd::{
    api::{ClipDecision, ClipUpdate},
    service::MutationPorts,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

struct SelectedTrim(TrimRange);

impl TrimSelection for SelectedTrim {
    fn selected_range(&mut self, _: &Clip) -> Result<TrimRange, ClipPortError> {
        Ok(self.0)
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
        MemoryRepository(Arc::clone(&state)),
        MemoryFileSystem::default(),
        FakeTranscoder {
            calls: Arc::default(),
            failure_after_write: false,
        },
        FakeProbe,
        SelectedTrim(TrimRange::new(5_000, 35_000).unwrap()),
        FixedClock(50_000),
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
        MemoryRepository(Arc::clone(&state)),
        MemoryFileSystem::default(),
        FakeTranscoder {
            calls: Arc::clone(&calls),
            failure_after_write: false,
        },
        FakeProbe,
        SelectedTrim(TrimRange::new(5_000, 35_000).unwrap()),
        FixedClock(60_000),
        directories(root.path()),
        CancellationToken::new(),
    );

    let receipt = ports.trim("clip-42").unwrap();

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
        MemoryRepository(Arc::clone(&state)),
        file_system,
        FakeTranscoder {
            calls: Arc::default(),
            failure_after_write: false,
        },
        FakeProbe,
        SelectedTrim(TrimRange::new(5_000, 35_000).unwrap()),
        FixedClock(70_000),
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
        MemoryRepository(Arc::clone(&state)),
        MemoryFileSystem::default(),
        FakeTranscoder {
            calls: Arc::default(),
            failure_after_write: true,
        },
        FakeProbe,
        SelectedTrim(TrimRange::new(5_000, 35_000).unwrap()),
        FixedClock(80_000),
        directories(root.path()),
        CancellationToken::new(),
    );

    assert!(ports.trim("clip-42").is_err());

    assert_eq!(*state.lock().unwrap(), original);
    let derivative_directory = root.path().join("derivatives");
    assert_eq!(std::fs::read_dir(derivative_directory).unwrap().count(), 0);
}
