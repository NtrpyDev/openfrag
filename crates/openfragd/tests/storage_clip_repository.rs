pub use openfragd::{api, service};

#[path = "../src/clip_ports.rs"]
pub mod clip_ports;
#[path = "../src/storage_clip_repository.rs"]
mod storage_clip_repository;

use clip_ports::{ClipPortError, ClipStore};
use openfrag_clips::ClipRepository;
use openfrag_storage::{ClipReviewDecision, Layout, Storage};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use storage_clip_repository::{
    DurableReviewDecision, DurableReviewUpdate, MissingDurableClipFields, StorageClipRepository,
    StorageClipRepositoryError,
};

fn stored_clip(storage: &Storage, request_id: &str, bytes: &[u8]) -> String {
    let hash = storage
        .commit_artifact(storage.stage_artifact(bytes).unwrap(), "mkv", None)
        .unwrap();
    let session = storage.create_capture_session("76561198000000000").unwrap();
    let attempt = storage.request_save(&session, request_id, 1_000).unwrap();
    storage
        .complete_save(&attempt, &hash, 1_000, 31_000)
        .unwrap();
    storage
        .create_clip(&attempt, "raw_manual")
        .unwrap()
        .as_str()
        .to_owned()
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
fn keep_and_reject_review_projections_survive_database_reopen() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let kept_id = stored_clip(&storage, "keep-request", b"kept media");
    let rejected_id = stored_clip(&storage, "reject-request", b"rejected media");
    let shared = Arc::new(Mutex::new(storage));
    let repository = StorageClipRepository::new(Arc::clone(&shared));

    let kept = repository
        .persist_review(
            &kept_id,
            &update(DurableReviewDecision::Keep, "Kept clutch"),
        )
        .unwrap();
    let rejected = repository
        .persist_review(
            &rejected_id,
            &update(DurableReviewDecision::Reject, "Rejected hold"),
        )
        .unwrap();
    assert_eq!(kept.review_decision, ClipReviewDecision::Keep);
    assert_eq!(rejected.review_decision, ClipReviewDecision::Reject);
    drop(repository);
    drop(shared);

    let reopened = Storage::open(Layout::at(root.path())).unwrap();
    let kept = reopened.clip_detail(&kept_id).unwrap();
    assert_eq!(kept.summary.title.as_deref(), Some("Kept clutch"));
    assert_eq!(kept.note.as_deref(), Some("Kept clutch note"));
    assert_eq!(kept.tags, ["mirage", "rifle"]);
    assert_eq!(kept.review_decision, ClipReviewDecision::Keep);
    assert!(kept.favorite);
    let rejected = reopened.clip_detail(&rejected_id).unwrap();
    assert_eq!(rejected.summary.title.as_deref(), Some("Rejected hold"));
    assert_eq!(rejected.note.as_deref(), Some("Rejected hold note"));
    assert_eq!(rejected.tags, ["mirage", "rifle"]);
    assert_eq!(rejected.review_decision, ClipReviewDecision::Reject);
    assert!(rejected.favorite);
}

#[test]
fn model_load_reports_all_missing_durable_fields_without_fabrication() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "load-request", b"local media");
    let mut repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));

    assert_eq!(
        repository.load_durable(&clip_id),
        Err(StorageClipRepositoryError::Unavailable(
            MissingDurableClipFields {
                duration: true,
                evidence: true,
                revision: true,
            }
        ))
    );
    assert!(matches!(
        ClipStore::load(&mut repository, &clip_id),
        Err(ClipPortError::Unavailable(message))
            if message.contains("duration=true")
                && message.contains("evidence=true")
                && message.contains("revision=true")
    ));
}

#[test]
fn compare_and_swap_refuses_to_invent_a_durable_revision() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(Layout::at(root.path())).unwrap();
    let clip_id = stored_clip(&storage, "cas-request", b"local media");
    let artifact = storage.clip_artifact_file(&clip_id).unwrap();
    let clip = openfrag_clips::Clip::provisional(
        openfrag_clips::ClipId::new(clip_id).unwrap(),
        openfrag_clips::ArtifactProvenance::new(
            "test-artifact",
            artifact.path,
            artifact.sha256,
            30_000,
            u64::try_from(artifact.byte_length).unwrap(),
        )
        .unwrap(),
        "test-session",
        openfrag_clips::ClipOrigin::manual_flag("test-flag", 1_000, BTreeSet::default()).unwrap(),
        "Test clip",
    )
    .unwrap();
    let mut repository = StorageClipRepository::new(Arc::new(Mutex::new(storage)));

    let error = repository.compare_and_swap(0, &clip).unwrap_err();

    assert!(error.message.contains("revision is not stored"));
}
