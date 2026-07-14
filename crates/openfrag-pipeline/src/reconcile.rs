use openfrag_domain::{DemoHighlightEvidence, FinalHighlightLabel};
use openfrag_import::ParsedOutput;
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
    fn persist_reconciliation(
        &mut self,
        decision: &ReconciliationDecision,
    ) -> Result<(), Self::Error>;
}

pub fn reconcile_demo_candidates<P: ReconciliationPersistence>(
    _parsed: &ParsedOutput,
    _local_steam_id: u64,
    candidates: &[ReconciliationCandidate],
    persistence: &mut P,
) -> Result<Vec<ReconciliationDecision>, P::Error> {
    let mut decisions = Vec::new();
    for candidate in candidates {
        let decision = ReconciliationDecision {
            candidate_id: candidate.candidate_id.clone(),
            clip_id: candidate.clip_id.clone(),
            disposition: ReconciliationDisposition::EvidenceUnavailable(
                UnavailableEvidence::RoundMissingOrAmbiguous,
            ),
        };
        persistence.persist_reconciliation(&decision)?;
        decisions.push(decision);
    }
    Ok(decisions)
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
