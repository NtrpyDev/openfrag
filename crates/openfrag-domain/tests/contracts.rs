use std::collections::BTreeMap;

use openfrag_domain::{
    BaselineField, BaselineQuery, BaselineScope, CalculationIdentity, CalculationSeriesIdentity,
    CohortExclusionReason, ComponentVector, HistoryMatch, MapContext, MatchInterval,
    MatchIrregularities, MatchTrendExclusion, OFR_V1_FORMULA, RatingBasisPoints,
    RatingEvidenceBundle, RatingInput, RatingMetrics, RatingStatus, Rational, ReceiptEvidenceSet,
    RequiredEvidenceKind, UnavailableReason, UnitInterval, build_baseline, calculate_rating,
};

fn neutral_components() -> ComponentVector {
    let half = UnitInterval::new(Rational::HALF).unwrap();
    ComponentVector {
        direct_damage: half,
        frag_balance: half,
        opening_duels: half,
        trade_kills: half,
        utility: half,
        clutch_conversion: half,
    }
}

fn worked_metrics() -> RatingMetrics {
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
        clutch_opportunities: 2,
    }
}

fn calculation_identity() -> CalculationIdentity {
    CalculationIdentity::ofr_v1(
        "demo", "commit", "build", "proto", "schema", "metrics", "epoch",
    )
}

fn coherent_series() -> CalculationSeriesIdentity {
    CalculationSeriesIdentity::ofr_v1(
        "local", "commit", "build", "proto", "schema", "metrics", "epoch",
    )
}

#[test]
fn missing_rating_evidence_suppresses_value_and_complete_receipts_are_exact() {
    let empty = ReceiptEvidenceSet::new(Vec::<String>::new());
    let missing = calculate_rating(RatingInput {
        identity: calculation_identity(),
        game_build: "game".into(),
        metrics: worked_metrics(),
        irregularities: MatchIrregularities::default(),
        evidence_failure: None,
        evidence: RatingEvidenceBundle {
            eligible_round_ledger: empty.clone(),
            excluded_round_ledger: empty.clone(),
            direct_damage: empty.clone(),
            frag_balance: empty.clone(),
            opening_duels: empty.clone(),
            trade_kills: empty.clone(),
            utility: empty.clone(),
            clutch_conversion: empty,
        },
    })
    .unwrap();
    assert_eq!(
        missing.status,
        RatingStatus::Unavailable(UnavailableReason::MissingRequiredEvidence(
            RequiredEvidenceKind::EligibleRoundLedger,
        ))
    );
    assert_eq!(missing.rating_bp, None);
    assert_eq!(missing.component_receipts, None);

    let evidence = |id: &str| ReceiptEvidenceSet::new(vec![id.to_owned()]);
    let complete_input = RatingInput {
        identity: calculation_identity(),
        game_build: "game".into(),
        metrics: worked_metrics(),
        irregularities: MatchIrregularities::default(),
        evidence_failure: None,
        evidence: RatingEvidenceBundle {
            eligible_round_ledger: evidence("eligible-ledger"),
            excluded_round_ledger: evidence("excluded-ledger"),
            direct_damage: evidence("damage-evidence"),
            frag_balance: evidence("frag-evidence"),
            opening_duels: evidence("opening-evidence"),
            trade_kills: evidence("trade-evidence"),
            utility: evidence("utility-evidence"),
            clutch_conversion: evidence("clutch-evidence"),
        },
    };
    let complete = calculate_rating(complete_input.clone()).unwrap();
    let repeated = calculate_rating(complete_input).unwrap();
    assert_eq!(complete, repeated);
    let direct = &complete.component_receipts.as_ref().unwrap()[0];
    assert_eq!(direct.exact_numerator, 1);
    assert_eq!(direct.exact_denominator, 2);
    assert_eq!(direct.weight, Rational::new(3, 10).unwrap());
    assert_eq!(direct.evidence_ids, vec!["damage-evidence"]);
    assert_eq!(complete.rating_bp.unwrap().get(), 10_220);
}

#[test]
fn full_identity_tuple_separates_schema_semantics_and_parser_only_rebinds() {
    let calculation = CalculationIdentity::ofr_v1(
        "demo",
        "parser-commit",
        "parser-build",
        "proto-build",
        "schema-hash",
        "metrics-v1",
        "evidence-v1",
    );
    assert_eq!(calculation.demo_hash(), "demo");
    assert_eq!(calculation.parser_commit(), "parser-commit");
    assert_eq!(calculation.parser_build(), "parser-build");
    assert_eq!(calculation.generated_proto_build(), "proto-build");
    assert_eq!(calculation.requested_schema_hash(), "schema-hash");
    assert_eq!(calculation.metric_definition_version(), "metrics-v1");
    assert_eq!(calculation.formula_version(), OFR_V1_FORMULA);
    assert_eq!(calculation.evidence_semantics_epoch(), "evidence-v1");
    let different_schema = CalculationIdentity::ofr_v1(
        "demo",
        "parser-commit",
        "parser-build",
        "proto-build",
        "different-schema",
        "metrics-v1",
        "evidence-v1",
    );
    assert_ne!(calculation, different_schema);

    let old_series = CalculationSeriesIdentity::ofr_v1(
        "local",
        "parser-commit-a",
        "parser-build-a",
        "proto-build",
        "schema-hash",
        "metrics-v1",
        "evidence-v1",
    );
    let new_parser = CalculationSeriesIdentity::ofr_v1(
        "local",
        "parser-commit-b",
        "parser-build-b",
        "proto-build",
        "schema-hash",
        "metrics-v1",
        "evidence-v1",
    );
    let new_schema = CalculationSeriesIdentity::ofr_v1(
        "local",
        "parser-commit-b",
        "parser-build-b",
        "proto-build",
        "different-schema",
        "metrics-v1",
        "evidence-v1",
    );
    assert!(old_series.is_parser_only_rebind_to(&new_parser));
    assert!(!old_series.is_parser_only_rebind_to(&old_series));
    assert!(!old_series.is_parser_only_rebind_to(&new_schema));
}

#[test]
fn history_accepts_unavailable_without_fake_values_and_keeps_exact_cutoff_equality() {
    let unavailable = HistoryMatch {
        match_id: "unavailable".into(),
        demo_hash: "demo-u".into(),
        calculation_run_id: "run-u".into(),
        series: coherent_series(),
        canonical: true,
        map: MapContext::Unknown,
        interval: Some(MatchInterval {
            start_utc_ms: 10,
            end_utc_ms: 20,
        }),
        rating: None,
        components: None,
        side_vectors: BTreeMap::new(),
        trend_exclusion: Some(MatchTrendExclusion::Unavailable),
    };
    let missing_value = HistoryMatch {
        match_id: "missing-value".into(),
        demo_hash: "demo-m".into(),
        calculation_run_id: "run-m".into(),
        series: coherent_series(),
        canonical: true,
        map: MapContext::Known("de_mirage".into()),
        interval: Some(MatchInterval {
            start_utc_ms: 30,
            end_utc_ms: 40,
        }),
        rating: None,
        components: None,
        side_vectors: BTreeMap::new(),
        trend_exclusion: None,
    };
    let equality = HistoryMatch {
        match_id: "equality".into(),
        demo_hash: "demo-e".into(),
        calculation_run_id: "run-e".into(),
        series: coherent_series(),
        canonical: true,
        map: MapContext::Known("de_mirage".into()),
        interval: Some(MatchInterval {
            start_utc_ms: 50,
            end_utc_ms: 100,
        }),
        rating: Some(RatingBasisPoints::new(10_400).unwrap()),
        components: Some(neutral_components()),
        side_vectors: BTreeMap::new(),
        trend_exclusion: None,
    };
    let receipt = build_baseline(
        &[unavailable, missing_value, equality],
        BaselineQuery {
            series: coherent_series(),
            cutoff_start_utc_ms: 100,
            field: BaselineField::Rating,
            scope: BaselineScope::AllMaps,
        },
    )
    .unwrap();
    assert_eq!(receipt.included_calculation_run_ids, vec!["run-e"]);
    assert_eq!(
        receipt.selected_values,
        vec![Rational::from_integer(10_400)]
    );
    assert!(receipt.exclusions.iter().any(|excluded| {
        excluded.match_id == "unavailable"
            && excluded.reason == CohortExclusionReason::MatchRule(MatchTrendExclusion::Unavailable)
    }));
    assert!(receipt.exclusions.iter().any(|excluded| {
        excluded.match_id == "missing-value"
            && excluded.reason == CohortExclusionReason::MissingCanonicalValue
    }));
}

#[test]
fn unknown_map_is_explicit_and_never_enters_same_map_context() {
    let unknown = HistoryMatch {
        match_id: "unknown-map".into(),
        demo_hash: "demo-unknown".into(),
        calculation_run_id: "run-unknown".into(),
        series: coherent_series(),
        canonical: true,
        map: MapContext::Unknown,
        interval: Some(MatchInterval {
            start_utc_ms: 10,
            end_utc_ms: 20,
        }),
        rating: Some(RatingBasisPoints::new(10_000).unwrap()),
        components: Some(neutral_components()),
        side_vectors: BTreeMap::new(),
        trend_exclusion: None,
    };
    let receipt = build_baseline(
        &[unknown],
        BaselineQuery {
            series: coherent_series(),
            cutoff_start_utc_ms: 30,
            field: BaselineField::Rating,
            scope: BaselineScope::Map("de_mirage".into()),
        },
    )
    .unwrap();
    assert!(receipt.included_match_ids.is_empty());
    assert_eq!(
        receipt.exclusions[0].reason,
        CohortExclusionReason::UnknownMap
    );
}
