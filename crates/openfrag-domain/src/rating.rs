use crate::{CalculationIdentity, ExactError, RatingBasisPoints, Rational, UnitInterval};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ComponentKind {
    DirectDamage,
    FragBalance,
    OpeningDuels,
    TradeKills,
    Utility,
    ClutchConversion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentVector {
    pub direct_damage: UnitInterval,
    pub frag_balance: UnitInterval,
    pub opening_duels: UnitInterval,
    pub trade_kills: UnitInterval,
    pub utility: UnitInterval,
    pub clutch_conversion: UnitInterval,
}

impl ComponentVector {
    pub const fn get(self, kind: ComponentKind) -> UnitInterval {
        match kind {
            ComponentKind::DirectDamage => self.direct_damage,
            ComponentKind::FragBalance => self.frag_balance,
            ComponentKind::OpeningDuels => self.opening_duels,
            ComponentKind::TradeKills => self.trade_kills,
            ComponentKind::Utility => self.utility,
            ComponentKind::ClutchConversion => self.clutch_conversion,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RatingMetrics {
    pub eligible_rounds: u32,
    pub direct_damage: u32,
    pub kills: u32,
    pub deaths: u32,
    pub opening_wins: u32,
    pub opening_losses: u32,
    pub trade_kills: u32,
    pub utility_damage: u32,
    pub flash_assists: u32,
    pub clutch_wins: u32,
    pub clutch_opportunities: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MatchIrregularities {
    pub missed_live_round: bool,
    pub roster_imbalanced: bool,
    pub surrender: bool,
    pub forfeit_without_canonical_winner: bool,
    pub overtime: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceFailure {
    MissingRequiredStream,
    MalformedEvent,
    ContradictoryAttribution,
    FailedReconciliation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RatingInput {
    pub identity: CalculationIdentity,
    pub game_build: String,
    pub metrics: RatingMetrics,
    pub irregularities: MatchIrregularities,
    pub evidence_failure: Option<EvidenceFailure>,
    pub evidence_receipts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    InsufficientEligibleRounds { observed: u32, required: u32 },
    Evidence(EvidenceFailure),
    ForfeitWithoutCanonicalWinner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewReason {
    ShortSample { observed: u32, trend_required: u32 },
    MissedLiveRound,
    Surrender,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrendExclusionReason {
    Preview,
    RosterImbalanced,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RatingStatus {
    Unavailable(UnavailableReason),
    Preview(Vec<PreviewReason>),
    Rated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RatingReceipt {
    pub identity: CalculationIdentity,
    pub game_build: String,
    pub inputs: RatingMetrics,
    pub irregularities: MatchIrregularities,
    pub evidence_receipts: Vec<String>,
    pub status: RatingStatus,
    pub trend_exclusions: Vec<TrendExclusionReason>,
    pub components: Option<ComponentVector>,
    pub rating_exact: Option<Rational>,
    pub rating_bp: Option<RatingBasisPoints>,
}

impl RatingReceipt {
    pub fn trend_eligible(&self) -> bool {
        matches!(self.status, RatingStatus::Rated) && self.trend_exclusions.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RatingInputError {
    ClutchWinsExceedOpportunities,
    Exact(ExactError),
}

impl From<ExactError> for RatingInputError {
    fn from(value: ExactError) -> Self {
        Self::Exact(value)
    }
}

pub fn calculate_rating(input: RatingInput) -> Result<RatingReceipt, RatingInputError> {
    if input.metrics.clutch_wins > input.metrics.clutch_opportunities {
        return Err(RatingInputError::ClutchWinsExceedOpportunities);
    }

    let unavailable = if let Some(failure) = input.evidence_failure {
        Some(UnavailableReason::Evidence(failure))
    } else if input.irregularities.forfeit_without_canonical_winner {
        Some(UnavailableReason::ForfeitWithoutCanonicalWinner)
    } else if input.metrics.eligible_rounds < 8 {
        Some(UnavailableReason::InsufficientEligibleRounds {
            observed: input.metrics.eligible_rounds,
            required: 8,
        })
    } else {
        None
    };

    if let Some(reason) = unavailable {
        return Ok(RatingReceipt {
            identity: input.identity,
            game_build: input.game_build,
            inputs: input.metrics,
            irregularities: input.irregularities,
            evidence_receipts: input.evidence_receipts,
            status: RatingStatus::Unavailable(reason),
            trend_exclusions: vec![TrendExclusionReason::Unavailable],
            components: None,
            rating_exact: None,
            rating_bp: None,
        });
    }

    let components = component_vector(input.metrics)?;
    let rating_exact = weighted_rating(components)?;
    let rounded = rating_exact
        .checked_mul(Rational::from_integer(10_000))?
        .round_half_up_nonnegative()?;
    let rating_bp = RatingBasisPoints::new(
        u16::try_from(rounded).map_err(|_| ExactError::OutsideRatingRange)?,
    )?;

    let mut preview_reasons = Vec::new();
    if input.metrics.eligible_rounds < 12 {
        preview_reasons.push(PreviewReason::ShortSample {
            observed: input.metrics.eligible_rounds,
            trend_required: 12,
        });
    }
    if input.irregularities.missed_live_round {
        preview_reasons.push(PreviewReason::MissedLiveRound);
    }
    if input.irregularities.surrender {
        preview_reasons.push(PreviewReason::Surrender);
    }
    let status = if preview_reasons.is_empty() {
        RatingStatus::Rated
    } else {
        RatingStatus::Preview(preview_reasons)
    };
    let mut trend_exclusions = Vec::new();
    if matches!(status, RatingStatus::Preview(_)) {
        trend_exclusions.push(TrendExclusionReason::Preview);
    }
    if input.irregularities.roster_imbalanced {
        trend_exclusions.push(TrendExclusionReason::RosterImbalanced);
    }

    Ok(RatingReceipt {
        identity: input.identity,
        game_build: input.game_build,
        inputs: input.metrics,
        irregularities: input.irregularities,
        evidence_receipts: input.evidence_receipts,
        status,
        trend_exclusions,
        components: Some(components),
        rating_exact: Some(rating_exact),
        rating_bp: Some(rating_bp),
    })
}

fn component_vector(metrics: RatingMetrics) -> Result<ComponentVector, ExactError> {
    let rounds = i64::from(metrics.eligible_rounds);
    let direct_damage = UnitInterval::clamped(Rational::new(
        i64::from(metrics.direct_damage),
        160 * rounds,
    )?);
    let frag_delta = i64::from(metrics.kills) - i64::from(metrics.deaths);
    let frag_balance =
        UnitInterval::clamped(Rational::HALF.checked_add(Rational::new(frag_delta, 2 * rounds)?)?);
    let opening_total = i64::from(metrics.opening_wins) + i64::from(metrics.opening_losses);
    let opening_duels = if opening_total == 0 {
        UnitInterval::new(Rational::HALF)?
    } else {
        UnitInterval::new(Rational::new(
            i64::from(metrics.opening_wins),
            opening_total,
        )?)?
    };
    let trade_kills =
        UnitInterval::clamped(Rational::new(2 * i64::from(metrics.trade_kills), rounds)?);
    let utility_damage = UnitInterval::clamped(Rational::new(
        i64::from(metrics.utility_damage),
        30 * rounds,
    )?);
    let flash_assists =
        UnitInterval::clamped(Rational::new(2 * i64::from(metrics.flash_assists), rounds)?);
    let utility = UnitInterval::new(
        utility_damage
            .exact()
            .checked_mul(Rational::new(7, 10)?)?
            .checked_add(flash_assists.exact().checked_mul(Rational::new(3, 10)?)?)?,
    )?;
    let clutch_conversion = if metrics.clutch_opportunities == 0 {
        UnitInterval::new(Rational::HALF)?
    } else {
        UnitInterval::new(Rational::new(
            i64::from(metrics.clutch_wins),
            i64::from(metrics.clutch_opportunities),
        )?)?
    };
    Ok(ComponentVector {
        direct_damage,
        frag_balance,
        opening_duels,
        trade_kills,
        utility,
        clutch_conversion,
    })
}

fn weighted_rating(components: ComponentVector) -> Result<Rational, ExactError> {
    let weighted = components
        .direct_damage
        .exact()
        .checked_mul(Rational::new(3, 10)?)?
        .checked_add(
            components
                .frag_balance
                .exact()
                .checked_mul(Rational::new(3, 10)?)?,
        )?
        .checked_add(
            components
                .opening_duels
                .exact()
                .checked_mul(Rational::new(3, 20)?)?,
        )?
        .checked_add(
            components
                .trade_kills
                .exact()
                .checked_mul(Rational::new(1, 10)?)?,
        )?
        .checked_add(
            components
                .utility
                .exact()
                .checked_mul(Rational::new(1, 10)?)?,
        )?
        .checked_add(
            components
                .clutch_conversion
                .exact()
                .checked_mul(Rational::new(1, 20)?)?,
        )?;
    weighted.checked_mul(Rational::from_integer(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CalculationIdentity {
        CalculationIdentity::ofr_v1("demo", "parser", "proto", "metrics")
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

    fn input(metrics: RatingMetrics) -> RatingInput {
        RatingInput {
            identity: identity(),
            game_build: "game".into(),
            metrics,
            irregularities: MatchIrregularities::default(),
            evidence_failure: None,
            evidence_receipts: vec!["round-ledger".into()],
        }
    }

    #[test]
    fn worked_example_produces_the_fixed_ofr_v1_rating() {
        let receipt = calculate_rating(input(worked_metrics())).unwrap();
        assert_eq!(receipt.rating_exact, Some(Rational::new(511, 500).unwrap()));
        assert_eq!(receipt.rating_bp.unwrap().get(), 10_220);
        assert_eq!(receipt.rating_bp.unwrap().display_centi(), 102);
        assert!(receipt.trend_eligible());
    }

    #[test]
    fn availability_and_preview_rules_are_machine_readable() {
        let mut metrics = worked_metrics();
        metrics.eligible_rounds = 7;
        let unavailable = calculate_rating(input(metrics)).unwrap();
        assert!(matches!(
            unavailable.status,
            RatingStatus::Unavailable(UnavailableReason::InsufficientEligibleRounds { .. })
        ));

        metrics.eligible_rounds = 8;
        let preview = calculate_rating(input(metrics)).unwrap();
        assert!(matches!(preview.status, RatingStatus::Preview(_)));
        assert!(!preview.trend_eligible());
    }

    #[test]
    fn property_components_and_rating_remain_bounded() {
        for rounds in 8..=30 {
            for multiplier in 0..=4 {
                let metrics = RatingMetrics {
                    eligible_rounds: rounds,
                    direct_damage: multiplier * 160 * rounds,
                    kills: multiplier * rounds,
                    deaths: (4 - multiplier) * rounds,
                    opening_wins: multiplier,
                    opening_losses: 4 - multiplier,
                    trade_kills: multiplier * rounds,
                    utility_damage: multiplier * 30 * rounds,
                    flash_assists: multiplier * rounds,
                    clutch_wins: multiplier,
                    clutch_opportunities: 4,
                };
                let receipt = calculate_rating(input(metrics)).unwrap();
                let vector = receipt.components.unwrap();
                for kind in [
                    ComponentKind::DirectDamage,
                    ComponentKind::FragBalance,
                    ComponentKind::OpeningDuels,
                    ComponentKind::TradeKills,
                    ComponentKind::Utility,
                    ComponentKind::ClutchConversion,
                ] {
                    assert!((Rational::ZERO..=Rational::ONE).contains(&vector.get(kind).exact()));
                }
                assert!(receipt.rating_bp.unwrap().get() <= RatingBasisPoints::MAX);
            }
        }
    }

    #[test]
    fn property_an_added_non_trade_kill_never_lowers_rating() {
        for kills in 0..=40 {
            let mut metrics = worked_metrics();
            metrics.kills = kills;
            let before = calculate_rating(input(metrics)).unwrap().rating_bp.unwrap();
            metrics.kills += 1;
            let after = calculate_rating(input(metrics)).unwrap().rating_bp.unwrap();
            assert!(after >= before);
        }
    }
}
