use openfrag_clips::{
    ArtifactProvenance, Clip, ClipId, ClipOrigin, ClipRehydrationError, DerivativeProvenance,
    DurableClipSnapshot, ReviewState, TrimRange,
};
use std::{collections::BTreeSet, path::PathBuf};

fn source() -> ArtifactProvenance {
    ArtifactProvenance::new(
        "source-artifact",
        PathBuf::from("/var/lib/openfrag/source.mkv"),
        "source-sha256",
        42_000,
        8_000_000,
    )
    .unwrap()
}

fn derivative(source_artifact_id: &str, end_ms: u64) -> DerivativeProvenance {
    DerivativeProvenance::new(
        ArtifactProvenance::new(
            "derivative-artifact",
            PathBuf::from("/var/lib/openfrag/trimmed.mp4"),
            "derivative-sha256",
            30_000,
            5_000_000,
        )
        .unwrap(),
        source_artifact_id,
        TrimRange::new(5_000, end_ms).unwrap(),
    )
}

fn reviewed_snapshot() -> DurableClipSnapshot {
    DurableClipSnapshot {
        id: ClipId::new("clip-42").unwrap(),
        revision: 7,
        source_capture: source(),
        capture_session_id: "capture-session-1".into(),
        origin: ClipOrigin::auto_highlight(
            BTreeSet::from(["trigger-receipt".into()]),
            BTreeSet::from(["evidence-receipt".into()]),
        )
        .unwrap(),
        review_state: ReviewState::Reviewed {
            reviewed_at_ms: 99_000,
        },
        title: "Mirage clutch".into(),
        note: "Held B".into(),
        tags: BTreeSet::from(["clutch".into(), "mirage".into()]),
        favorite: true,
        derivative: Some(derivative("source-artifact", 35_000)),
    }
}

#[test]
fn durable_snapshot_reconstructs_every_private_clip_field_exactly() {
    let snapshot = reviewed_snapshot();

    let clip = Clip::from_durable_snapshot(snapshot.clone()).unwrap();

    assert_eq!(clip.id(), &snapshot.id);
    assert_eq!(clip.revision(), snapshot.revision);
    assert_eq!(clip.source_capture(), &snapshot.source_capture);
    assert_eq!(clip.capture_session_id(), snapshot.capture_session_id);
    assert_eq!(clip.origin(), &snapshot.origin);
    assert_eq!(clip.review_state(), &snapshot.review_state);
    assert_eq!(clip.title(), snapshot.title);
    assert_eq!(clip.note(), snapshot.note);
    assert_eq!(clip.tags(), &snapshot.tags);
    assert_eq!(clip.favorite(), snapshot.favorite);
    assert_eq!(clip.derivative(), snapshot.derivative.as_ref());
}

#[test]
fn manual_rejected_snapshot_is_rehydrated_without_replaying_transitions() {
    let mut snapshot = reviewed_snapshot();
    snapshot.revision = 3;
    snapshot.origin = ClipOrigin::manual_flag(
        "manual-flag-receipt",
        50_000,
        BTreeSet::from(["overlapping-auto".into()]),
    )
    .unwrap();
    snapshot.review_state = ReviewState::Rejected {
        rejected_at_ms: 101_000,
    };
    snapshot.derivative = None;

    let clip = Clip::from_durable_snapshot(snapshot).unwrap();

    assert_eq!(clip.revision(), 3);
    assert!(matches!(
        clip.review_state(),
        ReviewState::Rejected {
            rejected_at_ms: 101_000
        }
    ));
}

#[test]
fn rehydration_rejects_empty_capture_or_origin_evidence() {
    let mut snapshot = reviewed_snapshot();
    snapshot.capture_session_id.clear();
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::EmptyCaptureSession)
    );

    let mut snapshot = reviewed_snapshot();
    snapshot.origin = ClipOrigin::AutoHighlight {
        trigger_receipts: BTreeSet::from([String::new()]),
        evidence_receipts: BTreeSet::from(["evidence".into()]),
    };
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::EmptyOriginEvidence)
    );
}

#[test]
fn rehydration_rejects_review_state_and_revision_inconsistency() {
    let mut snapshot = reviewed_snapshot();
    snapshot.revision = 0;
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::InconsistentRevision)
    );

    let mut snapshot = reviewed_snapshot();
    snapshot.revision = 0;
    snapshot.review_state = ReviewState::Provisional;
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::ProvisionalStateHasReviewData)
    );
}

#[test]
fn rehydration_rejects_derivative_source_and_trim_inconsistency() {
    let mut snapshot = reviewed_snapshot();
    snapshot.derivative = Some(derivative("another-source", 35_000));
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::DerivativeHasWrongSource)
    );

    let mut snapshot = reviewed_snapshot();
    snapshot.derivative = Some(derivative("source-artifact", 50_000));
    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::TrimOutsideSource)
    );
}

#[test]
fn rehydration_rejects_a_derivative_that_reuses_the_source_artifact() {
    let mut snapshot = reviewed_snapshot();
    snapshot.derivative = Some(DerivativeProvenance::new(
        source(),
        "source-artifact",
        TrimRange::new(5_000, 35_000).unwrap(),
    ));

    assert_eq!(
        Clip::from_durable_snapshot(snapshot),
        Err(ClipRehydrationError::DerivativeReusesSourceArtifact)
    );
}
