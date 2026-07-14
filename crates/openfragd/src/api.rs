//! Typed, local-only API routes for the dashboard integration seam.
#![allow(clippy::missing_errors_doc)]

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RatingState { Unavailable, Preview, Rated }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HealthResponse { pub rating_state: RatingState, pub rating: Option<String> }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupCheck { pub id: String, pub status: String, pub summary: String }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupResponse { pub checks: Vec<SetupCheck> }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportJob { pub id: String, pub status: String, pub progress: Option<String> }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MatchSummary { pub id: String, pub map: Option<String>, pub rating_state: RatingState }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReceiptRef { pub id: String }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MatchDetail { pub id: String, pub map: Option<String>, pub rating_state: RatingState, pub rating: Option<String>, pub receipts: Vec<ReceiptRef> }
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Clip { pub id: String, pub title: Option<String>, pub note: Option<String>, pub tags: Vec<String> }
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ClipUpdate { pub title: String, pub note: String, pub tags: Vec<String>, pub decision: ClipDecision }
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipDecision { Keep, Reject }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedFile { pub filename: String, pub bytes: Vec<u8> }
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApiError { NotFound, Invalid(String), Unavailable(String) }
impl ApiError { fn response(&self) -> (StatusCode, Json<Value>) { let status = match self { Self::NotFound => StatusCode::NOT_FOUND, Self::Invalid(_) => StatusCode::BAD_REQUEST, Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE }; let message = match self { Self::NotFound => "not found", Self::Invalid(value) | Self::Unavailable(value) => value }; (status, Json(json!({"message":message}))) } }

/// Application-owned operations. Implement this with storage, importer, capture, and diagnostics adapters.
pub trait LocalApi: Send + Sync + 'static {
    fn health(&self) -> Result<HealthResponse, ApiError>;
    fn setup(&self) -> Result<SetupResponse, ApiError>;
    fn import(&self, file: ImportedFile) -> Result<ImportJob, ApiError>;
    fn import_status(&self, id: &str) -> Result<ImportJob, ApiError>;
    fn matches(&self) -> Result<Vec<MatchSummary>, ApiError>;
    fn match_detail(&self, id: &str) -> Result<MatchDetail, ApiError>;
    fn receipt(&self, id: &str) -> Result<Value, ApiError>;
    fn clips(&self) -> Result<Vec<Clip>, ApiError>;
    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<Clip, ApiError>;
    fn trim_clip(&self, id: &str) -> Result<Value, ApiError>;
    fn export_clip(&self, id: &str) -> Result<Value, ApiError>;
    fn manual_flag(&self) -> Result<Value, ApiError>;
    fn diagnostics(&self) -> Result<Value, ApiError>;
}

#[derive(Clone)] pub struct ApiState { service: Arc<dyn LocalApi> }
impl ApiState { #[must_use] pub fn new(service: Arc<dyn LocalApi>) -> Self { Self { service } } }
/// Future daemon integration: merge this router into `app()` with the daemon's concrete `LocalApi`.
#[must_use]
pub fn router(service: Arc<dyn LocalApi>) -> Router { Router::new().route("/api/health", get(health)).route("/api/setup", get(setup)).route("/api/imports", post(import)).route("/api/imports/{id}", get(import_status)).route("/api/matches", get(matches)).route("/api/matches/{id}", get(match_detail)).route("/api/receipts/{id}", get(receipt)).route("/api/clips", get(clips)).route("/api/clips/{id}", patch(update_clip)).route("/api/clips/{id}/trim", post(trim_clip)).route("/api/clips/{id}/export", post(export_clip)).route("/api/manual-flag", post(manual_flag)).route("/api/diagnostics", get(diagnostics)).with_state(ApiState::new(service)) }

macro_rules! get_handler { ($name:ident, $method:ident, $type:ty) => { async fn $name(State(state): State<ApiState>) -> Result<Json<$type>, (StatusCode, Json<Value>)> { state.service.$method().map(Json).map_err(|error| error.response()) } }; }
get_handler!(health, health, HealthResponse); get_handler!(setup, setup, SetupResponse); get_handler!(matches, matches, Vec<MatchSummary>); get_handler!(clips, clips, Vec<Clip>); get_handler!(diagnostics, diagnostics, Value); get_handler!(manual_flag, manual_flag, Value);
async fn import(State(state): State<ApiState>, mut multipart: Multipart) -> Result<Json<ImportJob>, (StatusCode, Json<Value>)> { let Some(field) = multipart.next_field().await.map_err(|error| ApiError::Invalid(error.to_string()).response())? else { return Err(ApiError::Invalid("missing .dem file".into()).response()); }; let filename = field.file_name().unwrap_or_default().to_owned(); if !filename.ends_with(".dem") { return Err(ApiError::Invalid("file must end in .dem".into()).response()); } let bytes = field.bytes().await.map_err(|error| ApiError::Invalid(error.to_string()).response())?.to_vec(); state.service.import(ImportedFile { filename, bytes }).map(Json).map_err(|error| error.response()) }
async fn import_status(State(state): State<ApiState>, Path(id): Path<String>) -> Result<Json<ImportJob>, (StatusCode, Json<Value>)> { state.service.import_status(&id).map(Json).map_err(|error| error.response()) }
async fn match_detail(State(state): State<ApiState>, Path(id): Path<String>) -> Result<Json<MatchDetail>, (StatusCode, Json<Value>)> { state.service.match_detail(&id).map(Json).map_err(|error| error.response()) }
async fn receipt(State(state): State<ApiState>, Path(id): Path<String>) -> Result<Json<Value>, (StatusCode, Json<Value>)> { state.service.receipt(&id).map(Json).map_err(|error| error.response()) }
async fn update_clip(State(state): State<ApiState>, Path(id): Path<String>, Json(update): Json<ClipUpdate>) -> Result<Json<Clip>, (StatusCode, Json<Value>)> { state.service.update_clip(&id, update).map(Json).map_err(|error| error.response()) }
async fn trim_clip(State(state): State<ApiState>, Path(id): Path<String>) -> Result<Json<Value>, (StatusCode, Json<Value>)> { state.service.trim_clip(&id).map(Json).map_err(|error| error.response()) }
async fn export_clip(State(state): State<ApiState>, Path(id): Path<String>) -> Result<Json<Value>, (StatusCode, Json<Value>)> { state.service.export_clip(&id).map(Json).map_err(|error| error.response()) }

/// Safe boot-time default: it exposes empty local collections and says operational work is unavailable.
#[derive(Default)] pub struct EmptyLocalApi;
impl LocalApi for EmptyLocalApi {
    fn health(&self) -> Result<HealthResponse, ApiError> { Ok(HealthResponse { rating_state: RatingState::Unavailable, rating: None }) }
    fn setup(&self) -> Result<SetupResponse, ApiError> { Ok(SetupResponse { checks: vec![] }) }
    fn import(&self, _: ImportedFile) -> Result<ImportJob, ApiError> { Err(ApiError::Unavailable("local import service is not connected".into())) }
    fn import_status(&self, _: &str) -> Result<ImportJob, ApiError> { Err(ApiError::NotFound) }
    fn matches(&self) -> Result<Vec<MatchSummary>, ApiError> { Ok(vec![]) }
    fn match_detail(&self, _: &str) -> Result<MatchDetail, ApiError> { Err(ApiError::NotFound) }
    fn receipt(&self, _: &str) -> Result<Value, ApiError> { Err(ApiError::NotFound) }
    fn clips(&self) -> Result<Vec<Clip>, ApiError> { Ok(vec![]) }
    fn update_clip(&self, _: &str, _: ClipUpdate) -> Result<Clip, ApiError> { Err(ApiError::NotFound) }
    fn trim_clip(&self, _: &str) -> Result<Value, ApiError> { Err(ApiError::NotFound) }
    fn export_clip(&self, _: &str) -> Result<Value, ApiError> { Err(ApiError::NotFound) }
    fn manual_flag(&self) -> Result<Value, ApiError> { Err(ApiError::Unavailable("capture service is not connected".into())) }
    fn diagnostics(&self) -> Result<Value, ApiError> { Ok(json!({"capture":"unavailable","gsi":"unavailable"})) }
}
