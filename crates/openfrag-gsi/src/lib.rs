use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub const MAX_BODY_BYTES: usize = 128 * 1024;

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceReceipt {
    private: (),
}

#[derive(Debug, Clone)]
pub struct GsiConfig {
    auth_token_hash: [u8; 32],
    local_steamid_hash: [u8; 32],
    heartbeat: Duration,
}

impl GsiConfig {
    #[must_use]
    pub fn new(auth_token: &str, local_steamid: &str, heartbeat: Duration) -> Self {
        Self {
            auth_token_hash: digest(auth_token.as_bytes()),
            local_steamid_hash: digest(local_steamid.as_bytes()),
            heartbeat,
        }
    }
}

#[derive(Clone)]
pub struct GsiService {
    config: GsiConfig,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn EventSink>,
}

impl GsiService {
    #[must_use]
    pub fn new(
        config: GsiConfig,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            config,
            clock,
            sink,
        }
    }
}

#[must_use]
pub fn router(service: GsiService) -> Router {
    Router::new()
        .route("/gsi", post(route_post))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(service)
}

async fn route_post(
    State(service): State<GsiService>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    if headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "wrong-content-type");
    }

    let Ok(payload) = serde_json::from_slice::<IdentityEnvelope>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid-json");
    };
    let token_matches = payload
        .auth
        .and_then(|auth| auth.token)
        .is_some_and(|token| digest(token.as_bytes()) == service.config.auth_token_hash);
    let player_matches = payload
        .player
        .and_then(|player| player.steamid)
        .is_some_and(|steamid| digest(steamid.as_bytes()) == service.config.local_steamid_hash);
    if !token_matches || !player_matches {
        return (StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let _ = (&service.clock, &service.sink, service.config.heartbeat);
    (StatusCode::OK, "accepted")
}

#[derive(Deserialize)]
struct IdentityEnvelope {
    auth: Option<Auth>,
    player: Option<PlayerIdentity>,
}

#[derive(Deserialize)]
struct Auth {
    token: Option<String>,
}

#[derive(Deserialize)]
struct PlayerIdentity {
    steamid: Option<String>,
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
