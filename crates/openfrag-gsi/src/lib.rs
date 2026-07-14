use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::Instant;

pub const MAX_BODY_BYTES: usize = 128 * 1024;

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
    hashes: HashSet<String>,
    started: Option<Instant>,
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
    if payload.provider.as_ref().and_then(|p| p.steamid.as_deref())
        != Some(config.local_steamid.as_str())
    {
        return Err(IngestError::WrongApp);
    }
    if payload.auth.as_ref().and_then(|a| a.token.as_deref()) != Some(config.auth_token.as_str()) {
        return Err(IngestError::WrongApp);
    }
    if !state.hashes.insert(hash.clone()) {
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
        let b=br#"{"provider":{"appid":730,"timestamp":"1","steamid":"secret"},"auth":{"token":"secret"}}"#;
        let r = ingest(&mut s, b).unwrap().unwrap();
        assert!(!r.redacted.contains("secret"));
        assert!(ingest(&mut s, b).unwrap().is_none());
        assert_eq!(
            ingest(&mut s, &vec![b'x'; MAX_BODY_BYTES + 1]),
            Err(IngestError::TooLarge)
        );
    }
}
