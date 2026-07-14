use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openfrag_clips::{
    ArtifactProvenance, Clip, ClipId, ClipOrigin, ClipRepository, DerivativeProvenance,
    ExportError, FileOperationError, LocalFileSystem, RepositoryError, ReviewEdit, ReviewError,
    ReviewState, StdLocalFileSystem, TrimRange, export_clip, reject_clip, review_clip,
    sanitize_export_filename,
};

#[derive(Default)]
struct MemoryRepository {
    committed: Vec<Clip>,
    fail: bool,
}

fn provisional_manual_clip() -> Clip {
    Clip::provisional(
        ClipId::new("manual-9").unwrap(),
        source(),
        "capture-session-7",
        ClipOrigin::manual_flag(
            "manual-flag-receipt",
            30_000,
            BTreeSet::from(["overlapping-auto-receipt".into()]),
        )
        .unwrap(),
        "Manual flag",
    )
    .unwrap()
}

impl ClipRepository for MemoryRepository {
    fn compare_and_swap(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError> {
        if self.fail {
            Err(RepositoryError::new("injected failure"))
        } else {
            assert_eq!(expected_revision, 0);
            self.committed.push(updated.clone());
            Ok(())
        }
    }
}

#[derive(Default)]
struct MemoryFileSystem {
    existing: BTreeSet<PathBuf>,
    attempts: Vec<(PathBuf, PathBuf)>,
    failure: Option<FileOperationError>,
}

impl LocalFileSystem for MemoryFileSystem {
    fn atomic_copy(
        &mut self,
        source: &Path,
        destination: &Path,
    ) -> Result<u64, FileOperationError> {
        self.attempts
            .push((source.to_path_buf(), destination.to_path_buf()));
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        if !self.existing.insert(destination.to_path_buf()) {
            return Err(FileOperationError::DestinationExists);
        }
        Ok(5_000_000)
    }
}

fn source() -> ArtifactProvenance {
    ArtifactProvenance::new(
        "capture-artifact",
        PathBuf::from("/captures/source.mp4"),
        "source-sha256",
        42_000,
        8_000_000,
    )
    .unwrap()
}

fn provisional_auto_clip() -> Clip {
    Clip::provisional(
        ClipId::new("clip-42").unwrap(),
        source(),
        "capture-session-7",
        ClipOrigin::auto_highlight(
            BTreeSet::from(["trigger-receipt".into()]),
            BTreeSet::from(["provisional-evidence".into()]),
        )
        .unwrap(),
        "Ace on Mirage",
    )
    .unwrap()
}

#[test]
fn review_saves_metadata_and_a_bounded_derivative_without_mutating_source() {
    let clip = provisional_auto_clip();
    let original_source = clip.source_capture().clone();
    let derivative = DerivativeProvenance::new(
        ArtifactProvenance::new(
            "trimmed-artifact",
            PathBuf::from("/captures/trimmed.mp4"),
            "trimmed-sha256",
            30_000,
            5_000_000,
        )
        .unwrap(),
        "capture-artifact",
        TrimRange::new(5_000, 35_000).unwrap(),
    );
    let mut repository = MemoryRepository::default();
    let reviewed = review_clip(
        &mut repository,
        &clip,
        ReviewEdit {
            title: "My clutch".into(),
            note: "Hold B".into(),
            tags: BTreeSet::from(["clutch".into(), "mirage".into()]),
            favorite: true,
            derivative: Some(derivative),
        },
        99_000,
    )
    .unwrap();

    assert_eq!(reviewed.source_capture(), &original_source);
    assert_eq!(clip.review_state(), &ReviewState::Provisional);
    assert!(matches!(
        reviewed.review_state(),
        ReviewState::Reviewed {
            reviewed_at_ms: 99_000
        }
    ));
    assert_eq!(reviewed.title(), "My clutch");
    assert_eq!(reviewed.note(), "Hold B");
    assert_eq!(
        reviewed.tags(),
        &BTreeSet::from(["clutch".into(), "mirage".into()])
    );
    assert!(reviewed.favorite());
    assert_eq!(repository.committed, vec![reviewed]);
}

#[test]
fn invalid_trim_or_repository_failure_leaves_the_public_clip_unchanged() {
    let clip = provisional_auto_clip();
    let invalid = DerivativeProvenance::new(
        ArtifactProvenance::new(
            "bad",
            PathBuf::from("/captures/bad.mp4"),
            "bad-sha256",
            10_000,
            1,
        )
        .unwrap(),
        "capture-artifact",
        TrimRange::new(10_000, 50_000).unwrap(),
    );
    let edit = ReviewEdit {
        title: "Title".into(),
        note: String::new(),
        tags: BTreeSet::new(),
        favorite: false,
        derivative: Some(invalid),
    };
    let mut repository = MemoryRepository::default();
    assert_eq!(
        review_clip(&mut repository, &clip, edit.clone(), 1),
        Err(ReviewError::TrimOutsideSource)
    );
    assert!(repository.committed.is_empty());
    repository.fail = true;
    let metadata_only = ReviewEdit {
        derivative: None,
        ..edit
    };
    assert!(matches!(
        review_clip(&mut repository, &clip, metadata_only, 1),
        Err(ReviewError::Repository(_))
    ));
    assert_eq!(clip.review_state(), &ReviewState::Provisional);
    assert_eq!(clip.source_capture(), &source());
}

#[test]
fn rejection_retains_manual_flag_and_source_provenance() {
    let clip = provisional_manual_clip();
    let original_source = clip.source_capture().clone();
    let original_origin = clip.origin().clone();
    let mut repository = MemoryRepository::default();

    let rejected = reject_clip(&mut repository, &clip, 123_000).unwrap();

    assert_eq!(clip.review_state(), &ReviewState::Provisional);
    assert_eq!(rejected.source_capture(), &original_source);
    assert_eq!(rejected.origin(), &original_origin);
    assert!(matches!(
        rejected.review_state(),
        ReviewState::Rejected {
            rejected_at_ms: 123_000
        }
    ));
    assert_eq!(repository.committed, vec![rejected]);
}

#[test]
fn failed_rejection_does_not_change_the_public_clip() {
    let clip = provisional_manual_clip();
    let mut repository = MemoryRepository {
        committed: Vec::new(),
        fail: true,
    };

    assert!(matches!(
        reject_clip(&mut repository, &clip, 123_000),
        Err(ReviewError::Repository(_))
    ));
    assert_eq!(clip.review_state(), &ReviewState::Provisional);
    assert_eq!(clip.source_capture(), &source());
    assert!(repository.committed.is_empty());
}

#[test]
fn export_uses_a_deterministic_sanitized_name_and_resolves_collisions() {
    let clip = provisional_auto_clip();
    let derivative = DerivativeProvenance::new(
        ArtifactProvenance::new(
            "trimmed-artifact",
            PathBuf::from("/captures/trimmed.mp4"),
            "trimmed-sha256",
            30_000,
            5_000_000,
        )
        .unwrap(),
        "capture-artifact",
        TrimRange::new(5_000, 35_000).unwrap(),
    );
    let mut repository = MemoryRepository::default();
    let reviewed = review_clip(
        &mut repository,
        &clip,
        ReviewEdit {
            title: "My clutch!! / Mirage 🔥".into(),
            note: "Evidence retained".into(),
            tags: BTreeSet::from(["mirage".into()]),
            favorite: true,
            derivative: Some(derivative),
        },
        99_000,
    )
    .unwrap();
    assert_eq!(
        sanitize_export_filename(reviewed.title(), reviewed.id()),
        "my-clutch-mirage-clip-42.mp4"
    );
    let export_directory = PathBuf::from("/exports");
    let first_destination = export_directory.join("my-clutch-mirage-clip-42.mp4");
    let mut file_system = MemoryFileSystem {
        existing: BTreeSet::from([first_destination.clone()]),
        ..MemoryFileSystem::default()
    };

    let receipt = export_clip(&mut file_system, &reviewed, &export_directory).unwrap();

    assert_eq!(
        receipt.destination_path(),
        Path::new("/exports/my-clutch-mirage-clip-42-2.mp4")
    );
    assert_eq!(receipt.bytes(), 5_000_000);
    assert_eq!(receipt.clip(), &reviewed);
    assert_eq!(receipt.clip().source_capture(), &source());
    assert_eq!(receipt.clip().origin(), clip.origin());
    assert_eq!(
        file_system.attempts,
        vec![
            (PathBuf::from("/captures/trimmed.mp4"), first_destination),
            (
                PathBuf::from("/captures/trimmed.mp4"),
                PathBuf::from("/exports/my-clutch-mirage-clip-42-2.mp4")
            )
        ]
    );
}

#[test]
fn failed_or_premature_export_leaves_clip_state_unchanged() {
    let provisional = provisional_manual_clip();
    let mut file_system = MemoryFileSystem::default();
    assert_eq!(
        export_clip(&mut file_system, &provisional, Path::new("/exports")),
        Err(ExportError::NotReviewed)
    );
    assert!(file_system.attempts.is_empty());

    let mut repository = MemoryRepository::default();
    let reviewed = review_clip(
        &mut repository,
        &provisional,
        ReviewEdit {
            title: "Manual".into(),
            note: String::new(),
            tags: BTreeSet::new(),
            favorite: false,
            derivative: None,
        },
        99_000,
    )
    .unwrap();
    let before = reviewed.clone();
    file_system.failure = Some(FileOperationError::Io("disk full".into()));
    assert_eq!(
        export_clip(&mut file_system, &reviewed, Path::new("/exports")),
        Err(ExportError::File(FileOperationError::Io(
            "disk full".into()
        )))
    );
    assert_eq!(reviewed, before);
    assert_eq!(reviewed.source_capture(), &source());
}

#[test]
fn standard_file_system_atomically_copies_without_overwriting_a_collision() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("openfrag-clips-{}-{unique}", std::process::id()));
    let export_directory = root.join("exports");
    std::fs::create_dir_all(&export_directory).unwrap();
    let source_path = root.join("source.mp4");
    std::fs::write(&source_path, b"source clip bytes").unwrap();
    let clip = Clip::provisional(
        ClipId::new("clip-7").unwrap(),
        ArtifactProvenance::new("source-7", source_path.clone(), "sha-7", 10_000, 17).unwrap(),
        "session-7",
        ClipOrigin::manual_flag("manual-7", 5_000, BTreeSet::new()).unwrap(),
        "Round win",
    )
    .unwrap();
    let mut repository = MemoryRepository::default();
    let reviewed = review_clip(
        &mut repository,
        &clip,
        ReviewEdit {
            title: "Round win".into(),
            note: String::new(),
            tags: BTreeSet::new(),
            favorite: false,
            derivative: None,
        },
        20_000,
    )
    .unwrap();
    let occupied = export_directory.join("round-win-clip-7.mp4");
    std::fs::write(&occupied, b"keep me").unwrap();

    let receipt = export_clip(&mut StdLocalFileSystem, &reviewed, &export_directory).unwrap();

    assert_eq!(std::fs::read(&source_path).unwrap(), b"source clip bytes");
    assert_eq!(std::fs::read(&occupied).unwrap(), b"keep me");
    assert_eq!(
        std::fs::read(receipt.destination_path()).unwrap(),
        b"source clip bytes"
    );
    assert_eq!(
        receipt.destination_path(),
        export_directory.join("round-win-clip-7-2.mp4")
    );
    std::fs::remove_dir_all(root).unwrap();
}
