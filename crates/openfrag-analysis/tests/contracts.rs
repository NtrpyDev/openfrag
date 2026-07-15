use openfrag_analysis::{AnalysisUnavailable, analyze, rate_verified_input};
use openfrag_domain::{
    CalculationIdentity, MatchIrregularities, RatingEvidenceBundle, RatingInput, RatingMetrics,
    RatingStatus, ReceiptEvidenceSet,
};
use openfrag_import::{
    CalculationIdentity as ImportIdentity, DemoMetadata, EventReceipt, NormalizedEvent,
    ParsedEvent, ParsedOutput, ParsedRound, Participant, PlayerSnapshot, SnapshotPhase, SteamId,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn evidence() -> RatingEvidenceBundle {
    let ids = || ReceiptEvidenceSet::new(vec!["sha256:evidence".into()]);
    RatingEvidenceBundle {
        eligible_round_ledger: ids(),
        excluded_round_ledger: ids(),
        direct_damage: ids(),
        frag_balance: ids(),
        opening_duels: ids(),
        trade_kills: ids(),
        utility: ids(),
        clutch_conversion: ids(),
    }
}

#[test]
fn published_golden_vector_is_10220_basis_points() {
    let receipt = rate_verified_input(RatingInput {
        identity: CalculationIdentity::ofr_v1(
            "demo", "commit", "parser", "proto", "query", "metrics", "epoch",
        ),
        game_build: "fixture-build".into(),
        metrics: RatingMetrics {
            eligible_rounds: 20,
            direct_damage: 1_600,
            kills: 18,
            deaths: 16,
            opening_wins: 3,
            opening_losses: 2,
            trade_kills: 4,
            utility_damage: 300,
            flash_assists: 2,
            clutch_wins: 1,
            clutch_opportunities: 2,
        },
        irregularities: MatchIrregularities::default(),
        evidence_failure: None,
        evidence: evidence(),
    })
    .expect("verified input must rate");
    assert!(matches!(receipt.status, RatingStatus::Rated));
    assert_eq!(receipt.rating_bp.expect("rated receipt").get(), 10_220);
}

fn event(name: &str, tick: i32, ordinal: u64, fields: &[(&str, Value)]) -> ParsedEvent {
    let raw_fields: BTreeMap<String, Value> = fields
        .iter()
        .map(|(name, value)| ((*name).into(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    ParsedEvent::from_raw(name, tick, ordinal, &raw_fields).expect("fixture event must normalize")
}

#[allow(clippy::too_many_lines)]
fn golden_parsed_output() -> ParsedOutput {
    let local = 76_561_198_000_000_001_u64;
    let mates = [local, 2, 3, 4, 5];
    let enemies = [11_u64, 12, 13, 14, 15];
    let participants = mates
        .iter()
        .map(|id| Participant {
            steam_id: (*id).into(),
            name: None,
            team: Some(2),
        })
        .chain(enemies.iter().map(|id| Participant {
            steam_id: (*id).into(),
            name: None,
            team: Some(3),
        }))
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    let mut snapshots = Vec::new();
    let mut rounds = Vec::new();
    let mut event_ordinal = 0_u64;
    let mut snapshot_ordinal = 0_u64;
    for round in 0_i32..20 {
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
        events.push(event(
            "round_freeze_end",
            base + 10,
            event_ordinal,
            &[("warmup", json!(false))],
        ));
        event_ordinal += 1;
        events.push(event(
            "player_hurt",
            base + 20,
            event_ordinal,
            &[
                ("attacker_steamid", json!(local.to_string())),
                ("user_steamid", json!(enemies[0].to_string())),
                ("dmg_health", json!(80)),
                ("weapon", json!("ak47")),
            ],
        ));
        event_ordinal += 1;
        if round < 15 {
            events.push(event(
                "player_hurt",
                base + 21,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(local.to_string())),
                    ("user_steamid", json!(enemies[1].to_string())),
                    ("dmg_health", json!(20)),
                    ("weapon", json!("hegrenade")),
                ],
            ));
            event_ordinal += 1;
        }
        if (5..=8).contains(&round) {
            events.push(event(
                "player_death",
                base + 30,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(enemies[2].to_string())),
                    ("user_steamid", json!(mates[1].to_string())),
                    ("weapon", json!("ak47")),
                    ("assistedflash", json!(false)),
                ],
            ));
            event_ordinal += 1;
        } else if (5..18).contains(&round) {
            events.push(event(
                "player_death",
                base + 30,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(enemies[3].to_string())),
                    ("user_steamid", json!(mates[1].to_string())),
                    ("weapon", json!("ak47")),
                    ("assistedflash", json!(false)),
                ],
            ));
            event_ordinal += 1;
        }
        if round < 18 {
            let victim = if (5..=8).contains(&round) {
                enemies[2]
            } else {
                enemies[0]
            };
            let kill_tick = if round < 3 { base + 30 } else { base + 40 };
            events.push(event(
                "player_death",
                kill_tick,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(local.to_string())),
                    ("user_steamid", json!(victim.to_string())),
                    ("weapon", json!("ak47")),
                    ("assistedflash", json!(false)),
                ],
            ));
            event_ordinal += 1;
        }
        if round < 16 {
            let death_tick = if (3..=4).contains(&round) {
                base + 30
            } else {
                base + 50
            };
            events.push(event(
                "player_death",
                death_tick,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(enemies[4].to_string())),
                    ("user_steamid", json!(local.to_string())),
                    ("weapon", json!("ak47")),
                    ("assistedflash", json!(false)),
                ],
            ));
            event_ordinal += 1;
        }
        if round >= 18 {
            events.push(event(
                "player_death",
                base + 40,
                event_ordinal,
                &[
                    ("attacker_steamid", json!(mates[1].to_string())),
                    ("user_steamid", json!(enemies[0].to_string())),
                    ("assister_steamid", json!(local.to_string())),
                    ("weapon", json!("ak47")),
                    ("assistedflash", json!(true)),
                ],
            ));
            event_ordinal += 1;
            for participant in &participants {
                let alive = participant.steam_id.get() == local
                    || enemies.contains(&participant.steam_id.get());
                snapshots.push(PlayerSnapshot {
                    tick: (base + 50).into(),
                    ingestion_ordinal: snapshot_ordinal.into(),
                    phase: SnapshotPhase::RequestedTick,
                    steam_id: participant.steam_id,
                    entity_id: None,
                    team: participant.team,
                    health: Some(100),
                    alive: Some(alive),
                    life_state: Some(i32::from(!alive)),
                    round_counter: Some(round),
                });
                snapshot_ordinal += 1;
            }
        }
        let winner = if round == 19 { 3 } else { 2 };
        events.push(event(
            "round_end",
            base + 90,
            event_ordinal,
            &[("winner", json!(winner))],
        ));
        event_ordinal += 1;
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
            number: u64::try_from(round + 1).unwrap().into(),
            end_tick: (base + 90).into(),
            winner: Some(winner),
        });
    }
    events.sort_by_key(|event| (event.tick, event.ingestion_ordinal));
    for (ordinal, event) in events.iter_mut().enumerate() {
        event.ingestion_ordinal = (ordinal as u64).into();
    }
    let receipts = events
        .iter()
        .map(|event| EventReceipt {
            ingestion_ordinal: event.ingestion_ordinal,
            event_name: event.name().into(),
            tick: event.tick,
            fields: BTreeMap::new(),
            raw_fields: BTreeMap::new(),
            evidence_sha256: format!("fixture-{:04}", event.ingestion_ordinal.get()),
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
            source_sha256: "fixture-demo".into(),
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

#[test]
fn complete_parser_evidence_reproduces_the_published_vector() {
    let analysis =
        analyze(&golden_parsed_output(), 76_561_198_000_000_001).expect("complete evidence rates");
    assert_eq!(
        analysis.rating_input.metrics,
        RatingMetrics {
            eligible_rounds: 20,
            direct_damage: 1_600,
            kills: 18,
            deaths: 16,
            opening_wins: 3,
            opening_losses: 2,
            trade_kills: 4,
            utility_damage: 300,
            flash_assists: 2,
            clutch_wins: 1,
            clutch_opportunities: 2
        }
    );
    assert_eq!(
        analysis.rating_receipt.rating_bp.expect("rated").get(),
        10_220
    );
}

fn resequence(parsed: &mut ParsedOutput) {
    parsed
        .events
        .sort_by_key(|event| (event.tick, event.ingestion_ordinal));
    for (ordinal, event) in parsed.events.iter_mut().enumerate() {
        event.ingestion_ordinal = (ordinal as u64).into();
    }
    parsed.receipts = parsed
        .events
        .iter()
        .map(|event| EventReceipt {
            ingestion_ordinal: event.ingestion_ordinal,
            event_name: event.name().into(),
            tick: event.tick,
            fields: BTreeMap::new(),
            raw_fields: BTreeMap::new(),
            evidence_sha256: format!("fixture-{:04}", event.ingestion_ordinal.get()),
        })
        .collect();
}

#[test]
fn duplicate_or_missing_round_end_is_rejected() {
    let mut duplicate = golden_parsed_output();
    let end = duplicate
        .events
        .iter()
        .find(|event| event.name() == "round_end")
        .unwrap()
        .clone();
    duplicate.events.push(end);
    resequence(&mut duplicate);
    assert_eq!(
        analyze(&duplicate, 76_561_198_000_000_001),
        Err(AnalysisUnavailable::MalformedEvidence)
    );
    let mut missing = golden_parsed_output();
    let index = missing
        .events
        .iter()
        .position(|event| event.name() == "round_end")
        .unwrap();
    missing.events.remove(index);
    resequence(&mut missing);
    assert_eq!(
        analyze(&missing, 76_561_198_000_000_001),
        Err(AnalysisUnavailable::MalformedEvidence)
    );
}

#[test]
fn stale_freeze_identity_snapshot_is_not_guessed() {
    let mut parsed = golden_parsed_output();
    parsed.player_snapshots.retain(|snapshot| {
        !(snapshot.steam_id.get() == 76_561_198_000_000_001 && snapshot.tick.get() == 10)
    });
    for (ordinal, snapshot) in parsed.player_snapshots.iter_mut().enumerate() {
        snapshot.ingestion_ordinal = (ordinal as u64).into();
    }
    let error =
        analyze(&parsed, 76_561_198_000_000_001).expect_err("stale state must exclude, not guess");
    assert!(matches!(
        error,
        AnalysisUnavailable::MalformedEvidence | AnalysisUnavailable::MissingTeamOrLiveness
    ));
}

#[test]
fn excess_damage_is_capped_and_team_self_world_damage_do_not_score() {
    let mut parsed = golden_parsed_output();
    let hurt = parsed
        .events
        .iter_mut()
        .find(|event| event.name() == "player_hurt")
        .unwrap();
    let NormalizedEvent::PlayerHurt(hurt_event) = &mut hurt.event else {
        unreachable!();
    };
    hurt_event.damage_health = Some(200);
    let tick = hurt.tick;
    let ordinal = hurt.ingestion_ordinal;
    for attacker in [2_u64, 76_561_198_000_000_001, 0] {
        parsed.events.push(event(
            "player_hurt",
            tick.get() + 1,
            ordinal.get() + attacker,
            &[
                ("attacker_steamid", json!(attacker.to_string())),
                ("user_steamid", json!(2_u64.to_string())),
                ("dmg_health", json!(100)),
                ("weapon", json!("ak47")),
            ],
        ));
    }
    resequence(&mut parsed);
    let analysis = analyze(&parsed, 76_561_198_000_000_001).expect("non-enemy damage is excluded");
    assert_eq!(analysis.rating_input.metrics.direct_damage, 1_620);
    assert_eq!(analysis.rating_input.metrics.utility_damage, 300);
}

#[test]
fn trade_window_includes_exactly_five_seconds_and_excludes_over() {
    let exact = analyze(&golden_parsed_output(), 76_561_198_000_000_001).unwrap();
    assert_eq!(exact.rating_input.metrics.trade_kills, 4);
    let mut over = golden_parsed_output();
    let deaths = over
        .events
        .iter_mut()
        .filter(|event| event.name() == "player_death" && event.tick.get() / 1_000 == 5)
        .collect::<Vec<_>>();
    let local_kill = deaths
        .into_iter()
        .find(|event| event.attacker() == Some(SteamId::new(76_561_198_000_000_001)))
        .unwrap();
    local_kill.tick = (local_kill.tick.get() + 321).into();
    resequence(&mut over);
    let analysis = analyze(&over, 76_561_198_000_000_001).unwrap();
    assert_eq!(analysis.rating_input.metrics.trade_kills, 3);
}

#[test]
fn flash_assist_requires_teammate_attribution() {
    let mut parsed = golden_parsed_output();
    let flash = parsed
        .events
        .iter_mut()
        .find(|event| {
            matches!(&event.event, NormalizedEvent::PlayerDeath(death) if death.assisted_flash)
        })
        .unwrap();
    let NormalizedEvent::PlayerDeath(flash_event) = &mut flash.event else {
        unreachable!();
    };
    flash_event.attacker = Some(SteamId::new(11));
    assert_eq!(
        analyze(&parsed, 76_561_198_000_000_001),
        Err(AnalysisUnavailable::MalformedEvidence)
    );
}

#[test]
fn same_tick_deaths_use_ingestion_order_for_the_opening_duel() {
    let mut parsed = golden_parsed_output();
    let first_round_deaths = parsed
        .events
        .iter_mut()
        .filter(|event| event.name() == "player_death" && event.tick.get() < 1_000)
        .collect::<Vec<_>>();
    let tick = first_round_deaths[0].tick;
    for death in first_round_deaths {
        death.tick = tick;
    }
    resequence(&mut parsed);
    let analysis = analyze(&parsed, 76_561_198_000_000_001).unwrap();
    assert_eq!(analysis.rating_input.metrics.opening_wins, 3);
    assert_eq!(analysis.rating_input.metrics.opening_losses, 2);
}

#[test]
fn every_v1_utility_weapon_is_classified_as_utility() {
    for weapon in ["hegrenade", "inferno", "molotov", "incgrenade"] {
        let mut parsed = golden_parsed_output();
        let hurt = parsed
            .events
            .iter_mut()
            .find(|event| event.name() == "player_hurt" && event.weapon() == Some("ak47"))
            .unwrap();
        let NormalizedEvent::PlayerHurt(hurt_event) = &mut hurt.event else {
            unreachable!();
        };
        hurt_event.weapon = Some(weapon.into());
        let analysis = analyze(&parsed, 76_561_198_000_000_001).unwrap();
        assert_eq!(analysis.rating_input.metrics.direct_damage, 1_520);
        assert_eq!(analysis.rating_input.metrics.utility_damage, 380);
    }
}

#[test]
fn disconnect_transition_and_post_death_bomb_win_preserve_clutch_conversion() {
    let mut parsed = golden_parsed_output();
    let base = 18_000;
    parsed.events.push(event(
        "player_disconnect",
        base + 49,
        u64::MAX - 2,
        &[("user_steamid", json!(2_u64.to_string()))],
    ));
    parsed.events.push(event(
        "player_death",
        base + 60,
        u64::MAX - 1,
        &[
            ("attacker_steamid", json!(11_u64.to_string())),
            (
                "user_steamid",
                json!(76_561_198_000_000_001_u64.to_string()),
            ),
            ("weapon", json!("ak47")),
            ("assistedflash", json!(false)),
        ],
    ));
    let ordinal = parsed.player_snapshots.len() as u64;
    parsed.player_snapshots.push(PlayerSnapshot {
        tick: (base + 60).into(),
        ingestion_ordinal: ordinal.into(),
        phase: SnapshotPhase::RequestedTick,
        steam_id: 76_561_198_000_000_001.into(),
        entity_id: None,
        team: Some(2),
        health: Some(0),
        alive: Some(false),
        life_state: Some(1),
        round_counter: Some(18),
    });
    parsed
        .player_snapshots
        .sort_by_key(|snapshot| (snapshot.tick, snapshot.ingestion_ordinal));
    for (ordinal, snapshot) in parsed.player_snapshots.iter_mut().enumerate() {
        snapshot.ingestion_ordinal = (ordinal as u64).into();
    }
    resequence(&mut parsed);
    let analysis = analyze(&parsed, 76_561_198_000_000_001).unwrap();
    assert_eq!(analysis.rating_input.metrics.clutch_opportunities, 2);
    assert_eq!(analysis.rating_input.metrics.clutch_wins, 1);
}

#[test]
fn missing_tick_rate_suppresses_analysis_instead_of_fabricating_zero() {
    let fixture = openfrag_analysis::fixtures::minimal_parsed_output_without_tick_rate();
    assert_eq!(
        analyze(&fixture, 76_561_198_000_000_001),
        Err(AnalysisUnavailable::MissingTickRate)
    );
}

#[test]
fn real_fixture_is_deterministically_unavailable_without_tick_rate() {
    let Ok(path) = std::env::var("OPENFRAG_DEMOPARSER_FIXTURE") else {
        return;
    };
    let parsed = openfrag_import::parse_with_pinned_demoparser(std::path::Path::new(&path))
        .expect("pinned fixture parses");
    let first = analyze(&parsed, 76_561_197_964_020_430);
    let second = analyze(&parsed, 76_561_197_964_020_430);
    assert_eq!(first, Err(AnalysisUnavailable::MissingTickRate));
    assert_eq!(second, first);
}
