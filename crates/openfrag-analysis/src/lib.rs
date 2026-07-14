//! Deterministic evidence conversion between parsed demos and the rating domain.

use openfrag_domain::{
    calculate_rating, CalculationIdentity, MatchIrregularities, RatingEvidenceBundle, RatingInput,
    RatingInputError, RatingMetrics, RatingReceipt, ReceiptEvidenceSet,
};
use openfrag_import::{ParsedEvent, ParsedOutput, PlayerSnapshot};

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

pub fn rate_verified_input(input: RatingInput) -> Result<RatingReceipt, RatingInputError> {
    calculate_rating(input)
}

pub fn analyze(
    parsed: &ParsedOutput,
    local_steam_id: u64,
) -> Result<AnalysisReceipt, AnalysisUnavailable> {
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
    for stream in [
        "round_freeze_end",
        "round_end",
        "player_hurt",
        "player_death",
    ] {
        if !parsed.events.iter().any(|event| event.name == stream) {
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
    let rating_input = RatingInput {
        identity,
        game_build,
        metrics,
        irregularities: MatchIrregularities::default(),
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
    local: u64,
) -> Result<Vec<RoundLedger>, AnalysisUnavailable> {
    let freezes = parsed
        .events
        .iter()
        .filter(|e| e.name == "round_freeze_end");
    let ends: Vec<&ParsedEvent> = parsed
        .events
        .iter()
        .filter(|e| e.name == "round_end")
        .collect();
    let mut rounds = Vec::new();
    for (index, freeze) in freezes.enumerate() {
        let Some(end) = ends.iter().copied().find(|end| end.tick > freeze.tick) else {
            continue;
        };
        let snapshot = snapshot_at(parsed, local, freeze.tick);
        let (team, alive, exclusion) = match snapshot {
            None => (0, false, Some(RoundExclusion::MissingFreezeState)),
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
        let winner = end.winner().ok_or(AnalysisUnavailable::MalformedEvidence)?;
        let hashes = parsed
            .receipts
            .iter()
            .filter(|r| r.tick >= freeze.tick && r.tick <= end.tick)
            .map(|r| r.evidence_sha256.clone())
            .collect();
        rounds.push(RoundLedger {
            number: (index + 1) as u64,
            freeze_tick: freeze.tick,
            end_tick: end.tick,
            winner,
            local_team: team,
            eligible: alive && exclusion.is_none(),
            exclusion,
            ordered_evidence_hashes: hashes,
        });
    }
    if rounds.is_empty() {
        return Err(AnalysisUnavailable::MissingRequiredStream(
            "eligible round ledger",
        ));
    }
    Ok(rounds)
}

fn score_round(
    parsed: &ParsedOutput,
    round: &RoundLedger,
    local: u64,
    tick_rate: u32,
    metrics: &mut RatingMetrics,
) -> Result<(), AnalysisUnavailable> {
    let events: Vec<&ParsedEvent> = parsed
        .events
        .iter()
        .filter(|e| e.tick >= round.freeze_tick && e.tick <= round.end_tick)
        .collect();
    let deaths: Vec<&ParsedEvent> = events
        .iter()
        .copied()
        .filter(|e| e.name == "player_death")
        .collect();
    let mut opening_seen = false;
    for event in &events {
        if event.name == "player_hurt" && event.attacker() == Some(local) {
            let victim = event.victim().ok_or(AnalysisUnavailable::MissingIdentity)?;
            let attacker_team = team_at(parsed, local, event.tick)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            let victim_team = team_at(parsed, victim, event.tick)
                .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
            if attacker_team != victim_team && victim != local {
                let reported = u32::try_from(
                    event
                        .damage_health()
                        .ok_or(AnalysisUnavailable::MalformedEvidence)?,
                )
                .map_err(|_| AnalysisUnavailable::MalformedEvidence)?;
                let health = snapshot_at(parsed, victim, event.tick)
                    .and_then(|s| s.health)
                    .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
                let scored = reported.min(
                    u32::try_from(health).map_err(|_| AnalysisUnavailable::MalformedEvidence)?,
                );
                if event
                    .weapon()
                    .is_some_and(|weapon| UTILITY_WEAPONS.contains(&weapon))
                {
                    metrics.utility_damage += scored;
                } else {
                    metrics.direct_damage += scored;
                }
            }
        }
        if event.name == "player_death" {
            let victim = event.victim().ok_or(AnalysisUnavailable::MissingIdentity)?;
            let attacker = event.attacker();
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
            let assister = event
                .u64_field("assister_steamid")
                .or_else(|| event.assister());
            if event.assisted_flash() == Some(true) && assister == Some(local) {
                metrics.flash_assists += 1;
            }
        }
    }
    let window = i32::try_from(tick_rate.saturating_mul(5))
        .map_err(|_| AnalysisUnavailable::MalformedEvidence)?;
    for kill in deaths.iter().filter(|e| e.attacker() == Some(local)) {
        let victim = kill.victim().ok_or(AnalysisUnavailable::MissingIdentity)?;
        if deaths.iter().rev().any(|prior| {
            prior.tick <= kill.tick
                && kill.tick - prior.tick <= window
                && prior.attacker() == Some(victim)
                && prior
                    .victim()
                    .is_some_and(|mate| team_at(parsed, mate, prior.tick) == Some(round.local_team))
        }) {
            metrics.trade_kills += 1;
        }
    }
    let local_alive = snapshot_at(parsed, local, round.freeze_tick)
        .and_then(|s| s.alive)
        .ok_or(AnalysisUnavailable::MissingTeamOrLiveness)?;
    if local_alive {
        let teammates_alive = parsed
            .participants
            .iter()
            .filter(|p| p.steam_id != local && p.team == Some(round.local_team))
            .filter(|p| {
                snapshot_at(parsed, p.steam_id, round.end_tick)
                    .and_then(|s| s.alive)
                    .unwrap_or(false)
            })
            .count();
        let enemies_alive = parsed
            .participants
            .iter()
            .filter(|p| {
                p.team
                    .is_some_and(|team| team != round.local_team && matches!(team, 2 | 3))
            })
            .filter(|p| {
                snapshot_at(parsed, p.steam_id, round.end_tick)
                    .and_then(|s| s.alive)
                    .unwrap_or(false)
            })
            .count();
        if teammates_alive == 0 && enemies_alive > 0 {
            metrics.clutch_opportunities += 1;
            if i64::from(round.local_team) == round.winner {
                metrics.clutch_wins += 1;
            }
        }
    }
    Ok(())
}

fn snapshot_at(parsed: &ParsedOutput, steam_id: u64, tick: i32) -> Option<&PlayerSnapshot> {
    parsed
        .player_snapshots
        .iter()
        .filter(|s| s.steam_id == steam_id && s.tick <= tick)
        .max_by_key(|s| (s.tick, s.ingestion_ordinal))
}

fn team_at(parsed: &ParsedOutput, steam_id: u64, tick: i32) -> Option<i32> {
    snapshot_at(parsed, steam_id, tick).and_then(|snapshot| snapshot.team)
}

pub mod fixtures {
    use openfrag_import::{CalculationIdentity as ImportIdentity, DemoMetadata, ParsedOutput};

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
}
