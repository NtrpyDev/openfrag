//! Durable local Demo import primitives. This crate owns orchestration, not parser output.

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
    pub steam_id: u64,
    pub name: Option<String>,
    pub team: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEvent {
    pub name: String,
    pub tick: i32,
    pub ingestion_ordinal: u64,
    pub fields: BTreeMap<String, String>,
    pub raw_fields: BTreeMap<String, serde_json::Value>,
}
impl ParsedEvent {
    pub fn u64_field(&self, name: &str) -> Option<u64> {
        let value = self.raw_fields.get(name)?;
        value.as_u64().or_else(|| value.as_str()?.parse().ok())
    }
    pub fn i64_field(&self, name: &str) -> Option<i64> {
        self.raw_fields.get(name)?.as_i64()
    }
    pub fn bool_field(&self, name: &str) -> Option<bool> {
        self.raw_fields.get(name)?.as_bool()
    }
    pub fn string_field(&self, name: &str) -> Option<&str> {
        self.raw_fields.get(name)?.as_str()
    }
    pub fn attacker(&self) -> Option<u64> {
        self.u64_field("attacker_steamid")
            .or_else(|| self.u64_field("attacker"))
    }
    pub fn victim(&self) -> Option<u64> {
        self.u64_field("user_steamid")
            .or_else(|| self.u64_field("userid"))
            .or_else(|| self.u64_field("victim"))
    }
    pub fn assister(&self) -> Option<u64> {
        self.u64_field("assister")
    }
    pub fn assisted_flash(&self) -> Option<bool> {
        self.bool_field("assistedflash")
    }
    pub fn damage_health(&self) -> Option<i64> {
        self.i64_field("dmg_health")
    }
    pub fn weapon(&self) -> Option<&str> {
        self.string_field("weapon")
    }
    pub fn winner(&self) -> Option<i64> {
        self.i64_field("winner")
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReceipt {
    pub ingestion_ordinal: u64,
    pub event_name: String,
    pub tick: i32,
    pub fields: BTreeMap<String, String>,
    pub raw_fields: BTreeMap<String, serde_json::Value>,
    pub evidence_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRound {
    pub number: u64,
    pub end_tick: i32,
    pub winner: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub tick: i32,
    pub ingestion_ordinal: u64,
    pub steam_id: u64,
    pub entity_id: Option<i32>,
    pub team: Option<i32>,
    pub health: Option<i32>,
    pub alive: Option<bool>,
    pub life_state: Option<i32>,
    pub round_counter: Option<i32>,
    pub raw_properties: BTreeMap<String, serde_json::Value>,
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
pub const DEMOPARSER_BUILD: &str = "parser-0.1.1/csgoproto-0.1.5";
pub const QUERY_PLAN_VERSION: &str = "openfrag-evidence-query-2";
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
pub const EVIDENCE_SEMANTICS_EPOCH: &str = "openfrag-evidence-1";
pub const GENERATED_PROTO_BUILD: &str = "csgoproto-0.1.5";

pub fn query_plan_hash() -> String {
    let canonical = format!(
        "{}\nevents:{}\nproperties:{}",
        QUERY_PLAN_VERSION,
        EVENT_QUERY.join(","),
        PROPERTY_QUERY.join(",")
    );
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParserError {
    Io(String),
    Parse(String),
    Quarantined { reason: &'static str },
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

#[cfg(feature = "demoparser")]
pub fn parse_with_pinned_demoparser(path: &Path) -> Result<ParsedOutput, ParserError> {
    use ahash::AHashMap;
    use demoparser_parser::second_pass::parser_settings::create_huffman_lookup_table;
    use demoparser_parser::{
        first_pass::parser_settings::ParserInputs,
        parse_demo::{Parser, ParsingMode},
        second_pass::variants::{OutputSerdeHelperStruct, soa_to_aos},
    };
    let bytes = std::fs::read(path).map_err(|e| ParserError::Io(e.to_string()))?;
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
    let mut parser = Parser::new(inputs, ParsingMode::Normal);
    let out = parser
        .parse_demo(&bytes)
        .map_err(|e| ParserError::Parse(e.to_string()))?;
    let mut evidence_ticks: Vec<i32> = out.game_events.iter().map(|event| event.tick).collect();
    evidence_ticks.sort_unstable();
    evidence_ticks.dedup();
    let snapshot_inputs = ParserInputs {
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
        wanted_ticks: evidence_ticks,
        wanted_events: vec![],
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
    let mut snapshot_parser = Parser::new(snapshot_inputs, ParsingMode::Normal);
    let snapshot_out = snapshot_parser
        .parse_demo(&bytes)
        .map_err(|e| ParserError::Parse(e.to_string()))?;
    let player_snapshots = soa_to_aos(OutputSerdeHelperStruct {
        prop_infos: snapshot_out.prop_controller.prop_infos.clone(),
        inner: snapshot_out.df.into(),
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
            tick: i64_value("tick")? as i32,
            ingestion_ordinal: ordinal as u64,
            steam_id,
            entity_id: i64_value("entity_id").map(|v| v as i32),
            team: i64_value("team_num").map(|v| v as i32),
            health: i64_value("health").map(|v| v as i32),
            alive: raw_properties
                .get("is_alive")
                .and_then(serde_json::Value::as_bool),
            life_state: i64_value("life_state").map(|v| v as i32),
            round_counter: i64_value("total_rounds_played").map(|v| v as i32),
            raw_properties,
        })
    })
    .collect::<Vec<_>>();
    let header = out.header.unwrap_or_default();
    let participants: Vec<Participant> = out
        .roster
        .iter()
        .filter_map(|p| {
            p.steamid.map(|id| Participant {
                steam_id: id,
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
        let parsed = ParsedEvent {
            name: event.name.clone(),
            tick: event.tick,
            ingestion_ordinal: ordinal as u64,
            fields: fields.clone(),
            raw_fields: raw_fields.clone(),
        };
        if event.name == "round_end" {
            round_no += 1;
            rounds.push(ParsedRound {
                number: round_no,
                end_tick: event.tick,
                winner: fields.get("winner").cloned(),
            });
        }
        let evidence_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(ordinal, &event.name, event.tick, &raw_fields))
                    .map_err(|e| ParserError::Parse(e.to_string()))?
            )
        );
        receipts.push(EventReceipt {
            ingestion_ordinal: ordinal as u64,
            event_name: event.name.clone(),
            tick: event.tick,
            fields: fields.clone(),
            raw_fields,
            evidence_sha256,
        });
        events.push(parsed);
    }
    let suspicious_empty = participants.is_empty()
        || rounds.is_empty()
        || !events.iter().any(|e| e.name == "player_death");
    if participants.is_empty() {
        return Err(ParserError::Quarantined {
            reason: "no participants",
        });
    }
    if rounds.is_empty() {
        return Err(ParserError::Quarantined {
            reason: "no canonical round_end events",
        });
    }
    if !events.iter().any(|e| e.name == "player_death") {
        return Err(ParserError::Quarantined {
            reason: "no requested player_death events",
        });
    }
    for required in ["round_freeze_end", "player_hurt"] {
        if !events.iter().any(|event| event.name == required) {
            return Err(ParserError::Quarantined {
                reason: "missing required rating evidence event",
            });
        }
    }
    if player_snapshots.is_empty() {
        return Err(ParserError::Quarantined {
            reason: "no requested player snapshots",
        });
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
        hasher.update(&buf[..n]);
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
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|_| ErrorCode::Io)?;
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
            hasher.update(&buf[..n]);
            output.write_all(&buf[..n]).map_err(|_| ErrorCode::Io)?;
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
    }

    #[cfg(feature = "demoparser")]
    #[test]
    fn pinned_fixture_adapter_smoke_when_requested() {
        let Ok(path) = std::env::var("OPENFRAG_DEMOPARSER_FIXTURE") else {
            return;
        };
        let start = std::time::Instant::now();
        let parsed =
            parse_with_pinned_demoparser(Path::new(&path)).expect("pinned fixture must parse");
        assert!(!parsed.suspicious_empty);
        assert_eq!(parsed.metadata.map.as_deref(), Some("de_mirage"));
        assert_eq!(parsed.participants.len(), 10);
        assert_eq!(parsed.rounds.len(), 10);
        assert_eq!(parsed.events.len(), 366);
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|e| e.name == "player_hurt")
                .count(),
            264
        );
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|e| e.name == "player_death")
                .count(),
            73
        );
        assert_eq!(
            parsed
                .events
                .iter()
                .filter(|e| e.name == "round_freeze_end")
                .count(),
            9
        );
        let hurt = parsed
            .events
            .iter()
            .find(|e| e.name == "player_hurt")
            .unwrap();
        assert_eq!(hurt.attacker(), Some(76561197964020430));
        assert_eq!(hurt.victim(), Some(76561198073049527));
        assert_eq!(hurt.damage_health(), Some(100));
        assert_eq!(hurt.weapon(), Some("p250"));
        let death = parsed
            .events
            .iter()
            .find(|e| e.name == "player_death")
            .unwrap();
        assert_eq!(death.attacker(), Some(76561197964020430));
        assert_eq!(death.victim(), Some(76561198073049527));
        assert_eq!(death.weapon(), Some("p250"));
        assert!(!parsed.player_snapshots.is_empty());
        assert!(parsed.player_snapshots.iter().all(|snapshot| {
            parsed
                .participants
                .iter()
                .any(|participant| participant.steam_id == snapshot.steam_id)
        }));
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
                .events
                .iter()
                .any(|e| e.name == "player_death" && !e.fields.is_empty())
        );
        let ordinals: Vec<u64> = parsed
            .receipts
            .iter()
            .map(|r| r.ingestion_ordinal)
            .collect();
        assert!(ordinals.windows(2).all(|w| w[0] < w[1]));
        eprintln!(
            "fixture={} participants={} rounds={} events={} elapsed_ms={} map={:?}",
            path,
            parsed.participants.len(),
            parsed.rounds.len(),
            parsed.events.len(),
            start.elapsed().as_millis(),
            parsed.metadata.map
        );
    }
}
