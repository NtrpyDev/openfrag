//! Deterministic evidence conversion between parsed demos and the rating domain.

use openfrag_domain::{
    CalculationIdentity, MatchIrregularities, RatingEvidenceBundle, RatingInput, RatingInputError,
    RatingMetrics, RatingReceipt, ReceiptEvidenceSet, calculate_rating,
};
use openfrag_import::{
    DemoTick, NormalizedEvent, NormalizedEventKind, ParsedEvent, ParsedOutput, PlayerSnapshot,
    SteamId,
};

const UTILITY_WEAPONS: &[&str] = &["hegrenade", "inferno", "molotov", "incgrenade"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalysisUnavailable {
    MissingTickRate,
    MissingGameBuild,
    MissingRequiredStream(&'static str),
    MissingLocalParticipant,
    MissingIdentity,
    MissingTeamOrLiveness,
    MalformedOrdering,
    MalformedEvidence,
    Domain(RatingInputError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoundExclusion {
    Warmup,
    MissingFreezeState,
    LocalPlayerNotAlive,
    InvalidTeam,
    MissingCanonicalEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoundLedger {
    pub number: u64,
    pub freeze_tick: i32,
    pub end_tick: i32,
    pub winner: i64,
    pub local_team: i32,
    pub eligible: bool,
    pub exclusion: Option<RoundExclusion>,
    pub ordered_evidence_hashes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisReceipt {
    pub rounds: Vec<RoundLedger>,
    pub rating_input: RatingInput,
    pub rating_receipt: RatingReceipt,
}

/// Applies the immutable domain formula to a fully verified input.
///
/// # Errors
/// Returns the domain validation error when the supplied metrics are inconsistent.
pub fn rate_verified_input(input: RatingInput) -> Result<RatingReceipt, RatingInputError> {
    calculate_rating(input)
}

/// Converts ordered parser evidence for one local player into canonical ledgers and a receipt.
///
/// # Errors
/// Returns a typed evidence failure whenever required identity, ordering, state, or timing is absent.
#[allow(clippy::too_many_lines)]
pub fn analyze(
    parsed: &ParsedOutput,
    local_steam_id: u64,
) -> Result<AnalysisReceipt, AnalysisUnavailable> {
    let local_steam_id = SteamId::new(local_steam_id);
    let tick_rate = parsed
        .metadata
        .tick_rate
        .as_deref()
        .ok_or(AnalysisUnavailable::MissingTickRate)?
        .parse::<u32>()
        .map_err(|_| AnalysisUnavailable::MissingTickRate)?;
    if tick_rate == 0 {
        return Err(AnalysisUnavailable::MissingTickRate);
    }
    let game_build = parsed
        .metadata
        .patch_build
        .clone()
        .ok_or(AnalysisUnavailable::MissingGameBuild)?;
    if !parsed
        .participants
        .iter()
        .any(|p| p.steam_id == local_steam_id)
    {
        return Err(AnalysisUnavailable::MissingLocalParticipant);
    }
    verify_order(parsed)?;
    for (kind, stream) in [
        (NormalizedEventKind::RoundFreezeEnd, "round_freeze_end"),
        (NormalizedEventKind::RoundEnd, "round_end"),
        (NormalizedEventKind::PlayerHurt, "player_hurt"),
        (NormalizedEventKind::PlayerDeath, "player_death"),
    ] {
        if !parsed.events.iter().any(|event| event.kind() == kind) {
            return Err(AnalysisUnavailable::MissingRequiredStream(stream));
        }
    }

    let rounds = build_rounds(parsed, local_steam_id)?;
    let eligible: Vec<&RoundLedger> = rounds.iter().filter(|round| round.eligible).collect();
    let mut metrics = RatingMetrics {
        eligible_rounds: u32::try_from(eligible.len())
            .map_err(|_| AnalysisUnavailable::MalformedEvidence)?,
        direct_damage: 0,
        kills: 0,
        deaths: 0,
        opening_wins: 0,
        opening_losses: 0,
        trade_kills: 0,
        utility_damage: 0,
        flash_assists: 0,
        clutch_wins: 0,
        clutch_opportunities: 0,
    };
    for round in &eligible {
        score_round(parsed, round, local_steam_id, tick_rate, &mut metrics)?;
    }
    let identity = CalculationIdentity::ofr_v1(
        &parsed.identity.source_sha256,
        &parsed.identity.parser_commit,
        &parsed.identity.parser_build,
        &parsed.identity.generated_proto_build,
        &parsed.identity.requested_schema_hash,
        &parsed.identity.metric_definition_version,
        &parsed.identity.evidence_semantics_epoch,
    );
    let eligible_ids = rounds
        .iter()
        .filter(|r| r.eligible)
        .flat_map(|r| r.ordered_evidence_hashes.clone())
        .collect::<Vec<_>>();
    let excluded_ids = rounds
        .iter()
        .filter(|r| !r.eligible)
        .flat_map(|r| r.ordered_evidence_hashes.clone())
        .collect::<Vec<_>>();
    let component_ids = parsed
        .receipts
        .iter()
        .map(|receipt| receipt.evidence_sha256.clone())
        .collect::<Vec<_>>();
    let nonempty_excluded = if excluded_ids.is_empty() {
        vec![format!("none:{}", parsed.identity.source_sha256)]
    } else {
        excluded_ids
    };
    let evidence = RatingEvidenceBundle {
        eligible_round_ledger: ReceiptEvidenceSet::new(eligible_ids),
        excluded_round_ledger: ReceiptEvidenceSet::new(nonempty_excluded),
        direct_damage: ReceiptEvidenceSet::new(component_ids.clone()),
        frag_balance: ReceiptEvidenceSet::new(component_ids.clone()),
        opening_duels: ReceiptEvidenceSet::new(component_ids.clone()),
        trade_kills: ReceiptEvidenceSet::new(component_ids.clone()),
        utility: ReceiptEvidenceSet::new(component_ids.clone()),
        clutch_conversion: ReceiptEvidenceSet::new(component_ids),
    };
    let short_roster_rounds = rounds
        .iter()
        .filter(|round| round.eligible)
        .filter(|round| {
            [2, 3].into_iter().any(|team| {
                parsed
                    .participants
                    .iter()
                    .filter(|participant| participant.team == Some(team))
                    .filter(|participant| {
                        snapshot_at(
                            parsed,
                            participant.steam_id,
                            DemoTick::new(round.freeze_tick),
                        )
                        .and_then(|snapshot| snapshot.alive)
                            == Some(true)
                    })
                    .count()
                    < 5
            })
        })
        .count();
    let irregularities = MatchIrregularities {
        missed_live_round: rounds
            .iter()
            .any(|round| round.exclusion == Some(RoundExclusion::MissingFreezeState)),
        roster_imbalanced: short_roster_rounds > 1,
        surrender: false,
        forfeit_without_canonical_winner: false,
        overtime: metrics.eligible_rounds > 24,
    };
    let rating_input = RatingInput {
        identity,
        game_build,
        metrics,
        irregularities,
        evidence_failure: None,
        evidence,
    };
    let rating_receipt =
        calculate_rating(rating_input.clone()).map_err(AnalysisUnavailable::Domain)?;
    Ok(AnalysisReceipt {
        rounds,
        rating_input,
        rating_receipt,
    })
}

fn verify_order(parsed: &ParsedOutput) -> Result<(), AnalysisUnavailable> {
    if parsed.events.windows(2).any(|pair| {
        (pair[0].tick, pair[0].ingestion_ordinal) >= (pair[1].tick, pair[1].ingestion_ordinal)
    }) || parsed
        .receipts
        .windows(2)
        .any(|pair| pair[0].ingestion_ordinal >= pair[1].ingestion_ordinal)
        || parsed
            .player_snapshots
            .windows(2)
            .any(|pair| pair[0].ingestion_ordinal >= pair[1].ingestion_ordinal)
    {
        return Err(AnalysisUnavailable::MalformedOrdering);
    }
    Ok(())
}

fn build_rounds(
    parsed: &ParsedOutput,
    local: SteamId,
) -> Result<Vec<RoundLedger>, AnalysisUnavailable> {
    let freezes: Vec<&ParsedEvent> = parsed
        .events
        .iter()
        .filter(|event| matches!(event.event, NormalizedEvent::RoundFreezeEnd(_)))
        .collect();
    let ends: Vec<&ParsedEvent> = parsed
        .events
        .iter()
        .filter(|event| matches!(event.event, NormalizedEvent::RoundEnd(_)))
        .collect();
    let mut rounds = Vec::new();
    for (index, freeze) in freezes.iter().copied().enumerate() {
        let next_freeze_tick = freezes
            .get(index + 1)
            .map_or(DemoTick::new(i32::MAX), |event| event.tick);
        let matching: Vec<&ParsedEvent> = ends
            .iter()
            .copied()
            .filter(|end| end.tick > freeze.tick && end.tick < next_freeze_tick)
            .collect();
        if matching.len() != 1 {
            return Err(AnalysisUnavailable::MalformedEvidence);
        }
        let end = matching[0];
        let snapshot = snapshot_at(parsed, local, freeze.tick);
        let snapshot = snapshot.filter(|state| state.tick == freeze.tick);
        let NormalizedEvent::RoundFreezeEnd(freeze_event) = &freeze.event else {
            return Err(AnalysisUnavailable::MalformedEvidence);
        };
        let warmup = freeze_event.warmup;
        let (team, alive, exclusion) = match snapshot {
            _ if warmup => (0, false, Some(RoundExclusion::Warmup)),
            None => return Err(AnalysisUnavailable::MissingTeamOrLiveness),
            Some(state) if !state.alive.unwrap_or(false) => (
                state.team.unwrap_or(0),
                false,
                Some(RoundExclusion::LocalPlayerNotAlive),
            ),
            Some(state) if !matches!(state.team, Some(2 | 3)) => (
                state.team.unwrap_or(0),
                state.alive.unwrap_or(false),
                Some(RoundExclusion::InvalidTeam),
            ),
            Some(state) => (state.team.unwrap_or(0), true, None),
        };
        if exclusion.is_none() {
            let start_counter = snapshot
                .and_then(|state| state.round_counter)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            let end_counter = snapshot_at(parsed, local, end.tick)
                .and_then(|state| state.round_counter)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            if end_counter != start_counter + 1 {
                return Err(AnalysisUnavailable::MalformedEvidence);
            }
        }
        let NormalizedEvent::RoundEnd(end_event) = &end.event else {
            return Err(AnalysisUnavailable::MalformedEvidence);
        };
        let winner = i64::from(
            end_event
                .winner
                .ok_or(AnalysisUnavailable::MalformedEvidence)?,
        );
        let hashes = parsed
            .receipts
            .iter()
            .filter(|r| r.tick >= freeze.tick && r.tick <= end.tick)
            .map(|r| r.evidence_sha256.clone())
            .collect();
        rounds.push(RoundLedger {
            number: (index + 1) as u64,
            freeze_tick: freeze.tick.get(),
            end_tick: end.tick.get(),
            winner,
            local_team: team,
            eligible: alive && exclusion.is_none(),
            exclusion,
            ordered_evidence_hashes: hashes,
        });
    }
    if ends.len() != rounds.len() {
        return Err(AnalysisUnavailable::MalformedEvidence);
    }
    if rounds.is_empty() {
        return Err(AnalysisUnavailable::MissingRequiredStream(
            "eligible round ledger",
        ));
    }
    Ok(rounds)
}

#[allow(clippy::collapsible_if, clippy::too_many_lines)]
fn score_round(
    parsed: &ParsedOutput,
    round: &RoundLedger,
    local: SteamId,
    tick_rate: u32,
    metrics: &mut RatingMetrics,
) -> Result<(), AnalysisUnavailable> {
    let events: Vec<&ParsedEvent> = parsed
        .events
        .iter()
        .filter(|event| {
            event.tick >= DemoTick::new(round.freeze_tick)
                && event.tick <= DemoTick::new(round.end_tick)
        })
        .collect();
    let deaths: Vec<&ParsedEvent> = events
        .iter()
        .copied()
        .filter(|event| matches!(event.event, NormalizedEvent::PlayerDeath(_)))
        .collect();
    let mut opening_seen = false;
    for event in &events {
        if let NormalizedEvent::PlayerHurt(hurt) = &event.event
            && hurt.attacker == Some(local)
        {
            let victim = hurt.victim.ok_or(AnalysisUnavailable::MissingIdentity)?;
            let attacker_team = team_at(parsed, local, event.tick)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            let victim_team = team_at(parsed, victim, event.tick)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            if attacker_team != victim_team && victim != local {
                let reported = u32::try_from(
                    hurt.damage_health
                        .ok_or(AnalysisUnavailable::MalformedEvidence)?,
                )
                .map_err(|_| AnalysisUnavailable::MalformedEvidence)?;
                let health = snapshot_at(parsed, victim, event.tick)
                    .and_then(|s| s.health)
                    .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
                let scored = reported.min(
                    u32::try_from(health).map_err(|_| AnalysisUnavailable::MalformedEvidence)?,
                );
                if hurt
                    .weapon
                    .as_deref()
                    .is_some_and(|weapon| UTILITY_WEAPONS.contains(&weapon))
                {
                    metrics.utility_damage += scored;
                } else {
                    metrics.direct_damage += scored;
                }
            }
        }
        if let NormalizedEvent::PlayerDeath(death) = &event.event {
            let victim = death.victim.ok_or(AnalysisUnavailable::MissingIdentity)?;
            let attacker = death.attacker;
            if victim == local {
                metrics.deaths += 1;
            }
            if attacker == Some(local)
                && team_at(parsed, local, event.tick) != team_at(parsed, victim, event.tick)
            {
                metrics.kills += 1;
            }
            if !opening_seen {
                if let Some(attacker) = attacker.filter(|id| *id != victim) {
                    if team_at(parsed, attacker, event.tick)
                        .zip(team_at(parsed, victim, event.tick))
                        .is_some_and(|(a, v)| a != v)
                    {
                        opening_seen = true;
                        if attacker == local {
                            metrics.opening_wins += 1;
                        }
                        if victim == local {
                            metrics.opening_losses += 1;
                        }
                    }
                }
            }
            if death.assisted_flash && death.assister == Some(local) {
                let attacker = attacker.ok_or(AnalysisUnavailable::MissingIdentity)?;
                if team_at(parsed, attacker, event.tick) == team_at(parsed, local, event.tick) {
                    metrics.flash_assists += 1;
                } else {
                    return Err(AnalysisUnavailable::MalformedEvidence);
                }
            }
        }
    }
    let window = i32::try_from(tick_rate.saturating_mul(5))
        .map_err(|_| AnalysisUnavailable::MalformedEvidence)?;
    for kill in deaths.iter().filter(|event| {
        matches!(&event.event, NormalizedEvent::PlayerDeath(death) if death.attacker == Some(local))
    }) {
        let NormalizedEvent::PlayerDeath(kill_event) = &kill.event else {
            return Err(AnalysisUnavailable::MalformedEvidence);
        };
        let victim = kill_event
            .victim
            .ok_or(AnalysisUnavailable::MissingIdentity)?;
        if deaths.iter().rev().any(|prior| {
            let NormalizedEvent::PlayerDeath(prior_event) = &prior.event else {
                return false;
            };
            prior.tick <= kill.tick
                && kill.tick.get() - prior.tick.get() <= window
                && prior_event.attacker == Some(victim)
                && prior_event
                    .victim
                    .is_some_and(|mate| team_at(parsed, mate, prior.tick) == Some(round.local_team))
        }) {
            metrics.trade_kills += 1;
        }
    }
    let transition_ticks = parsed
        .player_snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.tick >= DemoTick::new(round.freeze_tick)
                && snapshot.tick <= DemoTick::new(round.end_tick)
        })
        .map(|snapshot| snapshot.tick);
    for tick in transition_ticks {
        let local_alive = snapshot_at(parsed, local, tick)
            .and_then(|s| s.alive)
            .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
        let teammates_alive = parsed
            .participants
            .iter()
            .filter(|p| p.steam_id != local && p.team == Some(round.local_team))
            .filter(|p| snapshot_at(parsed, p.steam_id, tick).and_then(|s| s.alive) == Some(true))
            .count();
        let enemies_alive = parsed
            .participants
            .iter()
            .filter(|p| {
                p.team
                    .is_some_and(|team| team != round.local_team && matches!(team, 2 | 3))
            })
            .filter(|p| snapshot_at(parsed, p.steam_id, tick).and_then(|s| s.alive) == Some(true))
            .count();
        if local_alive && teammates_alive == 0 && enemies_alive > 0 {
            metrics.clutch_opportunities += 1;
            if i64::from(round.local_team) == round.winner {
                metrics.clutch_wins += 1;
            }
            break;
        }
    }
    Ok(())
}

fn snapshot_at(
    parsed: &ParsedOutput,
    steam_id: SteamId,
    tick: DemoTick,
) -> Option<&PlayerSnapshot> {
    parsed
        .player_snapshots
        .iter()
        .filter(|s| s.steam_id == steam_id && s.tick <= tick)
        .max_by_key(|s| (s.tick, s.ingestion_ordinal))
}

fn team_at(parsed: &ParsedOutput, steam_id: SteamId, tick: DemoTick) -> Option<i32> {
    snapshot_at(parsed, steam_id, tick).and_then(|snapshot| snapshot.team)
}

pub mod fixtures {
    use openfrag_import::{CalculationIdentity as ImportIdentity, DemoMetadata, ParsedOutput};
    #[cfg(feature = "acceptance-fixtures")]
    use openfrag_import::{
        EventReceipt, ParsedEvent, ParsedRound, Participant, PlayerSnapshot, SnapshotPhase,
    };
    #[cfg(feature = "acceptance-fixtures")]
    use serde_json::json;
    #[cfg(feature = "acceptance-fixtures")]
    use std::collections::BTreeMap;

    #[must_use]
    pub fn minimal_parsed_output_without_tick_rate() -> ParsedOutput {
        ParsedOutput {
            metadata: DemoMetadata {
                map: Some("de_fixture".into()),
                patch_build: Some("fixture-build".into()),
                demo_stamp: None,
                server: None,
                game_directory: None,
                tick_rate: None,
                tick_rate_unavailable_reason: Some("fixture".into()),
            },
            participants: vec![],
            rounds: vec![],
            events: vec![],
            receipts: vec![],
            player_snapshots: vec![],
            suspicious_empty: false,
            identity: ImportIdentity {
                source_sha256: "demo".into(),
                parser_commit: "commit".into(),
                parser_build: "parser".into(),
                generated_proto_build: "proto".into(),
                requested_schema_hash: "query".into(),
                metric_definition_version: "metrics".into(),
                formula_version: "ofr-1.0.0".into(),
                evidence_semantics_epoch: "epoch".into(),
            },
        }
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
    #[cfg(feature = "acceptance-fixtures")]
    pub fn complete_parsed_output() -> ParsedOutput {
        let local = 76_561_197_964_020_430_u64;
        let participants = (0_u64..10)
            .map(|index| Participant {
                steam_id: (if index == 0 { local } else { local + index }).into(),
                name: Some(format!("p{index}")),
                team: Some(if index < 5 { 2 } else { 3 }),
            })
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        let mut snapshots = Vec::new();
        let mut rounds = Vec::new();
        let mut ordinal = 0_u64;
        let mut snapshot_ordinal = 0_u64;
        for round in 0_i32..12 {
            let base = round * 1_000;
            for participant in &participants {
                snapshots.push(PlayerSnapshot {
                    tick: (base + 10).into(),
                    ingestion_ordinal: snapshot_ordinal.into(),
                    phase: SnapshotPhase::RequestedTick,
                    steam_id: participant.steam_id,
                    entity_id: None,
                    team: participant.team,
                    health: Some(100),
                    alive: Some(true),
                    life_state: Some(0),
                    round_counter: Some(round),
                });
                snapshot_ordinal += 1;
            }
            let mut push =
                |name: &str, tick: i32, raw_fields: BTreeMap<String, serde_json::Value>| {
                    events.push(
                        ParsedEvent::from_raw(name, tick, ordinal, &raw_fields)
                            .expect("fixture event must normalize"),
                    );
                    ordinal += 1;
                };
            push(
                "round_freeze_end",
                base + 10,
                BTreeMap::from([("warmup".into(), json!(false))]),
            );
            push(
                "player_hurt",
                base + 20,
                BTreeMap::from([
                    ("attacker_steamid".into(), json!(local.to_string())),
                    ("user_steamid".into(), json!((local + 5).to_string())),
                    ("dmg_health".into(), json!(80)),
                    ("weapon".into(), json!("ak47")),
                ]),
            );
            push(
                "player_death",
                base + 30,
                BTreeMap::from([
                    ("attacker_steamid".into(), json!((local + 6).to_string())),
                    ("user_steamid".into(), json!((local + 1).to_string())),
                    ("weapon".into(), json!("ak47")),
                    ("assistedflash".into(), json!(false)),
                ]),
            );
            push(
                "round_end",
                base + 90,
                BTreeMap::from([("winner".into(), json!(2))]),
            );
            for participant in &participants {
                snapshots.push(PlayerSnapshot {
                    tick: (base + 90).into(),
                    ingestion_ordinal: snapshot_ordinal.into(),
                    phase: SnapshotPhase::RequestedTick,
                    steam_id: participant.steam_id,
                    entity_id: None,
                    team: participant.team,
                    health: Some(100),
                    alive: Some(true),
                    life_state: Some(0),
                    round_counter: Some(round + 1),
                });
                snapshot_ordinal += 1;
            }
            rounds.push(ParsedRound {
                number: u64::try_from(round + 1)
                    .expect("positive fixture round")
                    .into(),
                end_tick: (base + 90).into(),
                winner: Some(2),
            });
        }
        let receipts = events
            .iter()
            .map(|event| EventReceipt {
                ingestion_ordinal: event.ingestion_ordinal,
                event_name: event.name().into(),
                tick: event.tick,
                fields: BTreeMap::new(),
                raw_fields: BTreeMap::new(),
                evidence_sha256: format!("evidence-{:04}", event.ingestion_ordinal.get()),
            })
            .collect();
        ParsedOutput {
            metadata: DemoMetadata {
                map: Some("de_fixture".into()),
                patch_build: Some("fixture-build".into()),
                demo_stamp: None,
                server: None,
                game_directory: None,
                tick_rate: Some("64".into()),
                tick_rate_unavailable_reason: None,
            },
            participants,
            rounds,
            events,
            receipts,
            player_snapshots: snapshots,
            suspicious_empty: false,
            identity: ImportIdentity {
                source_sha256: "available-demo".into(),
                parser_commit: "commit".into(),
                parser_build: "parser".into(),
                generated_proto_build: "proto".into(),
                requested_schema_hash: "query".into(),
                metric_definition_version: "metrics".into(),
                formula_version: "ofr-1.0.0".into(),
                evidence_semantics_epoch: "epoch".into(),
            },
        }
    }
}
