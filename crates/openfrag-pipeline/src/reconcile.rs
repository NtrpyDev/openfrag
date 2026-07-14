use openfrag_domain::{DemoHighlightEvidence, FinalHighlightLabel, final_labels};
use openfrag_import::{ParsedOutput, PlayerSnapshot};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateOrigin {
    ProvisionalAutoRound,
    ManualFlag,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationCandidate {
    pub candidate_id: String,
    pub clip_id: String,
    pub demo_round_number: Option<u64>,
    pub origin: CandidateOrigin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableEvidence {
    RoundLinkMissing,
    RoundMissingOrAmbiguous,
    LocalTeam,
    VictimIdentityOrTeam,
    KillWeapon,
    Liveness,
    RoundWinner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceCompleteness {
    Complete,
    Partial(UnavailableEvidence),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationDisposition {
    Confirm {
        labels: BTreeSet<FinalHighlightLabel>,
        evidence: DemoHighlightEvidence,
        completeness: EvidenceCompleteness,
    },
    RejectOrdinary,
    EvidenceUnavailable(UnavailableEvidence),
    ManualIndependent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationDecision {
    pub candidate_id: String,
    pub clip_id: String,
    pub disposition: ReconciliationDisposition,
}

pub trait ReconciliationPersistence {
    type Error;
    /// Persists one deterministic decision at the integration boundary.
    ///
    /// # Errors
    /// Returns the persistence adapter's error without evaluating later candidates.
    fn persist_reconciliation(
        &mut self,
        decision: &ReconciliationDecision,
    ) -> Result<(), Self::Error>;
}

/// Evaluates explicitly Demo-linked candidates and persists decisions in stable identifier order.
///
/// # Errors
/// Returns the persistence adapter's first error and stops before subsequent decisions.
pub fn reconcile_demo_candidates<P: ReconciliationPersistence>(
    parsed: &ParsedOutput,
    local_steam_id: u64,
    candidates: &[ReconciliationCandidate],
    persistence: &mut P,
) -> Result<Vec<ReconciliationDecision>, P::Error> {
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        (&left.candidate_id, &left.clip_id).cmp(&(&right.candidate_id, &right.clip_id))
    });
    let mut decisions = Vec::with_capacity(ordered.len());
    for candidate in ordered {
        let decision = ReconciliationDecision {
            candidate_id: candidate.candidate_id.clone(),
            clip_id: candidate.clip_id.clone(),
            disposition: evaluate(parsed, local_steam_id, candidate),
        };
        persistence.persist_reconciliation(&decision)?;
        decisions.push(decision);
    }
    Ok(decisions)
}

#[allow(clippy::too_many_lines)]
fn evaluate(
    parsed: &ParsedOutput,
    local: u64,
    candidate: &ReconciliationCandidate,
) -> ReconciliationDisposition {
    if candidate.origin == CandidateOrigin::ManualFlag {
        return ReconciliationDisposition::ManualIndependent;
    }
    let Some(round_number) = candidate.demo_round_number else {
        return ReconciliationDisposition::EvidenceUnavailable(
            UnavailableEvidence::RoundLinkMissing,
        );
    };
    let matching_rounds = parsed
        .rounds
        .iter()
        .filter(|round| round.number == round_number)
        .collect::<Vec<_>>();
    let [round] = matching_rounds.as_slice() else {
        return ReconciliationDisposition::EvidenceUnavailable(
            UnavailableEvidence::RoundMissingOrAmbiguous,
        );
    };
    let previous_end = parsed
        .rounds
        .iter()
        .filter(|other| other.number < round_number)
        .map(|other| other.end_tick)
        .max()
        .unwrap_or(i32::MIN);
    let mut enemy_kills = 0_u8;
    let mut enemy_knife_kill = false;
    let mut partial = None;
    for death in parsed.events.iter().filter(|event| {
        event.name == "player_death"
            && event.tick > previous_end
            && event.tick <= round.end_tick
            && event.attacker() == Some(local)
    }) {
        let Some(victim) = death.victim() else {
            partial.get_or_insert(UnavailableEvidence::VictimIdentityOrTeam);
            continue;
        };
        let local_team = team_at(parsed, local, death.tick);
        let victim_team = team_at(parsed, victim, death.tick);
        let Some((local_team, victim_team)) = local_team.zip(victim_team) else {
            partial.get_or_insert(UnavailableEvidence::VictimIdentityOrTeam);
            continue;
        };
        if local_team == victim_team {
            continue;
        }
        enemy_kills = enemy_kills.saturating_add(1);
        match death.weapon() {
            Some(weapon) => enemy_knife_kill |= is_knife(weapon),
            None => {
                partial.get_or_insert(UnavailableEvidence::KillWeapon);
            }
        }
    }
    let clutch = clutch_opponents(parsed, local, previous_end, round.end_tick);
    let clutch_opponents_at_start = match clutch {
        Ok(value) => value,
        Err(reason) => {
            partial.get_or_insert(reason);
            None
        }
    };
    let team_won_round = if clutch_opponents_at_start.is_some() {
        if let Some((local_team, winner)) = team_at(parsed, local, round.end_tick)
            .zip(round.winner.as_deref().and_then(winner_team))
        {
            local_team == winner
        } else {
            partial.get_or_insert(if team_at(parsed, local, round.end_tick).is_none() {
                UnavailableEvidence::LocalTeam
            } else {
                UnavailableEvidence::RoundWinner
            });
            false
        }
    } else {
        false
    };
    let evidence = DemoHighlightEvidence {
        enemy_kills,
        clutch_opponents_at_start,
        team_won_round,
        enemy_knife_kill,
    };
    let labels = final_labels(evidence);
    if !labels.is_empty() {
        return ReconciliationDisposition::Confirm {
            labels,
            evidence,
            completeness: partial.map_or(
                EvidenceCompleteness::Complete,
                EvidenceCompleteness::Partial,
            ),
        };
    }
    partial.map_or(
        ReconciliationDisposition::RejectOrdinary,
        ReconciliationDisposition::EvidenceUnavailable,
    )
}

fn clutch_opponents(
    parsed: &ParsedOutput,
    local: u64,
    start_tick: i32,
    end_tick: i32,
) -> Result<Option<u8>, UnavailableEvidence> {
    let ticks = parsed
        .player_snapshots
        .iter()
        .filter(|snapshot| snapshot.tick > start_tick && snapshot.tick <= end_tick)
        .map(|snapshot| snapshot.tick)
        .collect::<BTreeSet<_>>();
    if ticks.is_empty() {
        return Err(UnavailableEvidence::Liveness);
    }
    for tick in ticks {
        let local_snapshot =
            snapshot_at(parsed, local, tick).ok_or(UnavailableEvidence::Liveness)?;
        let local_team = local_snapshot.team.ok_or(UnavailableEvidence::LocalTeam)?;
        let local_alive = local_snapshot.alive.ok_or(UnavailableEvidence::Liveness)?;
        if !local_alive {
            continue;
        }
        let mut teammates_alive = 0_u8;
        let mut enemies_alive = 0_u8;
        for participant in parsed
            .participants
            .iter()
            .filter(|participant| participant.steam_id != local)
        {
            let snapshot = snapshot_at(parsed, participant.steam_id, tick)
                .ok_or(UnavailableEvidence::Liveness)?;
            let team = snapshot.team.ok_or(UnavailableEvidence::LocalTeam)?;
            let alive = snapshot.alive.ok_or(UnavailableEvidence::Liveness)?;
            if alive && team == local_team {
                teammates_alive = teammates_alive.saturating_add(1);
            } else if alive && matches!(team, 2 | 3) {
                enemies_alive = enemies_alive.saturating_add(1);
            }
        }
        if teammates_alive == 0 && enemies_alive >= 2 {
            return Ok(Some(enemies_alive));
        }
    }
    Ok(None)
}

fn snapshot_at(parsed: &ParsedOutput, steam_id: u64, tick: i32) -> Option<&PlayerSnapshot> {
    parsed
        .player_snapshots
        .iter()
        .filter(|snapshot| snapshot.steam_id == steam_id && snapshot.tick <= tick)
        .max_by_key(|snapshot| (snapshot.tick, snapshot.ingestion_ordinal))
}

fn team_at(parsed: &ParsedOutput, steam_id: u64, tick: i32) -> Option<i32> {
    snapshot_at(parsed, steam_id, tick)
        .and_then(|snapshot| snapshot.team)
        .or_else(|| {
            parsed
                .participants
                .iter()
                .find(|participant| participant.steam_id == steam_id)
                .and_then(|participant| participant.team)
        })
}

fn winner_team(value: &str) -> Option<i32> {
    match value {
        "2" | "T" => Some(2),
        "3" | "CT" => Some(3),
        _ => None,
    }
}

fn is_knife(weapon: &str) -> bool {
    let normalized = weapon.to_ascii_lowercase();
    normalized.contains("knife")
        || matches!(
            normalized.as_str(),
            "bayonet" | "karambit" | "daggers" | "butterfly" | "falchion" | "kukri" | "machete"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfrag_import::{
        CalculationIdentity, DemoMetadata, ParsedEvent, ParsedRound, Participant, PlayerSnapshot,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemoryPersistence(Vec<ReconciliationDecision>);
    impl ReconciliationPersistence for MemoryPersistence {
        type Error = ();
        fn persist_reconciliation(
            &mut self,
            decision: &ReconciliationDecision,
        ) -> Result<(), Self::Error> {
            self.0.push(decision.clone());
            Ok(())
        }
    }

    fn candidate(id: &str, round: Option<u64>, origin: CandidateOrigin) -> ReconciliationCandidate {
        ReconciliationCandidate {
            candidate_id: id.into(),
            clip_id: format!("clip-{id}"),
            demo_round_number: round,
            origin,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn parsed(kills: usize, weapon: Option<&str>, clutch: bool) -> ParsedOutput {
        let mut events = Vec::new();
        for index in 0..kills {
            let victim = 20 + u64::try_from(index).unwrap();
            let mut raw_fields = BTreeMap::from([
                ("attacker_steamid".into(), json!(10)),
                ("user_steamid".into(), json!(victim)),
            ]);
            if let Some(weapon) = weapon {
                raw_fields.insert("weapon".into(), Value::String(weapon.into()));
            }
            events.push(ParsedEvent {
                name: "player_death".into(),
                tick: 20 + i32::try_from(index).unwrap(),
                ingestion_ordinal: u64::try_from(index).unwrap(),
                fields: BTreeMap::new(),
                raw_fields,
            });
        }
        let participants = vec![
            Participant {
                steam_id: 10,
                name: None,
                team: Some(2),
            },
            Participant {
                steam_id: 11,
                name: None,
                team: Some(2),
            },
            Participant {
                steam_id: 20,
                name: None,
                team: Some(3),
            },
            Participant {
                steam_id: 21,
                name: None,
                team: Some(3),
            },
            Participant {
                steam_id: 22,
                name: None,
                team: Some(3),
            },
            Participant {
                steam_id: 23,
                name: None,
                team: Some(3),
            },
            Participant {
                steam_id: 24,
                name: None,
                team: Some(3),
            },
        ];
        let mut player_snapshots = Vec::new();
        for participant in &participants {
            let alive = if clutch {
                participant.steam_id == 10 || matches!(participant.steam_id, 20 | 21)
            } else {
                true
            };
            player_snapshots.push(PlayerSnapshot {
                tick: 10,
                ingestion_ordinal: participant.steam_id,
                steam_id: participant.steam_id,
                entity_id: None,
                team: participant.team,
                health: Some(i32::from(alive) * 100),
                alive: Some(alive),
                life_state: Some(i32::from(!alive)),
                round_counter: Some(1),
                raw_properties: BTreeMap::new(),
            });
        }
        ParsedOutput {
            metadata: DemoMetadata {
                map: Some("de_mirage".into()),
                patch_build: None,
                demo_stamp: None,
                server: None,
                game_directory: None,
                tick_rate: Some("64".into()),
                tick_rate_unavailable_reason: None,
            },
            participants,
            rounds: vec![ParsedRound {
                number: 1,
                end_tick: 100,
                winner: Some("T".into()),
            }],
            events,
            receipts: vec![],
            player_snapshots,
            suspicious_empty: false,
            identity: CalculationIdentity {
                source_sha256: "a".repeat(64),
                parser_commit: "p".into(),
                parser_build: "b".into(),
                generated_proto_build: "g".into(),
                requested_schema_hash: "s".into(),
                metric_definition_version: "m".into(),
                formula_version: "f".into(),
                evidence_semantics_epoch: "e".into(),
            },
        }
    }

    fn disposition(
        parsed: &ParsedOutput,
        candidate: ReconciliationCandidate,
    ) -> ReconciliationDisposition {
        let mut persistence = MemoryPersistence::default();
        reconcile_demo_candidates(parsed, 10, &[candidate], &mut persistence)
            .unwrap()
            .remove(0)
            .disposition
    }

    #[test]
    fn labels_three_kill_four_kill_and_ace_from_demo_round_evidence() {
        for (kills, expected) in [
            (3, FinalHighlightLabel::Multikill),
            (4, FinalHighlightLabel::FourKill),
            (5, FinalHighlightLabel::Ace),
        ] {
            let result = disposition(
                &parsed(kills, Some("ak47"), false),
                candidate("auto", Some(1), CandidateOrigin::ProvisionalAutoRound),
            );
            assert!(
                matches!(result, ReconciliationDisposition::Confirm { labels, .. } if labels.contains(&expected))
            );
        }
    }

    #[test]
    fn labels_won_one_versus_two_clutch_and_knife() {
        let result = disposition(
            &parsed(1, Some("knife_karambit"), true),
            candidate("special", Some(1), CandidateOrigin::ProvisionalAutoRound),
        );
        assert!(
            matches!(result, ReconciliationDisposition::Confirm { labels, .. } if labels == BTreeSet::from([FinalHighlightLabel::Clutch, FinalHighlightLabel::Knife]))
        );
    }

    #[test]
    fn rejects_only_complete_ordinary_provisional_rounds() {
        assert_eq!(
            disposition(
                &parsed(1, Some("ak47"), false),
                candidate("ordinary", Some(1), CandidateOrigin::ProvisionalAutoRound)
            ),
            ReconciliationDisposition::RejectOrdinary
        );
    }

    #[test]
    fn manual_flag_is_independent_even_without_a_demo_round_link() {
        assert_eq!(
            disposition(
                &parsed(0, None, false),
                candidate("manual", None, CandidateOrigin::ManualFlag)
            ),
            ReconciliationDisposition::ManualIndependent
        );
    }

    #[test]
    fn incomplete_evidence_never_rejects_a_provisional_clip() {
        let mut output = parsed(1, None, false);
        output.player_snapshots.clear();
        assert!(matches!(
            disposition(
                &output,
                candidate("unknown", Some(1), CandidateOrigin::ProvisionalAutoRound)
            ),
            ReconciliationDisposition::EvidenceUnavailable(_)
        ));
    }

    #[test]
    fn decisive_available_evidence_retains_with_an_explicit_partial_marker() {
        let mut output = parsed(3, None, false);
        output.player_snapshots.clear();
        assert!(matches!(
            disposition(
                &output,
                candidate(
                    "partial-three-kill",
                    Some(1),
                    CandidateOrigin::ProvisionalAutoRound
                )
            ),
            ReconciliationDisposition::Confirm {
                labels,
                completeness: EvidenceCompleteness::Partial(_),
                ..
            } if labels.contains(&FinalHighlightLabel::Multikill)
        ));
    }

    #[test]
    fn persistence_receives_deterministically_sorted_decisions() {
        let mut persistence = MemoryPersistence::default();
        let decisions = reconcile_demo_candidates(
            &parsed(3, Some("ak47"), false),
            10,
            &[
                candidate("z", Some(1), CandidateOrigin::ProvisionalAutoRound),
                candidate("a", Some(1), CandidateOrigin::ProvisionalAutoRound),
            ],
            &mut persistence,
        )
        .unwrap();
        assert_eq!(
            decisions
                .iter()
                .map(|item| item.candidate_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(persistence.0, decisions);
    }
}
