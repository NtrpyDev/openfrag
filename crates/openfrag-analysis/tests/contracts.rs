use openfrag_analysis::{analyze, rate_verified_input, AnalysisUnavailable};
use openfrag_domain::{
    CalculationIdentity, MatchIrregularities, RatingEvidenceBundle, RatingInput, RatingMetrics,
    RatingStatus, ReceiptEvidenceSet,
};

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
