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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    NoChange,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceDiagnostic {
    StateUnavailable,
    SinkFailure,
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

    /// Polls the deterministic clock and emits a single stale receipt per outage.
    ///
    /// # Errors
    ///
    /// Returns an explicit diagnostic if state cannot be locked or evidence cannot be emitted.
    pub fn poll_stale(&self) -> Result<PollOutcome, ServiceDiagnostic> {
        let now = self.clock.now();
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| ServiceDiagnostic::StateUnavailable)?;
        let Some(last_received_at) = guard.last_received_at else {
            return Ok(PollOutcome::NoChange);
        };
        if guard.stale || now.saturating_sub(last_received_at) < stale_after(self.config.heartbeat)
        {
            return Ok(PollOutcome::NoChange);
        }
        let (Some(payload_hash), Some(presence)) =
            (guard.last_hash.clone(), guard.last_presence)
        else {
            return Ok(PollOutcome::NoChange);
        };
        let receipt = EvidenceReceipt {
            sequence: guard.sequence + 1,
            received_at: now,
            payload_hash,
            presence,
            output: StateOutput::Stale,
            facts: Vec::new(),
        };
        self.sink
            .emit(receipt.clone())
            .map_err(|_| ServiceDiagnostic::SinkFailure)?;
        guard.sequence = receipt.sequence;
        guard.stale = true;
        Ok(PollOutcome::Stale)
    }
}

#[must_use]
pub fn stale_after(heartbeat: Duration) -> Duration {
    heartbeat.saturating_mul(3).min(Duration::from_secs(90))
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

    let snapshot = TrustedSnapshot::from(&payload);
    let (output, facts) = if guard.snapshot.is_none() {
        (StateOutput::Seeded, Vec::new())
    } else if guard.stale {
        (StateOutput::Recovered, Vec::new())
    } else {
        derive_transition(
            guard.snapshot.as_ref().expect("snapshot checked above"),
            &snapshot,
        )
    };
    let presence = PresenceBits::from(&payload);
    let receipt = EvidenceReceipt {
        sequence: guard.sequence + 1,
        received_at,
        payload_hash: hash.clone(),
        presence,
        output,
        facts,
    };
    if service.sink.emit(receipt.clone()).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "sink-failure");
    }
    guard.sequence = receipt.sequence;
    guard.last_hash = Some(hash);
    guard.last_presence = Some(presence);
    guard.last_received_at = Some(received_at);
    guard.snapshot = Some(snapshot);
    guard.stale = false;
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
struct PlayerVitals {}

#[derive(Deserialize)]
struct MatchStats {
    kills: Option<i64>,
    deaths: Option<i64>,
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
    last_presence: Option<PresenceBits>,
    last_received_at: Option<Duration>,
    stale: bool,
}

#[derive(Debug, Clone)]
struct TrustedSnapshot {
    session_hash: Option<[u8; 32]>,
    round: Option<u64>,
    kills: Option<i64>,
    deaths: Option<i64>,
}

impl From<&Payload> for TrustedSnapshot {
    fn from(payload: &Payload) -> Self {
        let session_hash = payload.map.as_ref().and_then(|map| {
            if map.name.is_none() && map.mode.is_none() {
                return None;
            }
            let mut session = Sha256::new();
            if let Some(name) = &map.name {
                session.update(name.as_bytes());
            }
            session.update([0]);
            if let Some(mode) = &map.mode {
                session.update(mode.as_bytes());
            }
            Some(session.finalize().into())
        });
        let stats = payload
            .player
            .as_ref()
            .and_then(|player| player.match_stats.as_ref());
        Self {
            session_hash,
            round: payload.map.as_ref().and_then(|map| map.round),
            kills: stats.and_then(|stats| stats.kills),
            deaths: stats.and_then(|stats| stats.deaths),
        }
    }
}

fn derive_transition(
    previous: &TrustedSnapshot,
    current: &TrustedSnapshot,
) -> (StateOutput, Vec<TransitionFact>) {
    if session_reset(previous, current) {
        return (StateOutput::SessionReset, Vec::new());
    }

    let mut facts = Vec::new();
    if let (Some(previous), Some(current)) = (previous.kills, current.kills)
        && previous >= 0
        && current > previous
    {
        facts.push(TransitionFact::Kill { previous, current });
    }
    if let (Some(previous), Some(current)) = (previous.deaths, current.deaths)
        && previous >= 0
        && current > previous
    {
        facts.push(TransitionFact::Death { previous, current });
    }
    if let (Some(completed), Some(next)) = (previous.round, current.round)
        && next > completed
    {
        facts.push(TransitionFact::RoundEnd { completed, next });
    }
    (StateOutput::Healthy, facts)
}

fn session_reset(previous: &TrustedSnapshot, current: &TrustedSnapshot) -> bool {
    matches!(
        (previous.session_hash, current.session_hash),
        (Some(previous), Some(current)) if previous != current
    ) || decreased(previous.round, current.round)
        || decreased(previous.kills, current.kills)
        || decreased(previous.deaths, current.deaths)
}

fn decreased<T: PartialOrd>(previous: Option<T>, current: Option<T>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if current < previous)
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
