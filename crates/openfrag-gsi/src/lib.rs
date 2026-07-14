use serde::Deserialize;
use std::collections::HashSet;

pub const MAX_BODY_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError { TooLarge, InvalidJson, WrongApp }

#[derive(Debug, Clone, Deserialize)]
pub struct Payload { pub provider: Option<Provider>, pub map: Option<MapState>, pub player: Option<PlayerState>, pub auth: Option<Auth> }
#[derive(Debug, Clone, Deserialize)] pub struct Provider { pub appid: Option<u64>, pub timestamp: Option<String>, pub steamid: Option<String> }
#[derive(Debug, Clone, Deserialize)] pub struct MapState { pub name: Option<String>, pub mode: Option<String>, pub phase: Option<String>, pub round: Option<u64> }
#[derive(Debug, Clone, Deserialize)] pub struct PlayerState { pub activity: Option<String>, pub state: Option<PlayerHealth> }
#[derive(Debug, Clone, Deserialize)] pub struct PlayerHealth { pub health: Option<i64>, pub armor: Option<i64> }
#[derive(Debug, Clone, Deserialize)] pub struct Auth { pub token: Option<String> }

#[derive(Debug, Clone, PartialEq, Eq)] pub enum Evidence { Provisional, Confirmed }
#[derive(Debug, Clone)] pub struct Receipt { pub sequence: u64, pub payload: Payload, pub redacted: String, pub evidence: Evidence }
#[derive(Debug, Default)] pub struct IngestState { seen: HashSet<String>, next: u64, pub seed: Option<Receipt> }

pub fn ingest(state: &mut IngestState, body: &[u8]) -> Result<Option<Receipt>, IngestError> {
    if body.len() > MAX_BODY_BYTES { return Err(IngestError::TooLarge); }
    let payload: Payload = serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if payload.provider.as_ref().and_then(|p| p.appid) != Some(730) { return Err(IngestError::WrongApp); }
    let fingerprint = payload.provider.as_ref().and_then(|p| p.timestamp.clone()).unwrap_or_default();
    if !state.seen.insert(fingerprint) { return Ok(None); }
    state.next += 1;
    let mut value: serde_json::Value = serde_json::from_slice(body).map_err(|_| IngestError::InvalidJson)?;
    if let Some(obj) = value.as_object_mut() { obj.remove("auth"); if let Some(provider) = obj.get_mut("provider").and_then(|v| v.as_object_mut()) { provider.remove("steamid"); } }
    let receipt = Receipt { sequence: state.next, payload, redacted: value.to_string(), evidence: Evidence::Provisional };
    if state.seed.is_none() { state.seed = Some(receipt.clone()); }
    Ok(Some(receipt))
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test] fn cap_and_duplicate_and_redaction() { let mut s=IngestState::default(); let b=br#"{"provider":{"appid":730,"timestamp":"1","steamid":"secret"},"auth":{"token":"secret"}}"#; let r=ingest(&mut s,b).unwrap().unwrap(); assert!(!r.redacted.contains("secret")); assert!(ingest(&mut s,b).unwrap().is_none()); assert_eq!(ingest(&mut s,&vec![b'x';MAX_BODY_BYTES+1]),Err(IngestError::TooLarge)); }
}
