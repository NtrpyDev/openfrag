//! PROTOTYPE ONLY. Portable state reduction for captured CS2 GSI snapshots.

use serde_json::Value;

#[derive(Debug, Clone, Copy, Default)]
pub enum IdentityRelation {
    #[default]
    Unknown,
    Matched,
    Mismatch,
}

impl std::fmt::Display for IdentityRelation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unknown => "unknown",
            Self::Matched => "matched",
            Self::Mismatch => "mismatch",
        })
    }
}

#[derive(Debug, Default)]
pub struct Model {
    pub received: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub receive_monotonic_ms: Option<u128>,
    pub provider_timestamp: Option<String>,
    pub provider_steamid: Option<String>,
    pub player_steamid: Option<String>,
    pub map: Option<String>,
    pub mode: Option<String>,
    pub phase: Option<String>,
    pub round: Option<String>,
    pub observed_kills: Option<String>,
    pub observed_assists: Option<String>,
    pub observed_round_kills: Option<String>,
    pub saw_previously: bool,
    pub saw_added: bool,
    pub payload_hash: Option<String>,
    pub duplicate: bool,
    pub identity_relation: IdentityRelation,
    pub capture_warning: Option<String>,
    pub request_warning: Option<String>,
}

impl Model {
    pub fn reject(&mut self) {
        self.received += 1;
        self.rejected += 1;
    }

    pub fn observe(&mut self, received_at_ms: u128, payload: &Value, hash: String) {
        self.received += 1;
        self.accepted += 1;
        self.duplicate = self.payload_hash.as_deref() == Some(hash.as_str());
        self.payload_hash = Some(hash);
        self.receive_monotonic_ms = Some(received_at_ms);
        self.provider_timestamp = string_at(payload, "/provider/timestamp");
        self.provider_steamid = string_at(payload, "/provider/steamid");
        self.player_steamid = string_at(payload, "/player/steamid");
        self.map = string_at(payload, "/map/name");
        self.mode = string_at(payload, "/map/mode");
        self.phase =
            string_at(payload, "/round/phase").or_else(|| string_at(payload, "/map/phase"));
        self.round = string_at(payload, "/map/round");
        self.observed_kills = string_at(payload, "/player/match_stats/kills");
        self.observed_assists = string_at(payload, "/player/match_stats/assists");
        self.observed_round_kills = string_at(payload, "/player/state/round_kills");
        self.saw_previously = payload.get("previously").is_some();
        self.saw_added = payload.get("added").is_some();
        self.identity_relation = match (&self.provider_steamid, &self.player_steamid) {
            (Some(provider), Some(player)) if provider == player => IdentityRelation::Matched,
            (Some(_), Some(_)) => IdentityRelation::Mismatch,
            _ => IdentityRelation::Unknown,
        };
    }

    pub fn capture_failed(&mut self, error: String) {
        self.capture_warning = Some(error);
    }

    pub fn capture_succeeded(&mut self) {
        self.capture_warning = None;
    }

    pub fn request_failed(&mut self, error: String) {
        self.request_warning = Some(error);
    }

    pub fn warning(&self, now_ms: u128) -> Option<String> {
        if self.duplicate {
            return Some("duplicate payload hash".into());
        }
        if let Some(warning) = &self.capture_warning {
            return Some(format!("capture unavailable: {warning}"));
        }
        if let Some(warning) = &self.request_warning {
            return Some(format!("request handling error: {warning}"));
        }
        if matches!(self.identity_relation, IdentityRelation::Mismatch) {
            return Some("observed player identity differs from provider".into());
        }
        match self.receive_monotonic_ms {
            None => Some("waiting for an accepted POST".into()),
            Some(last) if now_ms.saturating_sub(last) > 45_000 => {
                Some("no accepted POST for over 45 seconds".into())
            }
            _ => None,
        }
    }
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).map(|value| match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => "<non-scalar>".into(),
    })
}
