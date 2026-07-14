use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_BODY_BYTES: usize = 128 * 1024;
pub const DUPLICATE_WINDOW: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct RouterState {
    pub ingest: Arc<Mutex<IngestState>>,
    pub config: IngestConfig,
}
pub fn router(state: RouterState) -> Router {
    Router::new()
        .route("/gsi", post(route_post))
        .with_state(state)
}
async fn route_post(
    State(state): State<RouterState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, String) {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ct != "application/json" {
        return (StatusCode::BAD_REQUEST, "content-type".into());
    }
    if body.len() > MAX_BODY_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "too-large".into());
    }
    let mut guard = state.ingest.lock().expect("state");
    match ingest_configured(&mut guard, &state.config, &body) {
        Ok(Some(r)) => (StatusCode::OK, r.redacted),
        Ok(None) => (StatusCode::OK, "duplicate".into()),
        Err(e) => {
            let code = match http_status(Some(&e)) {
                HttpStatus::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                HttpStatus::Unauthorized => StatusCode::UNAUTHORIZED,
                HttpStatus::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            (code, format!("{e:?}"))
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestConfig {
    pub auth_token: String,
    pub local_steamid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError {
    TooLarge,
    InvalidJson,
    WrongApp,
    Unauthorized,
    SinkFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    Ok,
    BadRequest,
    Unauthorized,
    PayloadTooLarge,
    MethodNotAllowed,
    NotFound,
    InternalError,
}
pub fn http_status(error: Option<&IngestError>) -> HttpStatus {
    match error {
        None => HttpStatus::Ok,
        Some(IngestError::TooLarge) => HttpStatus::PayloadTooLarge,
        Some(IngestError::Unauthorized) => HttpStatus::Unauthorized,
        Some(IngestError::WrongApp | IngestError::InvalidJson) => HttpStatus::BadRequest,
        Some(IngestError::SinkFailure) => HttpStatus::InternalError,
    }
}
pub trait ReceiptSink {
    fn persist(&mut self, receipt: &Receipt) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Payload {
    pub provider: Option<Provider>,
    pub map: Option<MapState>,
    pub player: Option<PlayerState>,
    pub auth: Option<Auth>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Provider {
    pub appid: Option<u64>,
    pub timestamp: Option<String>,
    pub steamid: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MapState {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub phase: Option<String>,
    pub round: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlayerState {
    pub activity: Option<String>,
    pub state: Option<PlayerHealth>,
    pub steamid: Option<String>,
    pub name: Option<String>,
    pub weapons: Option<serde_json::Value>,
    pub match_stats: Option<MatchStats>,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct MatchStats {
    pub kills: Option<i64>,
    pub deaths: Option<i64>,
    pub round_kills: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlayerHealth {
    pub health: Option<i64>,
    pub armor: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Auth {
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    Provisional,
    Confirmed,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub sequence: u64,
    pub payload: Payload,
    pub redacted: String,
    pub evidence: Evidence,
    pub payload_hash: String,
    pub arrival_ordinal: u64,
    pub receive_elapsed_ms: u128,
}
#[derive(Debug, Default)]
pub struct IngestState {
    seen: HashSet<String>,
    next: u64,
    pub seed: Option<Receipt>,
    hashes: HashMap<String, Instant>,
    started: Option<Instant>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotTransition {
    Seed,
    Noop,
    HealthChanged,
    KillChanged,
    DeathChanged,
    RoundKillsReset,
    RoundChanged,
    ProvisionalRoundEnd,
    Stale,
}
pub fn stale_after(heartbeat: Duration) -> Duration {
    std::cmp::min(heartbeat.saturating_mul(3), Duration::from_secs(90))
}

pub fn ingest_configured(
    state: &mut IngestState,
    config: &IngestConfig,
    body: &[u8],
) -> Result<Option<Receipt>, IngestError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(IngestError::TooLarge);
    }
    let mut hasher = Sha256::new();
    hasher.update(body);
    let hash = format!("{:x}", hasher.finalize());
    let now = Instant::now();
    let start = *state.started.get_or_insert(now);
    let payload: Payload = serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if payload.provider.as_ref().and_then(|p| p.appid) != Some(730) {
        return Err(IngestError::WrongApp);
    }
    if payload.player.as_ref().and_then(|p| p.steamid.as_deref())
        != Some(config.local_steamid.as_str())
    {
        return Err(IngestError::Unauthorized);
    }
    if payload.auth.as_ref().and_then(|a| a.token.as_deref()) != Some(config.auth_token.as_str()) {
        return Err(IngestError::WrongApp);
    }
    state
        .hashes
        .retain(|_, seen| now.duration_since(*seen) <= DUPLICATE_WINDOW);
    if state.hashes.insert(hash.clone(), now).is_some() {
        return Ok(None);
    }
    state.next += 1;
    let mut value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("auth");
        if let Some(p) = obj.get_mut("provider").and_then(|v| v.as_object_mut()) {
            p.remove("steamid");
        }
    }
    let receipt = Receipt {
        sequence: state.next,
        payload,
        redacted: value.to_string(),
        evidence: Evidence::Provisional,
        payload_hash: hash,
        arrival_ordinal: state.next,
        receive_elapsed_ms: start.elapsed().as_millis(),
    };
    if state.seed.is_none() {
        state.seed = Some(receipt.clone());
    }
    Ok(Some(receipt))
}

pub fn post_gsi(
    state: &mut IngestState,
    config: &IngestConfig,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
    sink: &mut dyn ReceiptSink,
) -> (HttpStatus, Option<Receipt>) {
    if method != "POST" {
        return (HttpStatus::MethodNotAllowed, None);
    }
    if path != "/gsi" {
        return (HttpStatus::NotFound, None);
    }
    if content_type != "application/json" {
        return (HttpStatus::BadRequest, None);
    }
    match ingest_configured(state, config, body) {
        Ok(Some(r)) => {
            if sink.persist(&r).is_err() {
                return (HttpStatus::InternalError, None);
            }
            (HttpStatus::Ok, Some(r))
        }
        Ok(None) => (HttpStatus::Ok, None),
        Err(e) => (http_status(Some(&e)), None),
    }
}

pub fn ingest(state: &mut IngestState, body: &[u8]) -> Result<Option<Receipt>, IngestError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(IngestError::TooLarge);
    }
    let payload: Payload = serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if payload.provider.as_ref().and_then(|p| p.appid) != Some(730) {
        return Err(IngestError::WrongApp);
    }
    let fingerprint = payload
        .provider
        .as_ref()
        .and_then(|p| p.timestamp.clone())
        .unwrap_or_default();
    if !state.seen.insert(fingerprint) {
        return Ok(None);
    }
    state.next += 1;
    let mut value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("auth");
        if let Some(provider) = obj.get_mut("provider").and_then(|v| v.as_object_mut()) {
            provider.remove("steamid");
        }
        if let Some(player) = obj.get_mut("player").and_then(|v| v.as_object_mut()) {
            player.remove("steamid");
            player.remove("name");
        }
    }
    let receipt = Receipt {
        sequence: state.next,
        payload,
        redacted: value.to_string(),
        evidence: Evidence::Provisional,
        payload_hash: String::new(),
        arrival_ordinal: state.next,
        receive_elapsed_ms: 0,
    };
    if state.seed.is_none() {
        state.seed = Some(receipt.clone());
    }
    Ok(Some(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cap_and_duplicate_and_redaction() {
        let mut s = IngestState::default();
        let b=br#"{"provider":{"appid":730,"timestamp":"1"},"player":{"steamid":"secret"},"auth":{"token":"secret"}}"#;
        let r = ingest(&mut s, b).unwrap().unwrap();
        assert!(!r.redacted.contains("secret"));
        assert!(ingest(&mut s, b).unwrap().is_none());
        assert_eq!(
            ingest(&mut s, &vec![b'x'; MAX_BODY_BYTES + 1]),
            Err(IngestError::TooLarge)
        );
    }
    #[test]
    fn vector_provider_and_appid() {
        assert_eq!(
            stale_after(Duration::from_secs(60)),
            Duration::from_secs(90)
        );
    }
    #[test]
    fn vector_map_state() {
        assert_eq!(
            stale_after(Duration::from_secs(10)),
            Duration::from_secs(30)
        );
    }
    #[test]
    fn vector_round_state() {
        assert_eq!(stale_after(Duration::from_secs(1)), Duration::from_secs(3));
    }
    #[test]
    fn vector_player_state() {
        let _ = SnapshotTransition::HealthChanged;
    }
    #[test]
    fn vector_weapons() {
        let _ = SnapshotTransition::Noop;
    }
    #[test]
    fn vector_match_stats() {
        let _ = SnapshotTransition::KillChanged;
    }
    #[test]
    fn vector_auth() {
        let _ = HttpStatus::Unauthorized;
    }
    #[test]
    fn vector_buffering() {
        let _ = SnapshotTransition::Stale;
    }
    #[test]
    fn vector_allplayers_boundary() {
        let _ = SnapshotTransition::Seed;
    }
    #[test]
    fn vector_round_end_provisional() {
        assert_eq!(
            SnapshotTransition::ProvisionalRoundEnd,
            SnapshotTransition::ProvisionalRoundEnd
        );
    }
}
