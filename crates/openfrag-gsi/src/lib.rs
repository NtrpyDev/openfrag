use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub const MAX_BODY_BYTES: usize = 128 * 1024;

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceReceipt {
    pub sequence: u64,
    pub received_at: Duration,
    pub payload_hash: String,
    pub presence: PresenceBits,
    pub output: StateOutput,
    pub facts: Vec<TransitionFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceBits {
    pub provider: bool,
    pub provider_timestamp: bool,
    pub map: bool,
    pub map_round: bool,
    pub player: bool,
    pub player_state: bool,
    pub match_stats: bool,
    pub auth: bool,
    pub auth_token: bool,
    pub player_steamid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOutput {
    Seeded,
    Healthy,
    Stale,
    Recovered,
    SessionReset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionFact {
    Kill { previous: i64, current: i64 },
    Death { previous: i64, current: i64 },
    RoundEnd { completed: u64, next: u64 },
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
    engine: Arc<Mutex<EngineState>>,
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
            engine: Arc::new(Mutex::new(EngineState::default())),
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
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .is_none_or(|media_type| !media_type.eq_ignore_ascii_case("application/json"))
    {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "wrong-content-type");
    }

    let Ok(payload) = serde_json::from_slice::<Payload>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid-json");
    };
    let token_matches = payload
        .auth
        .as_ref()
        .and_then(|auth| auth.token.as_deref())
        .is_some_and(|token| digest(token.as_bytes()) == service.config.auth_token_hash);
    let player_matches = payload
        .player
        .as_ref()
        .and_then(|player| player.steamid.as_deref())
        .is_some_and(|steamid| digest(steamid.as_bytes()) == service.config.local_steamid_hash);
    if !token_matches || !player_matches {
        return (StatusCode::UNAUTHORIZED, "unauthorized");
    }

    if payload.provider.as_ref().and_then(|provider| provider.appid) != Some(730) {
        return (StatusCode::BAD_REQUEST, "wrong-app");
    }

    let received_at = service.clock.now();
    let hash = hex_digest(&body);
    let mut guard = match service.engine.lock() {
        Ok(guard) => guard,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "state-unavailable"),
    };
    if guard.last_hash.as_deref() == Some(hash.as_str()) {
        return (StatusCode::OK, "duplicate");
    }

    let receipt = EvidenceReceipt {
        sequence: guard.sequence + 1,
        received_at,
        payload_hash: hash.clone(),
        presence: PresenceBits::from(&payload),
        output: StateOutput::Seeded,
        facts: Vec::new(),
    };
    if service.sink.emit(receipt.clone()).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "sink-failure");
    }
    guard.sequence = receipt.sequence;
    guard.last_hash = Some(hash);
    guard.last_received_at = Some(received_at);
    guard.snapshot = Some(TrustedSnapshot::from(&payload));
    let _ = service.config.heartbeat;
    (StatusCode::OK, "seeded")
}

#[derive(Deserialize)]
struct Payload {
    provider: Option<Provider>,
    map: Option<MapState>,
    auth: Option<Auth>,
    player: Option<PlayerState>,
}

#[derive(Deserialize)]
struct Provider {
    appid: Option<u64>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct MapState {
    name: Option<String>,
    mode: Option<String>,
    round: Option<u64>,
}

#[derive(Deserialize)]
struct Auth {
    token: Option<String>,
}

#[derive(Deserialize)]
struct PlayerState {
    steamid: Option<String>,
    state: Option<PlayerVitals>,
    match_stats: Option<MatchStats>,
}

#[derive(Deserialize)]
struct PlayerVitals {
    health: Option<i64>,
}

#[derive(Deserialize)]
struct MatchStats {
    kills: Option<i64>,
    deaths: Option<i64>,
    round_kills: Option<i64>,
}

impl From<&Payload> for PresenceBits {
    fn from(payload: &Payload) -> Self {
        Self {
            provider: payload.provider.is_some(),
            provider_timestamp: payload
                .provider
                .as_ref()
                .is_some_and(|provider| provider.timestamp.is_some()),
            map: payload.map.is_some(),
            map_round: payload.map.as_ref().is_some_and(|map| map.round.is_some()),
            player: payload.player.is_some(),
            player_state: payload
                .player
                .as_ref()
                .is_some_and(|player| player.state.is_some()),
            match_stats: payload
                .player
                .as_ref()
                .is_some_and(|player| player.match_stats.is_some()),
            auth: payload.auth.is_some(),
            auth_token: payload
                .auth
                .as_ref()
                .is_some_and(|auth| auth.token.is_some()),
            player_steamid: payload
                .player
                .as_ref()
                .is_some_and(|player| player.steamid.is_some()),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EngineState {
    sequence: u64,
    snapshot: Option<TrustedSnapshot>,
    last_hash: Option<String>,
    last_received_at: Option<Duration>,
}

#[derive(Debug, Clone)]
struct TrustedSnapshot {
    session_hash: [u8; 32],
    round: Option<u64>,
    kills: Option<i64>,
    deaths: Option<i64>,
    round_kills: Option<i64>,
    health: Option<i64>,
}

impl From<&Payload> for TrustedSnapshot {
    fn from(payload: &Payload) -> Self {
        let mut session = Sha256::new();
        if let Some(map) = &payload.map {
            if let Some(name) = &map.name {
                session.update(name.as_bytes());
            }
            session.update([0]);
            if let Some(mode) = &map.mode {
                session.update(mode.as_bytes());
            }
        }
        let stats = payload
            .player
            .as_ref()
            .and_then(|player| player.match_stats.as_ref());
        Self {
            session_hash: session.finalize().into(),
            round: payload.map.as_ref().and_then(|map| map.round),
            kills: stats.and_then(|stats| stats.kills),
            deaths: stats.and_then(|stats| stats.deaths),
            round_kills: stats.and_then(|stats| stats.round_kills),
            health: payload
                .player
                .as_ref()
                .and_then(|player| player.state.as_ref())
                .and_then(|state| state.health),
        }
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
