//! Durable local Demo import primitives. This crate owns orchestration, not parser output.
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )
)]

use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const MAX_DEMO_BYTES: u64 = 2_147_483_648;
pub const CHUNK_BYTES: usize = 8 * 1024 * 1024;
const DEMO_MAGIC: &[u8] = b"PBDEMS2\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    AwaitingImport,
    Validating,
    Hashing,
    Deduplicating,
    Copying,
    Parsing,
    Rating,
    LinkingClips,
    Ready,
    Error(ErrorCode),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Io,
    Corrupt,
    Unsupported,
    Size,
    Parse,
    Analysis,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub processed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub indeterminate: bool,
    pub work_done: u64,
    pub heartbeat: u64,
}
impl Progress {
    pub fn fraction_millionths(&self) -> Option<u64> {
        self.total_bytes
            .filter(|&t| t > 0)
            .map(|t| (self.processed_bytes.min(t) * 1_000_000) / t)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParserProgressPhase {
    FirstPass,
    SecondPass,
    Finalize,
}

impl ParserProgressPhase {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::FirstPass => "first_pass",
            Self::SecondPass => "second_pass",
            Self::Finalize => "finalize",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParserProgress {
    pub phase: ParserProgressPhase,
    pub bytes_consumed: u64,
    pub total_bytes: u64,
    pub frames: u64,
    pub events_emitted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculationIdentity {
    pub source_sha256: String,
    pub parser_commit: String,
    pub parser_build: String,
    pub generated_proto_build: String,
    pub requested_schema_hash: String,
    pub metric_definition_version: String,
    pub formula_version: String,
    pub evidence_semantics_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: u64,
    pub state: State,
    pub retries: u8,
    pub next_retry_after: Option<Duration>,
    pub progress: Progress,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<SystemTime>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DedupDecision {
    Copy,
    Ready,
    Resume,
    Parse,
    Rate,
    PriorFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParserCapability {
    Unavailable {
        reason: String,
    },
    Pinned {
        commit: String,
        build: String,
        schema_hash: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoMetadata {
    pub map: Option<String>,
    pub patch_build: Option<String>,
    pub demo_stamp: Option<String>,
    pub server: Option<String>,
    pub game_directory: Option<String>,
    pub tick_rate: Option<String>,
    pub tick_rate_unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    pub steam_id: SteamId,
    pub name: Option<String>,
    pub team: Option<i32>,
}

macro_rules! identifier_type {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($inner);

        impl $name {
            #[must_use]
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> $inner {
                self.0
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }
    };
}

identifier_type!(DemoTick, i32);
identifier_type!(IngestionOrdinal, u64);
identifier_type!(SteamId, u64);
identifier_type!(EntityId, i32);
identifier_type!(RoundNumber, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizedEventKind {
    RoundFreezeEnd,
    RoundEnd,
    PlayerHurt,
    PlayerDeath,
    PlayerDisconnect,
}

impl NormalizedEventKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RoundFreezeEnd => "round_freeze_end",
            Self::RoundEnd => "round_end",
            Self::PlayerHurt => "player_hurt",
            Self::PlayerDeath => "player_death",
            Self::PlayerDisconnect => "player_disconnect",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundFreezeEndEvent {
    pub warmup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundEndEvent {
    pub winner: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerHurtEvent {
    pub attacker: Option<SteamId>,
    pub victim: Option<SteamId>,
    pub damage_health: Option<i64>,
    pub weapon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerDeathEvent {
    pub attacker: Option<SteamId>,
    pub victim: Option<SteamId>,
    pub assister: Option<SteamId>,
    pub assisted_flash: bool,
    pub weapon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerDisconnectEvent {
    pub player: Option<SteamId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NormalizedEvent {
    RoundFreezeEnd(RoundFreezeEndEvent),
    RoundEnd(RoundEndEvent),
    PlayerHurt(PlayerHurtEvent),
    PlayerDeath(PlayerDeathEvent),
    PlayerDisconnect(PlayerDisconnectEvent),
}

impl NormalizedEvent {
    #[must_use]
    pub const fn kind(&self) -> NormalizedEventKind {
        match self {
            Self::RoundFreezeEnd(_) => NormalizedEventKind::RoundFreezeEnd,
            Self::RoundEnd(_) => NormalizedEventKind::RoundEnd,
            Self::PlayerHurt(_) => NormalizedEventKind::PlayerHurt,
            Self::PlayerDeath(_) => NormalizedEventKind::PlayerDeath,
            Self::PlayerDisconnect(_) => NormalizedEventKind::PlayerDisconnect,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEvent {
    pub tick: DemoTick,
    pub ingestion_ordinal: IngestionOrdinal,
    pub event: NormalizedEvent,
}
impl ParsedEvent {
    #[must_use]
    pub fn from_raw(
        name: &str,
        tick: i32,
        ingestion_ordinal: u64,
        fields: &BTreeMap<String, serde_json::Value>,
    ) -> Option<Self> {
        Some(Self {
            tick: DemoTick::new(tick),
            ingestion_ordinal: IngestionOrdinal::new(ingestion_ordinal),
            event: normalize_event(name, fields)?,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> NormalizedEventKind {
        self.event.kind()
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.kind().name()
    }

    #[must_use]
    pub const fn attacker(&self) -> Option<SteamId> {
        match &self.event {
            NormalizedEvent::PlayerHurt(event) => event.attacker,
            NormalizedEvent::PlayerDeath(event) => event.attacker,
            _ => None,
        }
    }

    #[must_use]
    pub const fn victim(&self) -> Option<SteamId> {
        match &self.event {
            NormalizedEvent::PlayerHurt(event) => event.victim,
            NormalizedEvent::PlayerDeath(event) => event.victim,
            _ => None,
        }
    }

    #[must_use]
    pub fn weapon(&self) -> Option<&str> {
        match &self.event {
            NormalizedEvent::PlayerHurt(event) => event.weapon.as_deref(),
            NormalizedEvent::PlayerDeath(event) => event.weapon.as_deref(),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReceipt {
    pub ingestion_ordinal: IngestionOrdinal,
    pub event_name: String,
    pub tick: DemoTick,
    pub fields: BTreeMap<String, String>,
    pub raw_fields: BTreeMap<String, serde_json::Value>,
    pub evidence_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRound {
    pub number: RoundNumber,
    pub end_tick: DemoTick,
    pub winner: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotPhase {
    /// Synthetic or explicitly requested state at a tick, with no event-order claim.
    RequestedTick,
    /// State after all net messages in a packet that emitted a requested event.
    AfterEventPacket,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub tick: DemoTick,
    pub ingestion_ordinal: IngestionOrdinal,
    pub phase: SnapshotPhase,
    pub steam_id: SteamId,
    pub entity_id: Option<EntityId>,
    pub team: Option<i32>,
    pub health: Option<i32>,
    pub alive: Option<bool>,
    pub life_state: Option<i32>,
    pub round_counter: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedOutput {
    pub metadata: DemoMetadata,
    pub participants: Vec<Participant>,
    pub rounds: Vec<ParsedRound>,
    pub events: Vec<ParsedEvent>,
    pub receipts: Vec<EventReceipt>,
    pub player_snapshots: Vec<PlayerSnapshot>,
    pub suspicious_empty: bool,
    pub identity: CalculationIdentity,
}
impl ParsedOutput {
    pub fn round_count(&self) -> usize {
        self.rounds.len()
    }
    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

pub const DEMOPARSER_COMMIT: &str = "ba39cc44cd5abfd7f34df2b3c0a7dd3630048311";
pub const DEMOPARSER_BUILD: &str =
    "parser-0.1.1+openfrag-typed-evidence-diagnostics-1/csgoproto-0.1.5";
pub const QUERY_PLAN_VERSION: &str = "openfrag-evidence-query-4";
pub const NORMALIZED_SCHEMA_VERSION: &str = "openfrag-demo-evidence-1";
pub const EVENT_QUERY: &[&str] = &[
    "round_freeze_end",
    "round_end",
    "player_hurt",
    "player_death",
    "player_disconnect",
];
pub const PROPERTY_QUERY: &[&str] = &[
    "player_steamid",
    "team_num",
    "is_alive",
    "life_state",
    "health",
    "total_rounds_played",
    "tick_rate",
    "weapon",
    "attacker",
    "victim",
    "assister",
    "assistedflash",
    "dmg_health",
    "winner",
    "warmup_period",
];
pub const SNAPSHOT_PHASE: &str = "after-event-packet";
pub const EVIDENCE_SEMANTICS_EPOCH: &str = "openfrag-evidence-3";
pub const GENERATED_PROTO_BUILD: &str = "csgoproto-0.1.5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaScalar {
    Boolean,
    Integer,
    String,
    SteamId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizedFieldSpec {
    pub name: &'static str,
    pub scalar: SchemaScalar,
    pub required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizedEventSpec {
    pub kind: NormalizedEventKind,
    pub fields: &'static [NormalizedFieldSpec],
}

const ROUND_FREEZE_END_FIELDS: &[NormalizedFieldSpec] = &[NormalizedFieldSpec {
    name: "warmup",
    scalar: SchemaScalar::Boolean,
    required: true,
}];
const ROUND_END_FIELDS: &[NormalizedFieldSpec] = &[NormalizedFieldSpec {
    name: "winner",
    scalar: SchemaScalar::Integer,
    required: false,
}];
const PLAYER_HURT_FIELDS: &[NormalizedFieldSpec] = &[
    NormalizedFieldSpec {
        name: "attacker",
        scalar: SchemaScalar::SteamId,
        required: false,
    },
    NormalizedFieldSpec {
        name: "victim",
        scalar: SchemaScalar::SteamId,
        required: false,
    },
    NormalizedFieldSpec {
        name: "damage_health",
        scalar: SchemaScalar::Integer,
        required: false,
    },
    NormalizedFieldSpec {
        name: "weapon",
        scalar: SchemaScalar::String,
        required: false,
    },
];
const PLAYER_DEATH_FIELDS: &[NormalizedFieldSpec] = &[
    NormalizedFieldSpec {
        name: "attacker",
        scalar: SchemaScalar::SteamId,
        required: false,
    },
    NormalizedFieldSpec {
        name: "victim",
        scalar: SchemaScalar::SteamId,
        required: false,
    },
    NormalizedFieldSpec {
        name: "assister",
        scalar: SchemaScalar::SteamId,
        required: false,
    },
    NormalizedFieldSpec {
        name: "assisted_flash",
        scalar: SchemaScalar::Boolean,
        required: true,
    },
    NormalizedFieldSpec {
        name: "weapon",
        scalar: SchemaScalar::String,
        required: false,
    },
];
const PLAYER_DISCONNECT_FIELDS: &[NormalizedFieldSpec] = &[NormalizedFieldSpec {
    name: "player",
    scalar: SchemaScalar::SteamId,
    required: false,
}];

pub const NORMALIZED_EVENT_SPEC: &[NormalizedEventSpec] = &[
    NormalizedEventSpec {
        kind: NormalizedEventKind::RoundFreezeEnd,
        fields: ROUND_FREEZE_END_FIELDS,
    },
    NormalizedEventSpec {
        kind: NormalizedEventKind::RoundEnd,
        fields: ROUND_END_FIELDS,
    },
    NormalizedEventSpec {
        kind: NormalizedEventKind::PlayerHurt,
        fields: PLAYER_HURT_FIELDS,
    },
    NormalizedEventSpec {
        kind: NormalizedEventKind::PlayerDeath,
        fields: PLAYER_DEATH_FIELDS,
    },
    NormalizedEventSpec {
        kind: NormalizedEventKind::PlayerDisconnect,
        fields: PLAYER_DISCONNECT_FIELDS,
    },
];

/// Generates the public normalized evidence schema from the pinned event specification.
#[must_use]
pub fn normalized_event_schema() -> serde_json::Value {
    let variants = NORMALIZED_EVENT_SPEC
        .iter()
        .map(|event| {
            let properties = event
                .fields
                .iter()
                .map(|field| {
                    let schema = match field.scalar {
                        SchemaScalar::Boolean => serde_json::json!({"type": "boolean"}),
                        SchemaScalar::Integer => serde_json::json!({"type": "integer"}),
                        SchemaScalar::String => serde_json::json!({"type": "string"}),
                        SchemaScalar::SteamId => serde_json::json!({
                            "type": "integer",
                            "minimum": 0,
                            "maximum": u64::MAX,
                            "x-openfrag-type": "SteamId"
                        }),
                    };
                    (field.name.to_owned(), schema)
                })
                .collect::<serde_json::Map<_, _>>();
            let required = event
                .fields
                .iter()
                .filter(|field| field.required)
                .map(|field| field.name)
                .collect::<Vec<_>>();
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "type": {"const": event.kind.name()},
                    "tick": {"type": "integer", "x-openfrag-type": "DemoTick"},
                    "ingestion_ordinal": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": u64::MAX,
                        "x-openfrag-type": "IngestionOrdinal"
                    },
                    "data": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": properties,
                        "required": required
                    }
                },
                "required": ["type", "tick", "ingestion_ordinal", "data"]
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": NORMALIZED_SCHEMA_VERSION,
        "title": "OpenFrag normalized Demo evidence",
        "oneOf": variants
    })
}

pub fn query_plan_hash() -> String {
    let canonical = format!(
        "{}\nevents:{}\nproperties:{}\nsnapshot-phase:{}\nnormalized-schema:{}",
        QUERY_PLAN_VERSION,
        EVENT_QUERY.join(","),
        PROPERTY_QUERY.join(","),
        SNAPSHOT_PHASE,
        NORMALIZED_SCHEMA_VERSION,
    );
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseDiagnosticCategory {
    SourceIo,
    Truncated,
    UnsupportedCommand,
    SchemaDrift,
    Corrupt,
    MissingEvidence,
    InternalInvariant,
}

impl ParseDiagnosticCategory {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SourceIo => "source_io",
            Self::Truncated => "truncated",
            Self::UnsupportedCommand => "unsupported_command",
            Self::SchemaDrift => "schema_drift",
            Self::Corrupt => "corrupt",
            Self::MissingEvidence => "missing_evidence",
            Self::InternalInvariant => "internal_invariant",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseStage {
    SourceRead,
    FrameScan,
    FirstPass,
    SecondPass,
    Normalize,
    Finalize,
}

impl ParseStage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SourceRead => "source_read",
            Self::FrameScan => "frame_scan",
            Self::FirstPass => "first_pass",
            Self::SecondPass => "second_pass",
            Self::Normalize => "normalize",
            Self::Finalize => "finalize",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDiagnostic {
    pub category: ParseDiagnosticCategory,
    pub stage: ParseStage,
    pub game_build: Option<String>,
    pub byte_offset: Option<u64>,
    pub frame_index: Option<u64>,
    pub tick: Option<DemoTick>,
    pub command: Option<i32>,
    pub required_item: Option<String>,
    pub observed_counts: BTreeMap<String, u64>,
    pub upstream_source: String,
}

impl ParseDiagnostic {
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "category": self.category.code(),
            "stage": self.stage.code(),
            "game_build": self.game_build,
            "byte_offset": self.byte_offset,
            "frame_index": self.frame_index,
            "tick": self.tick.map(DemoTick::get),
            "command": self.command,
            "required_item": self.required_item,
            "observed_counts": self.observed_counts,
            "upstream_source": self.upstream_source,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParserError {
    pub diagnostic: Box<ParseDiagnostic>,
}

pub fn parser_capability() -> ParserCapability {
    #[cfg(feature = "demoparser")]
    {
        ParserCapability::Pinned {
            commit: DEMOPARSER_COMMIT.into(),
            build: DEMOPARSER_BUILD.into(),
            schema_hash: query_plan_hash(),
        }
    }
    #[cfg(not(feature = "demoparser"))]
    {
        ParserCapability::Unavailable {
            reason: "demoparser feature is disabled".into(),
        }
    }
}

fn raw_u64(fields: &BTreeMap<String, serde_json::Value>, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        let value = fields.get(*name)?;
        value.as_u64().or_else(|| value.as_str()?.parse().ok())
    })
}

fn normalize_event(
    name: &str,
    fields: &BTreeMap<String, serde_json::Value>,
) -> Option<NormalizedEvent> {
    let steam_id = |names: &[&str]| raw_u64(fields, names).map(SteamId::new);
    match name {
        "round_freeze_end" => Some(NormalizedEvent::RoundFreezeEnd(RoundFreezeEndEvent {
            warmup: fields
                .get("warmup")
                .or_else(|| fields.get("warmup_period"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })),
        "round_end" => Some(NormalizedEvent::RoundEnd(RoundEndEvent {
            winner: fields.get("winner").and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str()?.parse::<i64>().ok())
                    .and_then(|winner| i32::try_from(winner).ok())
            }),
        })),
        "player_hurt" => Some(NormalizedEvent::PlayerHurt(PlayerHurtEvent {
            attacker: steam_id(&["attacker_steamid", "attacker"]),
            victim: steam_id(&["user_steamid", "userid", "victim"]),
            damage_health: fields.get("dmg_health").and_then(serde_json::Value::as_i64),
            weapon: fields
                .get("weapon")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })),
        "player_death" => Some(NormalizedEvent::PlayerDeath(PlayerDeathEvent {
            attacker: steam_id(&["attacker_steamid", "attacker"]),
            victim: steam_id(&["user_steamid", "userid", "victim"]),
            assister: steam_id(&["assister_steamid", "assister"]),
            assisted_flash: fields
                .get("assistedflash")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            weapon: fields
                .get("weapon")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })),
        "player_disconnect" => Some(NormalizedEvent::PlayerDisconnect(PlayerDisconnectEvent {
            player: steam_id(&["user_steamid", "userid", "player"]),
        })),
        _ => None,
    }
}

fn parser_error(
    category: ParseDiagnosticCategory,
    stage: ParseStage,
    source: impl Into<String>,
) -> ParserError {
    ParserError {
        diagnostic: Box::new(ParseDiagnostic {
            category,
            stage,
            game_build: None,
            byte_offset: None,
            frame_index: None,
            tick: None,
            command: None,
            required_item: None,
            observed_counts: BTreeMap::new(),
            upstream_source: source.into(),
        }),
    }
}

fn missing_evidence_error(
    required_item: &str,
    game_build: Option<String>,
    observed_counts: BTreeMap<String, u64>,
) -> ParserError {
    let mut error = parser_error(
        ParseDiagnosticCategory::MissingEvidence,
        ParseStage::Finalize,
        format!("required canonical evidence is absent: {required_item}"),
    );
    error.diagnostic.game_build = game_build;
    error.diagnostic.required_item = Some(required_item.into());
    error.diagnostic.observed_counts = observed_counts;
    error
}

#[cfg(feature = "demoparser")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameLocation {
    index: u64,
    byte_offset: u64,
    tick: DemoTick,
    command: i32,
}

#[cfg(feature = "demoparser")]
fn scan_frame_locations(bytes: &[u8]) -> Result<Vec<FrameLocation>, ParserError> {
    use demoparser_parser::{
        first_pass::read_bits::{DemoParserError, read_varint},
        maps::demo_cmd_type_from_int,
    };

    if bytes.len() < 16 {
        let mut error = parser_error(
            ParseDiagnosticCategory::Truncated,
            ParseStage::FrameScan,
            "file ended before the 16-byte Demo preamble",
        );
        error.diagnostic.byte_offset = Some(bytes.len() as u64);
        return Err(error);
    }
    if bytes.get(..DEMO_MAGIC.len()) != Some(DEMO_MAGIC) {
        return Err(parser_error(
            ParseDiagnosticCategory::UnsupportedCommand,
            ParseStage::FrameScan,
            "unsupported Demo file signature",
        ));
    }

    let mut ptr = 16_usize;
    let mut frame_index = 0_u64;
    let mut locations = Vec::new();
    while ptr < bytes.len() {
        let frame_start = ptr;
        let command = read_varint(bytes, &mut ptr)
            .map_err(|source| truncated_frame_error(frame_index, frame_start, None, source))?;
        let tick = read_varint(bytes, &mut ptr)
            .map_err(|source| truncated_frame_error(frame_index, frame_start, None, source))?;
        let size = read_varint(bytes, &mut ptr).map_err(|source| {
            truncated_frame_error(frame_index, frame_start, Some(tick as i32), source)
        })?;
        let command_number = (command & !64) as i32;
        if let Err(source) = demo_cmd_type_from_int(command_number) {
            let mut error = parser_error(
                ParseDiagnosticCategory::UnsupportedCommand,
                ParseStage::FrameScan,
                format!("{source:?}"),
            );
            error.diagnostic.byte_offset = Some(frame_start as u64);
            error.diagnostic.frame_index = Some(frame_index);
            error.diagnostic.tick = Some(DemoTick::new(tick as i32));
            error.diagnostic.command = Some(command_number);
            return Err(error);
        }
        let payload_end = ptr.checked_add(size as usize).ok_or_else(|| {
            truncated_frame_error(
                frame_index,
                frame_start,
                Some(tick as i32),
                DemoParserError::OutOfBytesError,
            )
        })?;
        if payload_end > bytes.len() {
            return Err(truncated_frame_error(
                frame_index,
                frame_start,
                Some(tick as i32),
                DemoParserError::DemoEndsEarly(format!(
                    "declared frame payload ends at byte {payload_end}, file has {} bytes",
                    bytes.len()
                )),
            ));
        }
        locations.push(FrameLocation {
            index: frame_index,
            byte_offset: frame_start as u64,
            tick: DemoTick::new(tick as i32),
            command: command_number,
        });
        ptr = payload_end;
        frame_index = frame_index.saturating_add(1);
    }
    Ok(locations)
}

#[cfg(feature = "demoparser")]
fn truncated_frame_error(
    frame_index: u64,
    byte_offset: usize,
    tick: Option<i32>,
    source: demoparser_parser::first_pass::read_bits::DemoParserError,
) -> ParserError {
    let mut error = parser_error(
        ParseDiagnosticCategory::Truncated,
        ParseStage::FrameScan,
        format!("{source:?}"),
    );
    error.diagnostic.byte_offset = Some(byte_offset as u64);
    error.diagnostic.frame_index = Some(frame_index);
    error.diagnostic.tick = tick.map(DemoTick::new);
    error
}

#[cfg(feature = "demoparser")]
fn map_upstream_error(
    error: demoparser_parser::first_pass::read_bits::DemoParserError,
    frames: &[FrameLocation],
) -> ParserError {
    use demoparser_parser::first_pass::read_bits::{DemoParserError as Upstream, DemoParserStage};

    let (context, source) = match error {
        Upstream::Context { context, source } => (Some(context), *source),
        source => (None, source),
    };
    let category = match &source {
        Upstream::OutOfBitsError
        | Upstream::OutOfBytesError
        | Upstream::FailedByteRead(_)
        | Upstream::DemoEndsEarly(_) => ParseDiagnosticCategory::Truncated,
        Upstream::UnknownDemoCmd(_) | Upstream::Source1DemoError | Upstream::UnknownFile => {
            ParseDiagnosticCategory::UnsupportedCommand
        }
        Upstream::ClassMapperNotFoundFirstPass
        | Upstream::FieldNoDecoder
        | Upstream::UnknownPathOP
        | Upstream::ClassNotFound
        | Upstream::StringTableNotFound
        | Upstream::IncorrectMetaDataProp
        | Upstream::UnknownPropName(_)
        | Upstream::GameEventListNotSet
        | Upstream::PropTypeNotFound(_)
        | Upstream::GameEventUnknownId(_)
        | Upstream::UnknownPawnPrefix(_)
        | Upstream::UnknownEntityHandle(_)
        | Upstream::ClsIdOutOfBounds
        | Upstream::UnknownGameEventVariant(_)
        | Upstream::NoSendTableMessage
        | Upstream::IllegalPathOp => ParseDiagnosticCategory::SchemaDrift,
        Upstream::MalformedMessage
        | Upstream::DecompressionFailure(_)
        | Upstream::MalformedVoicePacket => ParseDiagnosticCategory::Corrupt,
        Upstream::NoEvents => ParseDiagnosticCategory::MissingEvidence,
        Upstream::EntityNotFound
        | Upstream::UserIdNotFound
        | Upstream::EventListFallbackNotFound(_)
        | Upstream::VoiceDataWriteError(_)
        | Upstream::VectorResizeFailure
        | Upstream::ImpossibleCmd
        | Upstream::UnkVoiceFormat
        | Upstream::FileNotFound(_) => ParseDiagnosticCategory::InternalInvariant,
        Upstream::Context { .. } => ParseDiagnosticCategory::InternalInvariant,
    };
    let stage = context
        .as_ref()
        .map_or(ParseStage::FirstPass, |context| match context.stage {
            DemoParserStage::FirstPass => ParseStage::FirstPass,
            DemoParserStage::SecondPass => ParseStage::SecondPass,
        });
    let mut mapped = parser_error(category, stage, format!("{source:?}"));
    if let Some(context) = context {
        mapped.diagnostic.game_build = context.game_build;
        mapped.diagnostic.byte_offset = context.byte_offset.map(|offset| offset as u64);
        mapped.diagnostic.tick = context.tick.map(DemoTick::new);
        if let Some(offset) = mapped.diagnostic.byte_offset
            && let Some(frame) = frames
                .iter()
                .rev()
                .find(|frame| frame.byte_offset <= offset)
        {
            mapped.diagnostic.frame_index = Some(frame.index);
            mapped.diagnostic.command = Some(frame.command);
            mapped.diagnostic.tick.get_or_insert(frame.tick);
        }
    }
    if let Upstream::UnknownDemoCmd(command) = source {
        mapped.diagnostic.command = Some(command);
    }
    mapped
}

#[cfg(feature = "demoparser")]
pub fn parse_with_pinned_demoparser(path: &Path) -> Result<ParsedOutput, ParserError> {
    parse_with_pinned_demoparser_with_progress(path, &mut |_| {})
}

#[cfg(feature = "demoparser")]
pub fn parse_with_pinned_demoparser_with_progress(
    path: &Path,
    progress: &mut dyn FnMut(ParserProgress),
) -> Result<ParsedOutput, ParserError> {
    use ahash::AHashMap;
    use demoparser_parser::second_pass::parser_settings::create_huffman_lookup_table;
    use demoparser_parser::{
        first_pass::parser_settings::{EventSnapshotMode, FirstPassParser, ParserInputs},
        parse_demo::{Parser, ParsingMode},
        second_pass::variants::{OutputSerdeHelperStruct, soa_to_aos},
    };
    let bytes = std::fs::read(path).map_err(|source| {
        parser_error(
            ParseDiagnosticCategory::SourceIo,
            ParseStage::SourceRead,
            source.to_string(),
        )
    })?;
    let huf = create_huffman_lookup_table();
    let inputs = ParserInputs {
        real_name_to_og_name: AHashMap::new(),
        wanted_players: vec![],
        wanted_player_props: vec![
            "player_steamid".into(),
            "team_num".into(),
            "is_alive".into(),
            "life_state".into(),
            "health".into(),
        ],
        wanted_other_props: vec!["total_rounds_played".into()],
        wanted_prop_states: AHashMap::new(),
        wanted_ticks: vec![],
        wanted_events: EVENT_QUERY.iter().map(|s| (*s).into()).collect(),
        event_snapshot_mode: Some(EventSnapshotMode::AfterEventPacket),
        parse_ents: true,
        parse_projectiles: false,
        parse_grenades: false,
        only_header: false,
        only_convars: false,
        huffman_lookup_table: &huf,
        order_by_steamid: false,
        list_props: false,
        fallback_bytes: None,
    };
    let game_build = (bytes.len() >= 16)
        .then(|| {
            FirstPassParser::new(&inputs)
                .parse_header_only(&bytes)
                .ok()
                .and_then(|header| header.get("patch_version").cloned())
        })
        .flatten();
    let frame_locations = scan_frame_locations(&bytes).map_err(|mut error| {
        error.diagnostic.game_build = game_build;
        error
    })?;
    let mut parser = Parser::new(inputs, ParsingMode::ForceSingleThreaded);
    let out = parser
        .parse_demo_with_progress(&bytes, &mut |upstream| {
            let phase = match upstream.phase {
                demoparser_parser::progress::ParsePhase::FirstPass => {
                    ParserProgressPhase::FirstPass
                }
                demoparser_parser::progress::ParsePhase::SecondPass => {
                    ParserProgressPhase::SecondPass
                }
                demoparser_parser::progress::ParsePhase::Finalize => ParserProgressPhase::Finalize,
            };
            progress(ParserProgress {
                phase,
                bytes_consumed: upstream.bytes_consumed,
                total_bytes: upstream.total_bytes,
                frames: upstream.frames,
                events_emitted: upstream.events_emitted,
            });
        })
        .map_err(|error| map_upstream_error(error, &frame_locations))?;
    let player_snapshots = soa_to_aos(OutputSerdeHelperStruct {
        prop_infos: out.prop_controller.prop_infos.clone(),
        inner: out.df.into(),
    })
    .into_iter()
    .enumerate()
    .filter_map(|(ordinal, row)| {
        let raw_properties: BTreeMap<String, serde_json::Value> = row
            .into_iter()
            .filter_map(|(name, value)| {
                value.and_then(|v| serde_json::to_value(v).ok().map(|v| (name, v)))
            })
            .collect();
        let i64_value = |name: &str| raw_properties.get(name).and_then(serde_json::Value::as_i64);
        let steam_id = raw_properties
            .get("steamid")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))?;
        Some(PlayerSnapshot {
            tick: DemoTick::new(i64_value("tick")? as i32),
            ingestion_ordinal: IngestionOrdinal::new(ordinal as u64),
            phase: SnapshotPhase::AfterEventPacket,
            steam_id: SteamId::new(steam_id),
            entity_id: i64_value("entity_id").map(|v| EntityId::new(v as i32)),
            team: i64_value("team_num").map(|v| v as i32),
            health: i64_value("health").map(|v| v as i32),
            alive: raw_properties
                .get("is_alive")
                .and_then(serde_json::Value::as_bool),
            life_state: i64_value("life_state").map(|v| v as i32),
            round_counter: i64_value("total_rounds_played").map(|v| v as i32),
        })
    })
    .collect::<Vec<_>>();
    let header = out.header.unwrap_or_default();
    let participants: Vec<Participant> = out
        .roster
        .iter()
        .filter_map(|p| {
            p.steamid.map(|id| Participant {
                steam_id: SteamId::new(id),
                name: p.name.clone(),
                team: p.team_number,
            })
        })
        .collect();
    let mut events = Vec::new();
    let mut receipts = Vec::new();
    let mut rounds = Vec::new();
    let mut round_no = 0;
    for (ordinal, event) in out.game_events.iter().enumerate() {
        let fields: BTreeMap<String, String> = event
            .fields
            .iter()
            .filter_map(|f| {
                f.data.as_ref().map(|v| {
                    (
                        f.name.clone(),
                        serde_json::to_value(v)
                            .unwrap_or(serde_json::Value::Null)
                            .to_string(),
                    )
                })
            })
            .collect();
        let raw_fields: BTreeMap<String, serde_json::Value> = event
            .fields
            .iter()
            .filter_map(|f| {
                f.data.as_ref().map(|v| {
                    (
                        f.name.clone(),
                        serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                    )
                })
            })
            .collect();
        let normalized = normalize_event(&event.name, &raw_fields).ok_or_else(|| {
            let mut error = parser_error(
                ParseDiagnosticCategory::SchemaDrift,
                ParseStage::Normalize,
                format!("unrecognized requested event {}", event.name),
            );
            error.diagnostic.game_build = header.get("patch_version").cloned();
            error.diagnostic.tick = Some(DemoTick::new(event.tick));
            error.diagnostic.required_item = Some(event.name.clone());
            error
        })?;
        let parsed = ParsedEvent {
            tick: DemoTick::new(event.tick),
            ingestion_ordinal: IngestionOrdinal::new(ordinal as u64),
            event: normalized,
        };
        if event.name == "round_end" {
            round_no += 1;
            let NormalizedEvent::RoundEnd(round_end) = &parsed.event else {
                let mut error = parser_error(
                    ParseDiagnosticCategory::InternalInvariant,
                    ParseStage::Normalize,
                    "round_end normalized to a different event variant",
                );
                error.diagnostic.game_build = header.get("patch_version").cloned();
                error.diagnostic.tick = Some(DemoTick::new(event.tick));
                return Err(error);
            };
            rounds.push(ParsedRound {
                number: RoundNumber::new(round_no),
                end_tick: DemoTick::new(event.tick),
                winner: round_end.winner,
            });
        }
        let evidence_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(ordinal, &event.name, event.tick, &raw_fields)).map_err(
                    |source| {
                        let mut error = parser_error(
                            ParseDiagnosticCategory::InternalInvariant,
                            ParseStage::Finalize,
                            source.to_string(),
                        );
                        error.diagnostic.game_build = header.get("patch_version").cloned();
                        error.diagnostic.tick = Some(DemoTick::new(event.tick));
                        error
                    }
                )?
            )
        );
        receipts.push(EventReceipt {
            ingestion_ordinal: IngestionOrdinal::new(ordinal as u64),
            event_name: event.name.clone(),
            tick: DemoTick::new(event.tick),
            fields: fields.clone(),
            raw_fields,
            evidence_sha256,
        });
        events.push(parsed);
    }
    let suspicious_empty = participants.is_empty()
        || rounds.is_empty()
        || !events
            .iter()
            .any(|event| event.kind() == NormalizedEventKind::PlayerDeath);
    let observed_counts = BTreeMap::from([
        ("participants".into(), participants.len() as u64),
        ("rounds".into(), rounds.len() as u64),
        ("events".into(), events.len() as u64),
        ("receipts".into(), receipts.len() as u64),
        ("player_snapshots".into(), player_snapshots.len() as u64),
    ]);
    let missing_evidence = |required_item: &str| {
        missing_evidence_error(
            required_item,
            header.get("patch_version").cloned(),
            observed_counts.clone(),
        )
    };
    if participants.is_empty() {
        return Err(missing_evidence("participants"));
    }
    if rounds.is_empty() {
        return Err(missing_evidence("round_end"));
    }
    if !events
        .iter()
        .any(|event| event.kind() == NormalizedEventKind::PlayerDeath)
    {
        return Err(missing_evidence("player_death"));
    }
    for required in [
        NormalizedEventKind::RoundFreezeEnd,
        NormalizedEventKind::PlayerHurt,
    ] {
        if !events.iter().any(|event| event.kind() == required) {
            return Err(missing_evidence(required.name()));
        }
    }
    if player_snapshots.is_empty() {
        return Err(missing_evidence("player_snapshots"));
    }
    let query_hash = query_plan_hash();
    let source_sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(ParsedOutput {
        metadata: DemoMetadata {
            map: header.get("map_name").cloned(),
            patch_build: header.get("patch_version").cloned(),
            demo_stamp: header.get("demo_file_stamp").cloned(),
            server: header.get("server_name").cloned(),
            game_directory: header.get("game_directory").cloned(),
            tick_rate: out.convars.get("sv_tickrate").cloned(),
            tick_rate_unavailable_reason: (!out.convars.contains_key("sv_tickrate"))
                .then(|| "pinned parser did not expose sv_tickrate in demo convars".into()),
        },
        participants,
        rounds,
        events,
        receipts,
        player_snapshots,
        suspicious_empty,
        identity: CalculationIdentity {
            source_sha256,
            parser_commit: DEMOPARSER_COMMIT.into(),
            parser_build: DEMOPARSER_BUILD.into(),
            generated_proto_build: GENERATED_PROTO_BUILD.into(),
            requested_schema_hash: query_hash,
            metric_definition_version: "openfrag-rating-1".into(),
            formula_version: "ofr-1.0.0".into(),
            evidence_semantics_epoch: EVIDENCE_SEMANTICS_EPOCH.into(),
        },
    })
}

pub trait ImportStore {
    fn save_attempt(&mut self, attempt: &Attempt) -> Result<(), ErrorCode>;
    fn load_attempt(&self, id: u64) -> Option<Attempt>;
}

pub trait Persist {
    fn put(&mut self, job: &Job) -> Result<(), ErrorCode>;
    fn get(&self, id: u64) -> Option<Job>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub state: State,
    pub attempt: Attempt,
    pub identity: Option<CalculationIdentity>,
    pub canonical: bool,
    pub quarantined: bool,
    pub cancelled: bool,
}

pub fn legal_transition(from: &State, to: &State) -> bool {
    matches!(
        (from, to),
        (State::AwaitingImport, State::Validating)
            | (State::Validating, State::Hashing | State::Error(_))
            | (State::Hashing, State::Deduplicating | State::Error(_))
            | (
                State::Deduplicating,
                State::Copying | State::Parsing | State::Rating | State::Ready | State::Error(_)
            )
            | (State::Copying, State::Parsing | State::Error(_))
            | (State::Parsing, State::Rating | State::Error(_))
            | (State::Rating, State::LinkingClips | State::Error(_))
            | (State::LinkingClips, State::Ready | State::Error(_))
            | (
                State::Error(_),
                State::AwaitingImport
                    | State::Validating
                    | State::Hashing
                    | State::Copying
                    | State::Parsing
                    | State::Rating
            )
            | (State::Ready, State::Parsing | State::Rating)
    )
}

pub struct Orchestrator<P: Persist> {
    pub store: P,
    next_id: u64,
}
impl<P: Persist> Orchestrator<P> {
    pub fn new(store: P) -> Self {
        Self { store, next_id: 1 }
    }
    pub fn create(&mut self) -> Result<Job, ErrorCode> {
        let job = self.job(State::AwaitingImport);
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn resume(&mut self, id: u64) -> Result<Job, ErrorCode> {
        self.store.get(id).ok_or(ErrorCode::Io)
    }
    pub fn transition(&mut self, mut job: Job, to: State) -> Result<Job, ErrorCode> {
        if job.cancelled || !legal_transition(&job.state, &to) {
            return Err(ErrorCode::Analysis);
        }
        job.state = to.clone();
        job.attempt.state = to;
        job.attempt.progress.heartbeat += 1;
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn acquire_lease(
        &mut self,
        mut job: Job,
        owner: &str,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<Job, ErrorCode> {
        if !lease_available(&job.attempt, now, owner) {
            return Err(ErrorCode::Io);
        }
        job.attempt.lease_owner = Some(owner.into());
        job.attempt.lease_expires_at = Some(now + ttl);
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn renew_lease(
        &mut self,
        mut job: Job,
        owner: &str,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<Job, ErrorCode> {
        if job.attempt.lease_owner.as_deref() != Some(owner) {
            return Err(ErrorCode::Io);
        }
        job.attempt.lease_expires_at = Some(now + ttl);
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn release_lease(&mut self, mut job: Job, owner: &str) -> Result<Job, ErrorCode> {
        if job.attempt.lease_owner.as_deref() != Some(owner) {
            return Err(ErrorCode::Io);
        }
        job.attempt.lease_owner = None;
        job.attempt.lease_expires_at = None;
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn retry(&mut self, mut job: Job, manual: bool) -> Result<Job, ErrorCode> {
        if manual {
            job.attempt.retries = 0;
            job.attempt.next_retry_after = None;
        } else {
            let delay = retry_delay(job.attempt.retries).ok_or(ErrorCode::Io)?;
            job.attempt.next_retry_after = Some(delay);
            job.attempt.retries += 1;
        }
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn cancel(&mut self, mut job: Job) -> Result<Job, ErrorCode> {
        job.cancelled = true;
        job.state = State::AwaitingImport;
        job.attempt.state = job.state.clone();
        self.store.put(&job)?;
        Ok(job)
    }
    pub fn quarantine_empty(&mut self, mut job: Job) -> Result<Job, ErrorCode> {
        job.quarantined = true;
        job.state = State::Error(ErrorCode::Parse);
        job.attempt.state = job.state.clone();
        self.store.put(&job)?;
        Ok(job)
    }
    fn job(&mut self, state: State) -> Job {
        let id = self.next_id;
        self.next_id += 1;
        Job {
            id,
            state: state.clone(),
            attempt: Attempt {
                id,
                state,
                retries: 0,
                next_retry_after: None,
                progress: Progress {
                    processed_bytes: 0,
                    total_bytes: None,
                    indeterminate: true,
                    work_done: 0,
                    heartbeat: 0,
                },
                lease_owner: None,
                lease_expires_at: None,
            },
            identity: None,
            canonical: false,
            quarantined: false,
            cancelled: false,
        }
    }
}

pub trait ParserAdapter {
    type Parsed;
    type Error: std::fmt::Display;
    fn parse(
        &self,
        demo: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Self::Parsed, Self::Error>;
    fn rate(&self, parsed: &Self::Parsed) -> Result<(), Self::Error>;
}

pub fn validate(path: &Path, free_bytes: u64) -> Result<u64, ErrorCode> {
    if path.is_symlink() {
        return Err(ErrorCode::Corrupt);
    }
    let meta = fs::metadata(path).map_err(|_| ErrorCode::Io)?;
    if !meta.is_file() {
        return Err(ErrorCode::Corrupt);
    }
    if meta.len() > MAX_DEMO_BYTES {
        return Err(ErrorCode::Size);
    }
    if free_bytes < meta.len() + meta.len() / 10 {
        return Err(ErrorCode::Io);
    }
    if meta.len() < DEMO_MAGIC.len() as u64 {
        return Err(ErrorCode::Corrupt);
    }
    let mut f = fs::File::open(path).map_err(|_| ErrorCode::Io)?;
    let mut magic = [0u8; DEMO_MAGIC.len()];
    f.read_exact(&mut magic).map_err(|_| ErrorCode::Corrupt)?;
    if magic != DEMO_MAGIC {
        return Err(ErrorCode::Unsupported);
    }
    Ok(meta.len())
}

pub fn hash_file(
    src: &Path,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, ErrorCode> {
    let before = fs::metadata(src).map_err(|_| ErrorCode::Io)?;
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    loop {
        let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n).ok_or(ErrorCode::Io)?);
        done += n as u64;
        progress(Progress {
            processed_bytes: done,
            total_bytes: Some(total),
            indeterminate: false,
            work_done: 0,
            heartbeat: done,
        });
    }
    let after = fs::metadata(src).map_err(|_| ErrorCode::Io)?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || done != total
    {
        return Err(ErrorCode::Io);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn copy_content_addressed(
    src: &Path,
    dst: &Path,
    expected_hash: &str,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ErrorCode> {
    if dst.exists() {
        return Ok(dst.to_path_buf());
    }
    let parent = dst.parent().ok_or(ErrorCode::Io)?;
    fs::create_dir_all(parent).map_err(|_| ErrorCode::Io)?;
    let tmp = parent.join(format!(
        ".staging-{}-{:x}",
        std::process::id(),
        Sha256::digest(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_le_bytes()
        )
    ));
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|_| ErrorCode::Io)?;
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    let mut h = Sha256::new();
    let result = (|| {
        loop {
            let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
            if n == 0 {
                break;
            }
            let read = buf.get(..n).ok_or(ErrorCode::Io)?;
            h.update(read);
            out.write_all(read).map_err(|_| ErrorCode::Io)?;
            done += n as u64;
            progress(Progress {
                processed_bytes: done,
                total_bytes: Some(total),
                indeterminate: false,
                work_done: 0,
                heartbeat: done,
            });
        }
        out.sync_all().map_err(|_| ErrorCode::Io)?;
        if done != total || format!("{:x}", h.finalize()) != expected_hash {
            return Err(ErrorCode::Corrupt);
        }
        fs::rename(&tmp, dst).map_err(|_| ErrorCode::Io)?;
        if let Some(p) = dst.parent() {
            fs::File::open(p)
                .and_then(|f| f.sync_all())
                .map_err(|_| ErrorCode::Io)?;
        }
        Ok(dst.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn hash_and_copy(
    src: &Path,
    dst: &Path,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, ErrorCode> {
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|_| ErrorCode::Io)?;
    }
    let tmp = dst.with_extension(format!(
        "staging-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|_| ErrorCode::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    let result = (|| {
        loop {
            let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
            if n == 0 {
                break;
            }
            let read = buf.get(..n).ok_or(ErrorCode::Io)?;
            hasher.update(read);
            output.write_all(read).map_err(|_| ErrorCode::Io)?;
            done += n as u64;
            progress(Progress {
                processed_bytes: done,
                total_bytes: Some(total),
                indeterminate: false,
                work_done: 0,
                heartbeat: done,
            });
        }
        output.sync_all().map_err(|_| ErrorCode::Io)?;
        fs::rename(&tmp, dst).map_err(|_| ErrorCode::Io)?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn retry_delay(automatic_retry: u8) -> Option<Duration> {
    match automatic_retry {
        0 => Some(Duration::from_secs(2)),
        1 => Some(Duration::from_secs(10)),
        2 => Some(Duration::from_secs(60)),
        _ => None,
    }
}

pub fn lease_available(attempt: &Attempt, now: SystemTime, owner: &str) -> bool {
    attempt
        .lease_owner
        .as_deref()
        .map(|x| x == owner)
        .unwrap_or(true)
        || attempt.lease_expires_at.map(|x| x <= now).unwrap_or(true)
}

pub fn choose_dedup(
    artifact_exists: bool,
    run_succeeded: bool,
    run_running: bool,
    parser_changed: bool,
    formula_changed: bool,
    prior_failed: bool,
) -> DedupDecision {
    if !artifact_exists {
        DedupDecision::Copy
    } else if run_succeeded && !parser_changed && !formula_changed {
        DedupDecision::Ready
    } else if run_running {
        DedupDecision::Resume
    } else if parser_changed {
        DedupDecision::Parse
    } else if formula_changed {
        DedupDecision::Rate
    } else if prior_failed {
        DedupDecision::PriorFailure
    } else {
        DedupDecision::Parse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    struct Fake;
    #[derive(Default)]
    struct Mem(BTreeMap<u64, Job>);
    impl Persist for Mem {
        fn put(&mut self, job: &Job) -> Result<(), ErrorCode> {
            self.0.insert(job.id, job.clone());
            Ok(())
        }
        fn get(&self, id: u64) -> Option<Job> {
            self.0.get(&id).cloned()
        }
    }
    impl ParserAdapter for Fake {
        type Parsed = u8;
        type Error = &'static str;
        fn parse(&self, _: &Path, p: &mut dyn FnMut(u64)) -> Result<u8, Self::Error> {
            p(1);
            Ok(1)
        }
        fn rate(&self, _: &u8) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    #[test]
    fn normalized_schema_is_pinned_and_exhaustive() {
        let schema = normalized_event_schema();
        assert_eq!(schema["$id"], NORMALIZED_SCHEMA_VERSION);
        assert_eq!(schema["oneOf"].as_array().unwrap().len(), EVENT_QUERY.len());
        let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&schema).unwrap()));
        assert_eq!(
            hash,
            "8f7f813904438aa2d1171b8ba67bd10efa7fe945b2abe1437b00eeebb61b1e5a"
        );
    }

    #[test]
    fn normalized_events_do_not_retain_raw_parser_keys() {
        let raw = BTreeMap::from([
            ("attacker_steamid".into(), serde_json::json!("10")),
            ("user_steamid".into(), serde_json::json!("20")),
            ("upstream_only".into(), serde_json::json!("receipt-sidecar")),
        ]);
        let event = ParsedEvent::from_raw("player_death", 12, 3, &raw).unwrap();
        assert_eq!(event.attacker(), Some(SteamId::new(10)));
        assert_eq!(event.victim(), Some(SteamId::new(20)));
        assert_eq!(event.name(), "player_death");
    }

    #[test]
    fn missing_evidence_diagnostic_preserves_required_item_and_counts() {
        let error = missing_evidence_error(
            "player_death",
            Some("fixture-build".into()),
            BTreeMap::from([("events".into(), 0), ("participants".into(), 10)]),
        );
        assert_eq!(
            error.diagnostic.category,
            ParseDiagnosticCategory::MissingEvidence
        );
        assert_eq!(error.diagnostic.stage, ParseStage::Finalize);
        assert_eq!(
            error.diagnostic.required_item.as_deref(),
            Some("player_death")
        );
        assert_eq!(error.diagnostic.observed_counts["events"], 0);
        assert_eq!(error.diagnostic.observed_counts["participants"], 10);
    }

    #[cfg(feature = "demoparser")]
    #[test]
    fn malformed_outer_frames_report_stable_locations() {
        let directory = tempdir().unwrap();
        let truncated_path = directory.path().join("truncated.dem");
        let mut truncated = [DEMO_MAGIC, &[0_u8; 8]].concat();
        truncated.extend([1, 5, 10, 0, 0]);
        fs::write(&truncated_path, truncated).unwrap();
        let error = parse_with_pinned_demoparser(&truncated_path).unwrap_err();
        assert_eq!(
            error.diagnostic.category,
            ParseDiagnosticCategory::Truncated
        );
        assert_eq!(error.diagnostic.stage, ParseStage::FrameScan);
        assert_eq!(error.diagnostic.byte_offset, Some(16));
        assert_eq!(error.diagnostic.frame_index, Some(0));
        assert_eq!(error.diagnostic.tick, Some(DemoTick::new(5)));

        let unsupported_path = directory.path().join("unsupported.dem");
        let mut unsupported = [DEMO_MAGIC, &[0_u8; 8]].concat();
        unsupported.extend([63, 7, 0]);
        fs::write(&unsupported_path, unsupported).unwrap();
        let error = parse_with_pinned_demoparser(&unsupported_path).unwrap_err();
        assert_eq!(
            error.diagnostic.category,
            ParseDiagnosticCategory::UnsupportedCommand
        );
        assert_eq!(error.diagnostic.command, Some(63));
        assert_eq!(error.diagnostic.byte_offset, Some(16));
        assert_eq!(error.diagnostic.tick, Some(DemoTick::new(7)));
    }

    #[cfg(feature = "demoparser")]
    #[test]
    fn upstream_failures_map_without_losing_parser_context() {
        use demoparser_parser::first_pass::read_bits::{
            DemoParserError as Upstream, DemoParserErrorContext, DemoParserStage,
        };

        let frames = vec![FrameLocation {
            index: 9,
            byte_offset: 4_000,
            tick: DemoTick::new(1_024),
            command: 7,
        }];
        let context = DemoParserErrorContext {
            stage: DemoParserStage::SecondPass,
            byte_offset: Some(4_096),
            tick: Some(1_024),
            game_build: Some("fixture-build".into()),
        };
        for (source, category) in [
            (
                Upstream::UnknownPropName("health".into()),
                ParseDiagnosticCategory::SchemaDrift,
            ),
            (
                Upstream::DecompressionFailure("snappy".into()),
                ParseDiagnosticCategory::Corrupt,
            ),
            (
                Upstream::ImpossibleCmd,
                ParseDiagnosticCategory::InternalInvariant,
            ),
            (Upstream::NoEvents, ParseDiagnosticCategory::MissingEvidence),
        ] {
            let error = map_upstream_error(source.with_context(context.clone()), &frames);
            assert_eq!(error.diagnostic.category, category);
            assert_eq!(error.diagnostic.stage, ParseStage::SecondPass);
            assert_eq!(
                error.diagnostic.game_build.as_deref(),
                Some("fixture-build")
            );
            assert_eq!(error.diagnostic.byte_offset, Some(4_096));
            assert_eq!(error.diagnostic.frame_index, Some(9));
            assert_eq!(error.diagnostic.tick, Some(DemoTick::new(1_024)));
            assert!(!error.diagnostic.upstream_source.is_empty());
        }
    }

    #[test]
    fn validates_limit_and_headroom() {
        let d = tempdir().unwrap();
        let p = d.path().join("x.dem");
        fs::write(&p, DEMO_MAGIC).unwrap();
        assert_eq!(validate(&p, 7), Err(ErrorCode::Io));
        assert_eq!(validate(&p, 8), Ok(8));
    }
    #[test]
    fn hashes_and_copies_atomically() {
        let d = tempdir().unwrap();
        let s = d.path().join("a.dem");
        let t = d.path().join("store/a.dem");
        fs::write(&s, [DEMO_MAGIC, b"demo bytes"].concat()).unwrap();
        let mut seen = Vec::new();
        let h = hash_and_copy(&s, &t, 18, &mut |p| seen.push(p)).unwrap();
        assert!(t.is_file());
        assert_eq!(seen.last().unwrap().fraction_millionths(), Some(1_000_000));
        assert_eq!(h.len(), 64);
    }
    #[test]
    fn retries_are_exact() {
        assert_eq!(retry_delay(0), Some(Duration::from_secs(2)));
        assert_eq!(retry_delay(1), Some(Duration::from_secs(10)));
        assert_eq!(retry_delay(2), Some(Duration::from_secs(60)));
        assert_eq!(retry_delay(3), None);
    }
    #[test]
    fn dedup_branches_are_deterministic() {
        assert_eq!(
            choose_dedup(false, false, false, false, false, false),
            DedupDecision::Copy
        );
        assert_eq!(
            choose_dedup(true, true, false, false, false, false),
            DedupDecision::Ready
        );
        assert_eq!(
            choose_dedup(true, false, true, false, false, false),
            DedupDecision::Resume
        );
        assert_eq!(
            choose_dedup(true, false, false, true, false, false),
            DedupDecision::Parse
        );
        assert_eq!(
            choose_dedup(true, false, false, false, true, false),
            DedupDecision::Rate
        );
    }
    #[test]
    fn fake_adapter_is_not_production_parser() {
        let d = tempdir().unwrap();
        let p = d.path().join("x");
        fs::write(&p, DEMO_MAGIC).unwrap();
        let f = Fake;
        let mut n = 0;
        let parsed = f.parse(&p, &mut |x| n = x).unwrap();
        f.rate(&parsed).unwrap();
        assert_eq!(n, 1);
    }
    #[test]
    fn orchestrator_transitions_and_lease_race() {
        let mut o = Orchestrator::new(Mem::default());
        let j = o.create().unwrap();
        let j = o.transition(j, State::Validating).unwrap();
        let now = SystemTime::now();
        let j = o
            .acquire_lease(j, "a", now, Duration::from_secs(60))
            .unwrap();
        assert!(
            o.acquire_lease(j.clone(), "b", now, Duration::from_secs(60))
                .is_err()
        );
        let j = o.renew_lease(j, "a", now, Duration::from_secs(60)).unwrap();
        o.release_lease(j, "a").unwrap();
    }
    #[test]
    fn retries_cancel_quarantine_and_resume() {
        let mut o = Orchestrator::new(Mem::default());
        let j = o.create().unwrap();
        let j = o.retry(j, false).unwrap();
        assert_eq!(j.attempt.retries, 1);
        let j = o.retry(j, true).unwrap();
        assert_eq!(j.attempt.retries, 0);
        let j = o.quarantine_empty(j).unwrap();
        assert!(!j.canonical && j.quarantined);
        let j = o.cancel(j).unwrap();
        assert!(j.cancelled);
        assert_eq!(o.resume(j.id).unwrap().id, j.id);
    }
    #[test]
    fn legal_transition_table_rejects_skips() {
        assert!(legal_transition(&State::AwaitingImport, &State::Validating));
        assert!(!legal_transition(&State::AwaitingImport, &State::Ready));
    }

    #[cfg(feature = "demoparser")]
    #[test]
    fn pinned_capability_uses_canonical_query_identity() {
        assert_eq!(
            parser_capability(),
            ParserCapability::Pinned {
                commit: DEMOPARSER_COMMIT.into(),
                build: DEMOPARSER_BUILD.into(),
                schema_hash: query_plan_hash(),
            }
        );
        assert_ne!(query_plan_hash(), QUERY_PLAN_VERSION);
        assert_eq!(SNAPSHOT_PHASE, "after-event-packet");
    }

    #[cfg(feature = "demoparser")]
    #[test]
    fn pinned_fixture_adapter_smoke_when_requested() {
        let Ok(path) = std::env::var("OPENFRAG_DEMOPARSER_FIXTURE") else {
            return;
        };
        let start = std::time::Instant::now();
        let mut parser_progress = Vec::new();
        let parsed =
            parse_with_pinned_demoparser_with_progress(Path::new(&path), &mut |progress| {
                parser_progress.push(progress)
            })
            .expect("pinned fixture must parse");
        assert!(
            parser_progress
                .iter()
                .any(|progress| progress.phase == ParserProgressPhase::FirstPass)
        );
        assert!(
            parser_progress
                .iter()
                .any(|progress| progress.phase == ParserProgressPhase::SecondPass)
        );
        assert_eq!(
            parser_progress.last().map(|progress| progress.phase),
            Some(ParserProgressPhase::Finalize)
        );
        for phase in [
            ParserProgressPhase::FirstPass,
            ParserProgressPhase::SecondPass,
            ParserProgressPhase::Finalize,
        ] {
            let samples = parser_progress
                .iter()
                .filter(|progress| progress.phase == phase)
                .collect::<Vec<_>>();
            assert!(samples.windows(2).all(|samples| {
                samples[0].bytes_consumed <= samples[1].bytes_consumed
                    && samples[0].frames <= samples[1].frames
                    && samples[0].events_emitted <= samples[1].events_emitted
            }));
            assert!(
                samples
                    .iter()
                    .all(|sample| sample.bytes_consumed <= sample.total_bytes)
            );
        }
        assert!(!parsed.suspicious_empty);
        assert_eq!(parsed.metadata.map.as_deref(), Some("de_mirage"));
        assert_eq!(parsed.participants.len(), 10);
        assert_eq!(parsed.rounds.len(), 10);
        assert_eq!(parsed.events.len(), 366);
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|event| event.kind() == NormalizedEventKind::PlayerHurt)
                .count(),
            264
        );
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|event| event.kind() == NormalizedEventKind::PlayerDeath)
                .count(),
            73
        );
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|event| event.kind() == NormalizedEventKind::RoundFreezeEnd)
                .count(),
            9
        );
        let hurt = parsed
            .events
            .iter()
            .find(|event| event.kind() == NormalizedEventKind::PlayerHurt)
            .unwrap();
        assert_eq!(hurt.attacker(), Some(SteamId::new(76561197964020430)));
        assert_eq!(hurt.victim(), Some(SteamId::new(76561198073049527)));
        let NormalizedEvent::PlayerHurt(hurt_event) = &hurt.event else {
            unreachable!();
        };
        assert_eq!(hurt_event.damage_health, Some(100));
        assert_eq!(hurt.weapon(), Some("p250"));
        let death = parsed
            .events
            .iter()
            .find(|event| event.kind() == NormalizedEventKind::PlayerDeath)
            .unwrap();
        assert_eq!(death.attacker(), Some(SteamId::new(76561197964020430)));
        assert_eq!(death.victim(), Some(SteamId::new(76561198073049527)));
        assert_eq!(death.weapon(), Some("p250"));
        assert!(!parsed.player_snapshots.is_empty());
        assert!(
            parsed
                .player_snapshots
                .iter()
                .all(|snapshot| snapshot.phase == SnapshotPhase::AfterEventPacket)
        );
        assert!(parsed.player_snapshots.iter().all(|snapshot| {
            parsed
                .participants
                .iter()
                .any(|participant| participant.steam_id == snapshot.steam_id)
        }));
        let event_ticks = parsed
            .events
            .iter()
            .map(|event| event.tick)
            .collect::<std::collections::BTreeSet<_>>();
        let snapshot_ticks_without_events = parsed
            .player_snapshots
            .iter()
            .map(|snapshot| snapshot.tick)
            .filter(|tick| !event_ticks.contains(tick))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            snapshot_ticks_without_events.is_empty(),
            "snapshot ticks without requested events: {snapshot_ticks_without_events:?}"
        );
        assert!(
            parsed
                .events
                .windows(2)
                .any(|events| events[0].tick == events[1].tick)
        );
        assert!(parsed.events.windows(2).all(|events| {
            events[0].tick != events[1].tick
                || events[0].ingestion_ordinal < events[1].ingestion_ordinal
        }));
        assert!(
            parsed
                .player_snapshots
                .windows(2)
                .all(|snapshots| snapshots[0].ingestion_ordinal < snapshots[1].ingestion_ordinal)
        );
        assert!(
            parsed.metadata.tick_rate.is_some()
                || parsed.metadata.tick_rate_unavailable_reason.is_some()
        );
        assert_eq!(
            parsed.identity.source_sha256,
            "84a1a4191302bdd2a3bbb5a727842093744b1fb1a228aeec630369e44b622cb2"
        );
        assert_eq!(parsed.identity.formula_version, "ofr-1.0.0");
        assert!(
            parsed
                .receipts
                .iter()
                .any(|receipt| receipt.event_name == "player_death" && !receipt.fields.is_empty())
        );
        let ordinals: Vec<u64> = parsed
            .receipts
            .iter()
            .map(|r| r.ingestion_ordinal.get())
            .collect();
        assert!(ordinals.windows(2).all(|w| w[0] < w[1]));
        let reparsed =
            parse_with_pinned_demoparser(Path::new(&path)).expect("repeat parse must succeed");
        assert_eq!(parsed, reparsed, "canonical evidence must be deterministic");
        eprintln!(
            "fixture={} participants={} rounds={} events={} snapshots={} elapsed_ms={} map={:?}",
            path,
            parsed.participants.len(),
            parsed.rounds.len(),
            parsed.events.len(),
            parsed.player_snapshots.len(),
            start.elapsed().as_millis(),
            parsed.metadata.map
        );
    }
}
