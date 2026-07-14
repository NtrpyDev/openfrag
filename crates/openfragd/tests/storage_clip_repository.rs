pub use openfragd::{api, service};

#[path = "../src/clip_ports.rs"]
pub mod clip_ports;
#[path = "../src/storage_clip_repository.rs"]
mod storage_clip_repository;

use clip_ports::{ClipPortError, ClipStore};
use openfrag_clips::{
    ArtifactProvenance, Clip, ClipOrigin, ClipRepository, DerivativeProvenance, RepositoryError,
    ReviewEdit, ReviewState, TrimRange, review_clip,
};
use openfrag_storage::{
    ClipExportStatus, ClipId as StorageClipId, DurableClipOriginInput, Layout, Storage,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use storage_clip_repository::{
    DurableReviewDecision, DurableReviewUpdate, StorageClipRepository, StorageClipRepositoryError,
};

fn stored_clip(storage: &Storage, request_id: &str, bytes: &[u8]) -> StorageClipId {
    let hash = storage
        .commit_artifact(storage.stage_artifact(bytes).unwrap(), "mkv", None)
        .unwrap();
    let session = storage.create_capture_session("76561198000000000").unwrap();
    let attempt = storage.request_save(&session, request_id, 1_000).unwrap();
    storage
        .complete_save(&attempt, &hash, 1_000, 31_000)
        .unwrap();
    storage.create_clip(&attempt, "raw_manual").unwrap()
}

fn complete_model(storage: &Storage, clip: &StorageClipId) {
    storage
        .complete_clip_model_metadata(
            clip.as_str(),
            30_000,
            DurableClipOriginInput::Manual {
                flag_receipt_id: "manual-flag-receipt",
                flag_time_ms: 12_000,
                overlapping_auto_receipts: &["overlapping-auto".into()],
            },
        )
        .unwrap();
}

fn update(decision: DurableReviewDecision, title: &str) -> DurableReviewUpdate {
    DurableReviewUpdate {
        title: title.into(),
        note: format!("{title} note"),
        tags: vec!["mirage".into(), "rifle".into()],
        favorite: true,
        decision,
    }
}

#[test]
fn complete_storage_record_rehydrates_every_represented_clip_field() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "load-request", b"local media");
    complete_model(&storage, &clip_id);
    let artifact = storage.clip_artifact_file(clip_id.as_str()).unwrap();
    let mut repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));

    let clip = ClipStore::load(&mut repository, clip_id.as_str()).unwrap();

    assert_eq!(clip.id().as_str(), clip_id.as_str());
    assert_eq!(clip.revision(), 0);
    assert_eq!(clip.source_capture().sha256(), artifact.sha256);
    assert_eq!(clip.source_capture().path(), artifact.path);
    assert_eq!(clip.source_capture().duration_ms(), 30_000);
    assert_eq!(
        clip.source_capture().bytes(),
        u64::try_from(artifact.byte_length).unwrap()
    );
    assert!(!clip.capture_session_id().is_empty());
    assert!(matches!(clip.review_state(), ReviewState::Provisional));
    assert!(matches!(
        clip.origin(),
        ClipOrigin::ManualFlag {
            flag_receipt_id,
            flag_time_ms: 12_000,
            overlapping_auto_receipts,
        } if flag_receipt_id == "manual-flag-receipt"
            && overlapping_auto_receipts.contains("overlapping-auto")
    ));
}

#[test]
fn incomplete_legacy_record_remains_typed_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "legacy-request", b"legacy media");
    let mut repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));

    assert!(matches!(
        repository.load_durable(clip_id.as_str()),
        Err(StorageClipRepositoryError::Unavailable(_))
    ));
    assert!(matches!(
        ClipStore::load(&mut repository, clip_id.as_str()),
        Err(ClipPortError::Unavailable(_))
    ));
}

#[test]
fn revisioned_keep_and_reject_survive_reopen_and_rehydrate() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let kept_id = stored_clip(&storage, "keep-request", b"kept media");
    let rejected_id = stored_clip(&storage, "reject-request", b"rejected media");
    complete_model(&storage, &kept_id);
    complete_model(&storage, &rejected_id);
    let shared = Arc::new(Mutex::new(storage));
    let repository = StorageClipRepository::new(Arc::clone(&shared));
    repository
        .persist_review(
            kept_id.as_str(),
            &update(DurableReviewDecision::Keep, "Kept clutch"),
        )
        .unwrap();
    repository
        .persist_review(
            rejected_id.as_str(),
            &update(DurableReviewDecision::Reject, "Rejected hold"),
        )
        .unwrap();
    drop(repository);
    drop(shared);

    let reopened = Arc::new(Mutex::new(Storage::open(Layout::at(root.path())).unwrap()));
    let repository = StorageClipRepository::new(reopened);
    let kept = repository.load_durable(kept_id.as_str()).unwrap();
    assert_eq!(kept.revision(), 1);
    assert_eq!(kept.title(), "Kept clutch");
    assert_eq!(
        kept.tags(),
        &BTreeSet::from(["mirage".into(), "rifle".into()])
    );
    assert!(matches!(kept.review_state(), ReviewState::Reviewed { .. }));
    let rejected = repository.load_durable(rejected_id.as_str()).unwrap();
    assert_eq!(rejected.revision(), 1);
    assert_eq!(rejected.title(), "Rejected hold");
    assert!(matches!(
        rejected.review_state(),
        ReviewState::Rejected { .. }
    ));
}

#[derive(Default)]
struct CaptureUpdated(Option<Clip>);

impl ClipRepository for CaptureUpdated {
    fn compare_and_swap(&mut self, _: u64, updated: &Clip) -> Result<(), RepositoryError> {
        self.0 = Some(updated.clone());
        Ok(())
    }
}

#[test]
fn durable_compare_and_swap_persists_metadata_and_rejects_stale_revision() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "cas-request", b"cas media");
    complete_model(&storage, &clip_id);
    let mut repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));
    let current = repository.load_durable(clip_id.as_str()).unwrap();
    let mut capture = CaptureUpdated::default();
    review_clip(
        &mut capture,
        &current,
        ReviewEdit {
            title: "Revisioned".into(),
            note: "CAS".into(),
            tags: BTreeSet::from(["reviewed".into()]),
            favorite: true,
            derivative: None,
        },
        20_000,
    )
    .unwrap();
    let updated = capture.0.unwrap();

    repository.compare_and_swap(0, &updated).unwrap();
    let error = repository.compare_and_swap(0, &updated).unwrap_err();

    assert!(error.message.contains("Conflict"));
    let durable = repository.load_durable(clip_id.as_str()).unwrap();
    assert_eq!(durable.revision(), 1);
    assert_eq!(durable.title(), "Revisioned");
}

#[test]
fn derivative_update_requires_the_new_clip_identity_boundary() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "trim-request", b"trim media");
    complete_model(&storage, &clip_id);
    let repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));
    let current = repository.load_durable(clip_id.as_str()).unwrap();
    let mut capture = CaptureUpdated::default();
    review_clip(
        &mut capture,
        &current,
        ReviewEdit {
            title: "Trimmed".into(),
            note: String::new(),
            tags: BTreeSet::new(),
            favorite: false,
            derivative: Some(DerivativeProvenance::new(
                ArtifactProvenance::new(
                    "derived-artifact",
                    PathBuf::from("/tmp/derived.mp4"),
                    "derived-sha",
                    10_000,
                    100,
                )
                .unwrap(),
                current.source_capture().artifact_id(),
                TrimRange::new(1_000, 11_000).unwrap(),
            )),
        },
        20_000,
    )
    .unwrap();

    assert_eq!(
        repository.compare_and_swap_durable(0, &capture.0.unwrap()),
        Err(StorageClipRepositoryError::DerivedClipIdentityRequired)
    );
    assert_eq!(
        repository
            .load_durable(clip_id.as_str())
            .unwrap()
            .revision(),
        0
    );
}

#[test]
fn represented_export_lifecycle_is_forwarded_without_state_invention() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "export-request", b"export media");
    let succeeded = storage.enqueue_export(&clip_id, "/tmp/export.mp4").unwrap();
    let failed = storage.enqueue_export(&clip_id, "/tmp/fail.mp4").unwrap();
    let repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));

    repository.begin_export(&succeeded).unwrap();
    repository
        .succeed_export(&succeeded, &"a".repeat(64), 123, "h264-aac")
        .unwrap();
    assert_eq!(
        repository.export(&succeeded).unwrap().status,
        ClipExportStatus::Succeeded
    );
    repository.fail_export(&failed, "disk_full").unwrap();
    let failed = repository.export(&failed).unwrap();
    assert_eq!(failed.status, ClipExportStatus::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("disk_full"));
}
