//! Storage-backed implementation of the dashboard's injected local API.
#![allow(clippy::missing_errors_doc)]

use crate::api::{
    ApiError, Clip, ClipTrimRequest, ClipUpdate, HealthResponse, ImportJob, ImportedFile, LocalApi,
    MatchDetail, MatchSummary, RatingState, ReceiptRef, SetupResponse,
};
use openfrag_pipeline::{ImportOutcome, ImportRequest, ImportService, PinnedParser, PipelineError};
use openfrag_storage::{ImportPhase, Storage, StoredRatingAvailability};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub trait MutationPorts: Send + Sync + 'static {
    fn import(&self, _: ImportedFile) -> Result<ImportJob, ApiError> {
        Err(ApiError::Unavailable(
            "local import pipeline is not connected".into(),
        ))
    }
    fn update_clip(&self, _: &str, _: ClipUpdate) -> Result<Clip, ApiError> {
        Err(ApiError::Unavailable(
            "clip review pipeline is not connected".into(),
        ))
    }
    fn trim(&self, _: &str, _: ClipTrimRequest) -> Result<Value, ApiError> {
        Err(ApiError::Unavailable(
            "clip trim pipeline is not connected".into(),
        ))
    }
    fn export(&self, _: &str) -> Result<Value, ApiError> {
        Err(ApiError::Unavailable(
            "local export pipeline is not connected".into(),
        ))
    }
    fn manual_flag(&self) -> Result<Value, ApiError> {
        Err(ApiError::Unavailable(
            "capture pipeline is not connected".into(),
        ))
    }
}
#[derive(Default)]
pub struct UnavailablePorts;
impl MutationPorts for UnavailablePorts {}

/// Composes independently testable import, clip, and live-capture adapters.
pub struct CompositePorts {
    imports: Arc<dyn MutationPorts>,
    clips: Arc<dyn MutationPorts>,
    live: Arc<dyn MutationPorts>,
}

impl CompositePorts {
    #[must_use]
    pub fn new(
        imports: Arc<dyn MutationPorts>,
        clips: Arc<dyn MutationPorts>,
        live: Arc<dyn MutationPorts>,
    ) -> Self {
        Self {
            imports,
            clips,
            live,
        }
    }
}

impl MutationPorts for CompositePorts {
    fn import(&self, file: ImportedFile) -> Result<ImportJob, ApiError> {
        self.imports.import(file)
    }

    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<Clip, ApiError> {
        self.clips.update_clip(id, update)
    }

    fn trim(&self, id: &str, request: ClipTrimRequest) -> Result<Value, ApiError> {
        self.clips.trim(id, request)
    }

    fn export(&self, id: &str) -> Result<Value, ApiError> {
        self.clips.export(id)
    }

    fn manual_flag(&self) -> Result<Value, ApiError> {
        self.live.manual_flag()
    }
}

pub struct PipelinePorts {
    storage: Arc<Mutex<Storage>>,
    data_directory: PathBuf,
    local_steam_id: Option<u64>,
}

impl PipelinePorts {
    #[must_use]
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        data_directory: PathBuf,
        local_steam_id: Option<u64>,
    ) -> Self {
        Self {
            storage,
            data_directory,
            local_steam_id,
        }
    }
}

impl MutationPorts for PipelinePorts {
    fn import(&self, file: ImportedFile) -> Result<ImportJob, ApiError> {
        let local_steam_id = self.local_steam_id.ok_or_else(|| {
            ApiError::Unavailable("configure the local Steam identity before importing".into())
        })?;
        let incoming = self.data_directory.join("incoming");
        fs::create_dir_all(&incoming).map_err(|error| {
            ApiError::Unavailable(format!("cannot prepare local import: {error}"))
        })?;
        let source = incoming.join(format!("{}.dem", Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&source)
            .map_err(|error| ApiError::Unavailable(format!("cannot stage local Demo: {error}")))?;
        output
            .write_all(&file.bytes)
            .and_then(|()| output.sync_all())
            .map_err(|error| ApiError::Unavailable(format!("cannot stage local Demo: {error}")))?;
        drop(output);
        let outcome = {
            let mut storage = self
                .storage
                .lock()
                .map_err(|_| unavailable("storage lock"))?;
            ImportService::new(&mut storage, PinnedParser).import(ImportRequest {
                source: &source,
                local_steam_id,
                worker: "openfragd-http",
                lease_expires_at_ms: i64::MAX,
            })
        };
        let _ = fs::remove_file(&source);
        let outcome = outcome.map_err(|error| pipeline_error(&error))?;
        let job_id = match outcome {
            ImportOutcome::Deduplicated { job_id, .. }
            | ImportOutcome::Ready { job_id, .. }
            | ImportOutcome::RatingUnavailable { job_id, .. } => job_id,
        };
        Ok(ImportJob {
            id: job_id.as_str().to_owned(),
            status: "completed".into(),
            progress: Some("100%".into()),
        })
    }
}
pub struct StorageApi<P> {
    storage: Arc<Mutex<Storage>>,
    ports: P,
    setup: SetupResponse,
}
impl<P> StorageApi<P> {
    #[must_use]
    pub fn new(storage: Arc<Mutex<Storage>>, ports: P, setup: SetupResponse) -> Self {
        Self {
            storage,
            ports,
            setup,
        }
    }
}
impl<P: MutationPorts> LocalApi for StorageApi<P> {
    fn health(&self) -> Result<HealthResponse, ApiError> {
        Ok(HealthResponse {
            rating_state: RatingState::Unavailable,
            rating: None,
        })
    }
    fn setup(&self) -> Result<SetupResponse, ApiError> {
        Ok(self.setup.clone())
    }
    fn import(&self, file: ImportedFile) -> Result<ImportJob, ApiError> {
        self.ports.import(file)
    }
    fn import_status(&self, id: &str) -> Result<ImportJob, ApiError> {
        let job = self
            .storage
            .lock()
            .map_err(|_| unavailable("storage lock"))?
            .import_job_by_id(id)
            .map_err(storage_error)?;
        Ok(ImportJob {
            id: id.into(),
            status: phase(job.phase).into(),
            progress: Some(format!("{}%", job.progress_bp / 100)),
        })
    }
    fn matches(&self) -> Result<Vec<MatchSummary>, ApiError> {
        Ok(self
            .storage
            .lock()
            .map_err(|_| unavailable("storage lock"))?
            .list_matches()
            .map_err(storage_error)?
            .into_iter()
            .map(|record| MatchSummary {
                id: record.id,
                map: record.map_name,
                rating_state: rating_state(&record.rating),
            })
            .collect())
    }
    fn match_detail(&self, id: &str) -> Result<MatchDetail, ApiError> {
        let detail = self
            .storage
            .lock()
            .map_err(|_| unavailable("storage lock"))?
            .match_detail(id)
            .map_err(storage_error)?;
        let (state, rating) = rating(&detail.summary.rating);
        Ok(MatchDetail {
            id: detail.summary.id,
            map: detail.summary.map_name,
            rating_state: state,
            rating,
            receipts: detail
                .receipt_ids
                .into_iter()
                .map(|id| ReceiptRef { id })
                .collect(),
        })
    }
    fn receipt(&self, id: &str) -> Result<Value, ApiError> {
        let record = self
            .storage
            .lock()
            .map_err(|_| unavailable("storage lock"))?
            .receipt_by_id(id)
            .map_err(storage_error)?;
        Ok(
            json!({"id":record.id,"metric_key":record.metric_key,"event_tick":record.event_tick,"round_id":record.round_id,"raw_payload":record.raw_payload_json,"parameters":record.parameters_json}),
        )
    }
    fn clips(&self) -> Result<Vec<Clip>, ApiError> {
        Ok(self
            .storage
            .lock()
            .map_err(|_| unavailable("storage lock"))?
            .list_clips()
            .map_err(storage_error)?
            .into_iter()
            .map(|record| Clip {
                id: record.id,
                title: record.title,
                note: None,
                tags: vec![],
            })
            .collect())
    }
    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<Clip, ApiError> {
        self.ports.update_clip(id, update)
    }
    fn trim_clip(&self, id: &str, request: ClipTrimRequest) -> Result<Value, ApiError> {
        self.ports.trim(id, request)
    }
    fn export_clip(&self, id: &str) -> Result<Value, ApiError> {
        self.ports.export(id)
    }
    fn manual_flag(&self) -> Result<Value, ApiError> {
        self.ports.manual_flag()
    }
    fn diagnostics(&self) -> Result<Value, ApiError> {
        let status = |id: &str| {
            self.setup
                .checks
                .iter()
                .find(|check| check.id == id)
                .map_or("unknown", |check| check.status.as_str())
        };
        Ok(json!({"capture":status("capture"),"gsi":status("gsi")}))
    }
}
fn unavailable(name: &str) -> ApiError {
    ApiError::Unavailable(format!("{name} is unavailable"))
}
fn storage_error(error: openfrag_storage::Error) -> ApiError {
    match error {
        openfrag_storage::Error::NotFound(_) => ApiError::NotFound,
        other => ApiError::Unavailable(format!("local storage error: {other:?}")),
    }
}
fn pipeline_error(error: &PipelineError) -> ApiError {
    match error {
        PipelineError::InvalidSource
        | PipelineError::UnsupportedDemo
        | PipelineError::Size
        | PipelineError::SourceChanged => {
            ApiError::Invalid(format!("local Demo rejected: {error:?}"))
        }
        PipelineError::InsufficientHeadroom
        | PipelineError::Parser(_)
        | PipelineError::Storage(_)
        | PipelineError::Reconciliation(_) => {
            ApiError::Unavailable(format!("local Demo import failed: {error:?}"))
        }
    }
}
fn phase(phase: ImportPhase) -> &'static str {
    match phase {
        ImportPhase::Queued => "queued",
        ImportPhase::Leased => "processing",
        ImportPhase::Succeeded => "completed",
        ImportPhase::Failed => "failed",
        ImportPhase::Cancelled => "cancelled",
    }
}
fn rating_state(rating: &StoredRatingAvailability) -> RatingState {
    match rating {
        StoredRatingAvailability::Available { .. } => RatingState::Rated,
        StoredRatingAvailability::Pending => RatingState::Preview,
        StoredRatingAvailability::Unavailable { .. } => RatingState::Unavailable,
    }
}
fn rating(value: &StoredRatingAvailability) -> (RatingState, Option<String>) {
    let state = rating_state(value);
    let rating = match value {
        StoredRatingAvailability::Available { rating_bp, .. } => Some(format!(
            "{}.{:02}",
            rating_bp / 100,
            rating_bp.unsigned_abs() % 100
        )),
        _ => None,
    };
    (state, rating)
}
