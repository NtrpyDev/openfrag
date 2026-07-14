#![allow(clippy::missing_errors_doc, clippy::struct_excessive_bools)]

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const MAX_BODY_BYTES: usize = 128 * 1024;
pub const DUPLICATE_WINDOW: Duration = Duration::from_secs(2);

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceReceipt {
    pub sequence: u64,
    pub received_at: Duration,
    pub payload_hash: String,
    pub presence: PresenceBits,
    pub context: EvidenceContext,
    pub output: StateOutput,
    /// Facts co-observed in one snapshot transition; vector order is not event order.
    pub facts: Vec<TransitionFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceContext {
    pub map_hash: Option<String>,
    pub observed_round: Option<u64>,
    pub provider_timestamp: Option<i64>,
    pub round_kills: Option<i64>,
    pub health: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresenceBits {
    pub provider: bool,
    pub provider_timestamp: bool,
    pub map: bool,
    pub map_round: bool,
    pub player: bool,
    pub player_state: bool,
    pub player_health: bool,
    pub player_round_kills: bool,
    pub player_weapons: bool,
    pub match_stats: bool,
    pub auth: bool,
    pub auth_token: bool,
    pub player_steamid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TransitionFact {
    CumulativeKillDelta {
        previous: i64,
        current: i64,
        delta: i64,
    },
    CumulativeDeathDelta {
        previous: i64,
        current: i64,
        delta: i64,
    },
    RoundKillDelta {
        previous: i64,
        current: i64,
        delta: i64,
    },
    HealthDepleted {
        previous: i64,
        current: i64,
    },
    RoundEnd {
        completed: u64,
        next: u64,
    },
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
    pub fn new(config: GsiConfig, clock: Arc<dyn Clock>, sink: Arc<dyn EventSink>) -> Self {
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
        let (Some(payload_hash), Some(presence), Some(context)) = (
            guard.last_hash.clone(),
            guard.last_presence,
            guard.last_context.clone(),
        ) else {
            return Ok(PollOutcome::NoChange);
        };
        let receipt = EvidenceReceipt {
            sequence: guard.sequence + 1,
            received_at: now,
            payload_hash,
            presence,
            context,
            output: StateOutput::Stale,
            facts: Vec::new(),
        };
        self.sink
            .emit(receipt.clone())
            .map_err(|_| ServiceDiagnostic::SinkFailure)?;
        guard.sequence = receipt.sequence;
        guard.stale = true;
        guard.snapshot = None;
        guard.recent_hashes.clear();
        Ok(PollOutcome::Stale)
    }
}

#[must_use]
pub fn stale_after(heartbeat: Duration) -> Duration {
    heartbeat.saturating_mul(3).min(Duration::from_secs(90))
}

pub fn router(service: GsiService) -> Router {
    Router::new()
        .route("/gsi/router", post(route_post))
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
    if !token_matches {
        return (StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let player_matches = payload
        .player
        .as_ref()
        .and_then(|player| player.steamid.as_deref())
        .is_some_and(|steamid| digest(steamid.as_bytes()) == service.config.local_steamid_hash);
    if !player_matches {
        let Ok(mut guard) = service.engine.lock() else {
            return (StatusCode::INTERNAL_SERVER_ERROR, "state-unavailable");
        };
        guard.clear_transition_baselines();
        return (StatusCode::UNAUTHORIZED, "unauthorized");
    }

    if payload
        .provider
        .as_ref()
        .and_then(|provider| provider.appid)
        != Some(730)
    {
        return (StatusCode::BAD_REQUEST, "wrong-app");
    }

    let received_at = service.clock.now();
    let hash = hex_digest(&body);
    let Ok(mut guard) = service.engine.lock() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "state-unavailable");
    };
    guard
        .recent_hashes
        .retain(|_, seen_at| received_at.saturating_sub(*seen_at) <= DUPLICATE_WINDOW);
    if guard
        .recent_hashes
        .get(&hash)
        .is_some_and(|seen_at| received_at.saturating_sub(*seen_at) <= DUPLICATE_WINDOW)
    {
        return (StatusCode::OK, "duplicate");
    }

    let snapshot = TrustedSnapshot::from(&payload);
    let (output, facts) = if guard.stale {
        (StateOutput::Recovered, Vec::new())
    } else {
        match guard.snapshot.as_ref() {
            None => (StateOutput::Seeded, Vec::new()),
            Some(previous) => derive_transition(previous, &snapshot),
        }
    };
    let presence = PresenceBits::from(&payload);
    let context = EvidenceContext::from(&snapshot);
    let receipt = EvidenceReceipt {
        sequence: guard.sequence + 1,
        received_at,
        payload_hash: hash.clone(),
        presence,
        context: context.clone(),
        output,
        facts,
    };
    if service.sink.emit(receipt.clone()).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "sink-failure");
    }
    if output == StateOutput::SessionReset {
        guard.clear_transition_baselines();
    }
    guard.sequence = receipt.sequence;
    guard.recent_hashes.insert(hash.clone(), received_at);
    guard.last_hash = Some(hash);
    guard.last_presence = Some(presence);
    guard.last_context = Some(context);
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
    weapons: Option<serde_json::Value>,
    match_stats: Option<MatchStats>,
}

#[derive(Deserialize)]
struct PlayerVitals {
    health: Option<i64>,
    round_kills: Option<i64>,
}

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
            player_health: payload
                .player
                .as_ref()
                .and_then(|player| player.state.as_ref())
                .is_some_and(|state| state.health.is_some()),
            player_round_kills: payload
                .player
                .as_ref()
                .and_then(|player| player.state.as_ref())
                .is_some_and(|state| state.round_kills.is_some()),
            player_weapons: payload
                .player
                .as_ref()
                .is_some_and(|player| player.weapons.is_some()),
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
    recent_hashes: HashMap<String, Duration>,
    last_hash: Option<String>,
    last_presence: Option<PresenceBits>,
    last_context: Option<EvidenceContext>,
    last_received_at: Option<Duration>,
    stale: bool,
}

impl EngineState {
    fn clear_transition_baselines(&mut self) {
        self.snapshot = None;
        self.recent_hashes.clear();
        self.last_hash = None;
        self.last_presence = None;
        self.last_context = None;
        self.last_received_at = None;
        self.stale = false;
    }
}

#[derive(Debug, Clone)]
struct TrustedSnapshot {
    session_hash: Option<[u8; 32]>,
    map_hash: Option<[u8; 32]>,
    round: Option<u64>,
    provider_timestamp: Option<i64>,
    kills: Option<i64>,
    deaths: Option<i64>,
    round_kills: Option<i64>,
    health: Option<i64>,
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
        let map_hash = payload
            .map
            .as_ref()
            .and_then(|map| map.name.as_deref())
            .map(|name| digest(name.as_bytes()));
        let stats = payload
            .player
            .as_ref()
            .and_then(|player| player.match_stats.as_ref());
        Self {
            session_hash,
            map_hash,
            round: payload.map.as_ref().and_then(|map| map.round),
            provider_timestamp: payload
                .provider
                .as_ref()
                .and_then(|provider| provider.timestamp),
            kills: stats.and_then(|stats| stats.kills),
            deaths: stats.and_then(|stats| stats.deaths),
            round_kills: payload
                .player
                .as_ref()
                .and_then(|player| player.state.as_ref())
                .and_then(|state| state.round_kills),
            health: payload
                .player
                .as_ref()
                .and_then(|player| player.state.as_ref())
                .and_then(|state| state.health),
        }
    }
}

impl From<&TrustedSnapshot> for EvidenceContext {
    fn from(snapshot: &TrustedSnapshot) -> Self {
        Self {
            map_hash: snapshot.map_hash.map(hex_array),
            observed_round: snapshot.round,
            provider_timestamp: snapshot.provider_timestamp,
            round_kills: snapshot.round_kills,
            health: snapshot.health,
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
        facts.push(TransitionFact::CumulativeKillDelta {
            previous,
            current,
            delta: current - previous,
        });
    }
    if let (Some(previous), Some(current)) = (previous.deaths, current.deaths)
        && previous >= 0
        && current > previous
    {
        facts.push(TransitionFact::CumulativeDeathDelta {
            previous,
            current,
            delta: current - previous,
        });
    }
    let same_round = matches!((previous.round, current.round), (Some(previous), Some(current)) if previous == current);
    if same_round
        && let (Some(previous), Some(current)) = (previous.round_kills, current.round_kills)
        && previous >= 0
        && current > previous
    {
        facts.push(TransitionFact::RoundKillDelta {
            previous,
            current,
            delta: current - previous,
        });
    }
    if same_round
        && let (Some(previous), Some(current)) = (previous.health, current.health)
        && previous > 0
        && current <= 0
    {
        facts.push(TransitionFact::HealthDepleted { previous, current });
    }
    if let (Some(previous), Some(current)) = (previous.round, current.round) {
        for completed in previous..current {
            facts.push(TransitionFact::RoundEnd {
                completed,
                next: completed + 1,
            });
        }
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
        || decreased(previous.provider_timestamp, current.provider_timestamp)
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

fn hex_array(bytes: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
