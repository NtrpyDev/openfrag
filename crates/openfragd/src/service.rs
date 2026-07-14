//! Storage-backed implementation of the dashboard's injected local API.
#![allow(clippy::missing_errors_doc)]

use crate::api::{ApiError, Clip, ClipUpdate, HealthResponse, ImportedFile, ImportJob, LocalApi, MatchDetail, MatchSummary, RatingState, ReceiptRef, SetupResponse};
use openfrag_storage::{ImportPhase, Storage, StoredRatingAvailability};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub trait MutationPorts: Send + Sync + 'static {
    fn import(&self, _: ImportedFile) -> Result<ImportJob, ApiError> { Err(ApiError::Unavailable("local import pipeline is not connected".into())) }
    fn update_clip(&self, _: &str, _: ClipUpdate) -> Result<Clip, ApiError> { Err(ApiError::Unavailable("clip review pipeline is not connected".into())) }
    fn trim(&self, _: &str) -> Result<Value, ApiError> { Err(ApiError::Unavailable("clip trim pipeline is not connected".into())) }
    fn export(&self, _: &str) -> Result<Value, ApiError> { Err(ApiError::Unavailable("local export pipeline is not connected".into())) }
    fn manual_flag(&self) -> Result<Value, ApiError> { Err(ApiError::Unavailable("capture pipeline is not connected".into())) }
}
#[derive(Default)] pub struct UnavailablePorts; impl MutationPorts for UnavailablePorts {}
pub struct StorageApi<P> { storage: Arc<Mutex<Storage>>, ports: P, setup: SetupResponse }
impl<P> StorageApi<P> { #[must_use] pub fn new(storage: Arc<Mutex<Storage>>, ports: P, setup: SetupResponse) -> Self { Self { storage, ports, setup } } }
impl<P: MutationPorts> LocalApi for StorageApi<P> {
    fn health(&self) -> Result<HealthResponse, ApiError> { Ok(HealthResponse { rating_state: RatingState::Unavailable, rating: None }) }
    fn setup(&self) -> Result<SetupResponse, ApiError> { Ok(self.setup.clone()) }
    fn import(&self, file: ImportedFile) -> Result<ImportJob, ApiError> { self.ports.import(file) }
    fn import_status(&self, id: &str) -> Result<ImportJob, ApiError> { let job=self.storage.lock().map_err(|_| unavailable("storage lock"))?.import_job_by_id(id).map_err(storage_error)?; Ok(ImportJob { id: id.into(), status: phase(job.phase).into(), progress: Some(format!("{}%", job.progress_bp / 100)) }) }
    fn matches(&self) -> Result<Vec<MatchSummary>, ApiError> { Ok(self.storage.lock().map_err(|_| unavailable("storage lock"))?.list_matches().map_err(storage_error)?.into_iter().map(|record| MatchSummary { id:record.id, map:record.map_name, rating_state:rating_state(&record.rating) }).collect()) }
    fn match_detail(&self, id: &str) -> Result<MatchDetail, ApiError> { let detail=self.storage.lock().map_err(|_| unavailable("storage lock"))?.match_detail(id).map_err(storage_error)?; let (state,rating)=rating(&detail.summary.rating); Ok(MatchDetail { id:detail.summary.id, map:detail.summary.map_name, rating_state:state, rating, receipts:detail.receipt_ids.into_iter().map(|id| ReceiptRef{id}).collect() }) }
    fn receipt(&self, id: &str) -> Result<Value, ApiError> { let record=self.storage.lock().map_err(|_| unavailable("storage lock"))?.receipt_by_id(id).map_err(storage_error)?; Ok(json!({"id":record.id,"metric_key":record.metric_key,"event_tick":record.event_tick,"round_id":record.round_id,"raw_payload":record.raw_payload_json,"parameters":record.parameters_json})) }
    fn clips(&self) -> Result<Vec<Clip>, ApiError> { Ok(self.storage.lock().map_err(|_| unavailable("storage lock"))?.list_clips().map_err(storage_error)?.into_iter().map(|record| Clip { id:record.id, title:record.title, note:None, tags:vec![] }).collect()) }
    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<Clip, ApiError> { self.ports.update_clip(id, update) }
    fn trim_clip(&self, id: &str) -> Result<Value, ApiError> { self.ports.trim(id) }
    fn export_clip(&self, id: &str) -> Result<Value, ApiError> { self.ports.export(id) }
    fn manual_flag(&self) -> Result<Value, ApiError> { self.ports.manual_flag() }
    fn diagnostics(&self) -> Result<Value, ApiError> { Ok(json!({"capture":"unavailable until pipeline is connected","gsi":"unavailable until pipeline is connected"})) }
}
fn unavailable(name: &str) -> ApiError { ApiError::Unavailable(format!("{name} is unavailable")) }
fn storage_error(error: openfrag_storage::Error) -> ApiError { match error { openfrag_storage::Error::NotFound(_) => ApiError::NotFound, other => ApiError::Unavailable(format!("local storage error: {other:?}")) } }
fn phase(phase: ImportPhase) -> &'static str { match phase { ImportPhase::Queued=>"queued", ImportPhase::Leased=>"processing", ImportPhase::Succeeded=>"completed", ImportPhase::Failed=>"failed", ImportPhase::Cancelled=>"cancelled" } }
fn rating_state(rating: &StoredRatingAvailability) -> RatingState { match rating { StoredRatingAvailability::Available { .. } => RatingState::Rated, StoredRatingAvailability::Pending => RatingState::Preview, StoredRatingAvailability::Unavailable { .. } => RatingState::Unavailable } }
fn rating(value: &StoredRatingAvailability) -> (RatingState, Option<String>) { let state=rating_state(value); let rating=match value { StoredRatingAvailability::Available { rating_bp, .. } => Some(format!("{}.{:02}", rating_bp / 100, rating_bp.unsigned_abs() % 100)), _ => None }; (state,rating) }
