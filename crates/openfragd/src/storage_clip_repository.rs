//! SQLite-backed clip review persistence with explicit model rehydration limits.
#![allow(clippy::missing_errors_doc)]

use crate::clip_ports::{ClipPortError, ClipStore};
use openfrag_clips::{Clip, ClipRepository, RepositoryError};
use openfrag_storage::{ClipDetailRecord, ClipReviewDecision, Error as StorageError, Storage};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MissingDurableClipFields {
    pub duration: bool,
    pub evidence: bool,
    pub revision: bool,
}

impl MissingDurableClipFields {
    const V1_SCHEMA: Self = Self {
        duration: true,
        evidence: true,
        revision: true,
    };
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageClipRepositoryError {
    NotFound,
    Unavailable(MissingDurableClipFields),
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

    /// Persists the schema-supported review projection without claiming model-level CAS.
    pub fn persist_review(
        &self,
        clip_id: &str,
        update: &DurableReviewUpdate,
    ) -> Result<ClipDetailRecord, StorageClipRepositoryError> {
        let storage = self.lock()?;
        storage
            .update_clip_review(
                clip_id,
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

    /// Inspects durable prerequisites and reports every field the v1 schema cannot supply.
    pub fn load_durable(&self, clip_id: &str) -> Result<Clip, StorageClipRepositoryError> {
        let storage = self.lock()?;
        storage.clip_detail(clip_id).map_err(storage_error)?;
        storage.clip_artifact_file(clip_id).map_err(storage_error)?;
        Err(StorageClipRepositoryError::Unavailable(
            MissingDurableClipFields::V1_SCHEMA,
        ))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Storage>, StorageClipRepositoryError> {
        self.storage
            .lock()
            .map_err(|_| StorageClipRepositoryError::Storage("storage lock is unavailable".into()))
    }
}

impl ClipStore for StorageClipRepository {
    fn load(&mut self, clip_id: &str) -> Result<Clip, ClipPortError> {
        self.load_durable(clip_id).map_err(|error| match error {
            StorageClipRepositoryError::NotFound => ClipPortError::NotFound,
            StorageClipRepositoryError::Unavailable(fields) => ClipPortError::Unavailable(format!(
                "durable clip model is unavailable: duration={}, evidence={}, revision={}",
                fields.duration, fields.evidence, fields.revision
            )),
            StorageClipRepositoryError::Storage(message) => ClipPortError::Unavailable(message),
        })
    }
}

impl ClipRepository for StorageClipRepository {
    fn compare_and_swap(
        &mut self,
        _expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError> {
        let storage = self
            .lock()
            .map_err(|error| RepositoryError::new(format!("{error:?}")))?;
        storage
            .clip_detail(updated.id().as_str())
            .map_err(|error| RepositoryError::new(format!("{:?}", storage_error(error))))?;
        Err(RepositoryError::new(
            "durable compare-and-swap is unavailable: the clip revision is not stored",
        ))
    }
}

fn storage_error(error: StorageError) -> StorageClipRepositoryError {
    match error {
        StorageError::NotFound(_) => StorageClipRepositoryError::NotFound,
        other => StorageClipRepositoryError::Storage(format!("{other:?}")),
    }
}
