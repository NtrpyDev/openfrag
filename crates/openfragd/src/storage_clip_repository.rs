//! SQLite-backed clip model, review, and export lifecycle persistence.
#![allow(clippy::missing_errors_doc)]

use crate::clip_ports::{ClipPortError, ClipStore};
use openfrag_clips::{
    ArtifactProvenance, Clip, ClipId, ClipOrigin, ClipRehydrationError, ClipRepository,
    DurableClipSnapshot, RepositoryError, ReviewState,
};
use openfrag_storage::{
    ClipDetailRecord, ClipExportRecord, ClipReviewDecision, DurableClipModelRecord,
    DurableClipOriginRecord, Error as StorageError, ExportId, Storage,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageClipRepositoryError {
    NotFound,
    Unavailable(String),
    Conflict(String),
    InconsistentModel(String),
    DerivedClipIdentityRequired,
    Storage(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableReviewDecision {
    Keep,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableReviewUpdate {
    pub title: String,
    pub note: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub decision: DurableReviewDecision,
}

#[derive(Clone)]
pub struct StorageClipRepository {
    storage: Arc<Mutex<Storage>>,
}

impl StorageClipRepository {
    #[must_use]
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn load_durable(&self, clip_id: &str) -> Result<Clip, StorageClipRepositoryError> {
        let record = self
            .lock()?
            .durable_clip_model(clip_id)
            .map_err(storage_error)?;
        clip_from_record(record)
    }

    pub fn persist_review(
        &self,
        clip_id: &str,
        update: &DurableReviewUpdate,
    ) -> Result<ClipDetailRecord, StorageClipRepositoryError> {
        let current = self.load_durable(clip_id)?;
        let storage = self.lock()?;
        storage
            .compare_and_swap_clip_review(
                clip_id,
                current.revision(),
                Some(&update.title),
                Some(&update.note),
                &update.tags,
                match update.decision {
                    DurableReviewDecision::Keep => ClipReviewDecision::Keep,
                    DurableReviewDecision::Reject => ClipReviewDecision::Reject,
                },
                update.favorite,
            )
            .map_err(storage_error)?;
        storage.clip_detail(clip_id).map_err(storage_error)
    }

    pub fn compare_and_swap_durable(
        &self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), StorageClipRepositoryError> {
        if updated.derivative().is_some() {
            return Err(StorageClipRepositoryError::DerivedClipIdentityRequired);
        }
        if updated.revision() != expected_revision.saturating_add(1) {
            return Err(StorageClipRepositoryError::InconsistentModel(
                "updated clip revision is not the expected successor".into(),
            ));
        }
        let decision = match updated.review_state() {
            ReviewState::Provisional => ClipReviewDecision::Pending,
            ReviewState::Reviewed { .. } => ClipReviewDecision::Keep,
            ReviewState::Rejected { .. } => ClipReviewDecision::Reject,
        };
        let storage = self.lock()?;
        let revision = storage
            .compare_and_swap_clip_review(
                updated.id().as_str(),
                expected_revision,
                Some(updated.title()),
                Some(updated.note()),
                &updated.tags().iter().cloned().collect::<Vec<_>>(),
                decision,
                updated.favorite(),
            )
            .map_err(storage_error)?;
        if revision != updated.revision() {
            return Err(StorageClipRepositoryError::InconsistentModel(
                "storage committed an unexpected clip revision".into(),
            ));
        }
        Ok(())
    }

    pub fn begin_export(&self, export: &ExportId) -> Result<(), StorageClipRepositoryError> {
        self.lock()?
            .begin_clip_export(export)
            .map_err(storage_error)
    }

    pub fn succeed_export(
        &self,
        export: &ExportId,
        output_sha256: &str,
        output_byte_length: u64,
        output_media_profile: &str,
    ) -> Result<(), StorageClipRepositoryError> {
        self.lock()?
            .succeed_clip_export(
                export,
                output_sha256,
                output_byte_length,
                output_media_profile,
            )
            .map_err(storage_error)
    }

    pub fn fail_export(
        &self,
        export: &ExportId,
        error_code: &str,
    ) -> Result<(), StorageClipRepositoryError> {
        self.lock()?
            .fail_clip_export(export, error_code)
            .map_err(storage_error)
    }

    pub fn export(
        &self,
        export: &ExportId,
    ) -> Result<ClipExportRecord, StorageClipRepositoryError> {
        self.lock()?.clip_export(export).map_err(storage_error)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Storage>, StorageClipRepositoryError> {
        self.storage
            .lock()
            .map_err(|_| StorageClipRepositoryError::Storage("storage lock is unavailable".into()))
    }
}

impl ClipStore for StorageClipRepository {
    fn load(&mut self, clip_id: &str) -> Result<Clip, ClipPortError> {
        self.load_durable(clip_id).map_err(clip_port_error)
    }
}

impl ClipRepository for StorageClipRepository {
    fn compare_and_swap(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError> {
        self.compare_and_swap_durable(expected_revision, updated)
            .map_err(|error| RepositoryError::new(format!("{error:?}")))
    }
}

fn clip_from_record(record: DurableClipModelRecord) -> Result<Clip, StorageClipRepositoryError> {
    let reviewed_at_ms = record.reviewed_at_ms;
    let byte_length = u64::try_from(record.artifact.byte_length).map_err(|_| {
        StorageClipRepositoryError::InconsistentModel("negative artifact byte length".into())
    })?;
    let artifact_id = record.artifact.sha256.clone();
    let source_capture = ArtifactProvenance::new(
        artifact_id,
        record.artifact.path,
        record.artifact.sha256,
        record.duration_ms,
        byte_length,
    )
    .map_err(|error| model_error(&error))?;
    let origin = match record.origin {
        DurableClipOriginRecord::Auto {
            trigger_receipts,
            evidence_receipts,
        } => ClipOrigin::auto_highlight(
            trigger_receipts.into_iter().collect::<BTreeSet<_>>(),
            evidence_receipts.into_iter().collect::<BTreeSet<_>>(),
        )
        .map_err(|error| model_error(&error))?,
        DurableClipOriginRecord::Manual {
            flag_receipt_id,
            flag_time_ms,
            overlapping_auto_receipts,
        } => ClipOrigin::manual_flag(
            flag_receipt_id,
            flag_time_ms,
            overlapping_auto_receipts
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
        .map_err(|error| model_error(&error))?,
    };
    let review_state = match record.detail.review_decision {
        ClipReviewDecision::Pending => ReviewState::Provisional,
        ClipReviewDecision::Keep => ReviewState::Reviewed {
            reviewed_at_ms: review_time(reviewed_at_ms)?,
        },
        ClipReviewDecision::Reject => ReviewState::Rejected {
            rejected_at_ms: review_time(reviewed_at_ms)?,
        },
    };
    Clip::from_durable_snapshot(DurableClipSnapshot {
        id: ClipId::new(record.detail.summary.id).map_err(|error| model_error(&error))?,
        revision: record.revision,
        source_capture,
        capture_session_id: record.capture_session_id,
        origin,
        review_state,
        title: record.detail.summary.title.unwrap_or_default(),
        note: record.detail.note.unwrap_or_default(),
        tags: record.detail.tags.into_iter().collect(),
        favorite: record.detail.favorite,
        derivative: None,
    })
    .map_err(|error| rehydration_error(&error))
}

fn review_time(value: Option<u64>) -> Result<u64, StorageClipRepositoryError> {
    value.ok_or_else(|| {
        StorageClipRepositoryError::InconsistentModel("reviewed clip has no review time".into())
    })
}

fn model_error(error: &openfrag_clips::ModelError) -> StorageClipRepositoryError {
    StorageClipRepositoryError::InconsistentModel(format!("{error:?}"))
}

fn rehydration_error(error: &ClipRehydrationError) -> StorageClipRepositoryError {
    StorageClipRepositoryError::InconsistentModel(format!("{error:?}"))
}

fn storage_error(error: StorageError) -> StorageClipRepositoryError {
    match error {
        StorageError::NotFound(_) => StorageClipRepositoryError::NotFound,
        StorageError::Unavailable(reason) => StorageClipRepositoryError::Unavailable(reason.into()),
        StorageError::Conflict(reason) => StorageClipRepositoryError::Conflict(reason.into()),
        other => StorageClipRepositoryError::Storage(format!("{other:?}")),
    }
}

fn clip_port_error(error: StorageClipRepositoryError) -> ClipPortError {
    match error {
        StorageClipRepositoryError::NotFound => ClipPortError::NotFound,
        StorageClipRepositoryError::InconsistentModel(message) => ClipPortError::Invalid(message),
        StorageClipRepositoryError::Unavailable(message)
        | StorageClipRepositoryError::Conflict(message)
        | StorageClipRepositoryError::Storage(message) => ClipPortError::Unavailable(message),
        StorageClipRepositoryError::DerivedClipIdentityRequired => ClipPortError::Unavailable(
            "trim creates a new durable clip identity and requires the derivative commit boundary"
                .into(),
        ),
    }
}
