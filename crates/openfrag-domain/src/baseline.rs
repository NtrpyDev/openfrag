use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CalculationSeriesIdentity, ComponentKind, ComponentVector, ExactError, RatingBasisPoints,
    Rational, UnitInterval,
};

pub const PERSONAL_BASELINE_POLICY: &str = "personal-baseline-1";
pub const BASELINE_MINIMUM_MATCHES: usize = 10;
pub const BASELINE_WINDOW_MATCHES: usize = 20;
pub const SESSION_GAP_MS: u64 = 90 * 60 * 1_000;
pub const GOAL_EVALUATION_MATCHES: usize = 10;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MatchInterval {
    pub start_utc_ms: u64,
    pub end_utc_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Side {
    Terrorist,
    CounterTerrorist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SideComponentVector {
    pub components: ComponentVector,
    pub opening_opportunities: u32,
    pub clutch_opportunities: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchTrendExclusion {
    Unavailable,
    Preview,
    RosterImbalanced,
    EvidenceRule,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MapContext {
    Known(String),
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryMatch {
    pub match_id: String,
    pub demo_hash: String,
    pub calculation_run_id: String,
    pub series: CalculationSeriesIdentity,
    pub canonical: bool,
    pub map: MapContext,
    pub interval: Option<MatchInterval>,
    pub rating: Option<RatingBasisPoints>,
    pub components: Option<ComponentVector>,
    pub side_vectors: BTreeMap<Side, SideComponentVector>,
    pub trend_exclusion: Option<MatchTrendExclusion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaselineField {
    Rating,
    Component(ComponentKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaselineScope {
    AllMaps,
    Map(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineQuery {
    pub series: CalculationSeriesIdentity,
    pub cutoff_start_utc_ms: u64,
    pub field: BaselineField,
    pub scope: BaselineScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CohortExclusionReason {
    CalculationSeriesMismatch,
    NonCanonicalRun,
    MatchRule(MatchTrendExclusion),
    MissingChronology,
    ReversedChronology,
    CurrentOrFutureMatch,
    ScopeMismatch,
    DuplicateDemo,
    OutsideRecentWindow,
    MissingSideVector,
    MissingCanonicalValue,
    UnknownMap,
    OverlappingChronology,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CohortExclusion {
    pub match_id: String,
    pub reason: CohortExclusionReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaselineMaturity {
    NotReady { selected: usize, required: usize },
    Limited { selected: usize },
    Recent { selected: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RobustStats {
    pub median: Rational,
    pub mad: Rational,
    pub lower: Rational,
    pub upper: Rational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangePosition {
    Below,
    Within,
    Above,
    UnavailableNoSpread,
}

impl RobustStats {
    pub fn position(self, value: Rational) -> RangePosition {
        if self.mad == Rational::ZERO {
            RangePosition::UnavailableNoSpread
        } else if value < self.lower {
            RangePosition::Below
        } else if value > self.upper {
            RangePosition::Above
        } else {
            RangePosition::Within
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineReceipt {
    pub policy_version: &'static str,
    pub query: BaselineQuery,
    pub included_match_ids: Vec<String>,
    pub included_demo_hashes: Vec<String>,
    pub included_calculation_run_ids: Vec<String>,
    pub selected_values: Vec<Rational>,
    pub exclusions: Vec<CohortExclusion>,
    pub maturity: BaselineMaturity,
    pub stats: Option<RobustStats>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaselineError {
    Exact(ExactError),
    EmptyStatistics,
    SideRatingForbidden,
    MapSideScopeForbidden,
}

impl From<ExactError> for BaselineError {
    fn from(value: ExactError) -> Self {
        Self::Exact(value)
    }
}

pub fn build_baseline(
    history: &[HistoryMatch],
    query: BaselineQuery,
) -> Result<BaselineReceipt, BaselineError> {
    build_lens(history, query, None)
}

pub fn build_side_component_lens(
    history: &[HistoryMatch],
    query: BaselineQuery,
    side: Side,
    component: ComponentKind,
) -> Result<BaselineReceipt, BaselineError> {
    if !matches!(&query.scope, BaselineScope::AllMaps) {
        return Err(BaselineError::MapSideScopeForbidden);
    }
    let query = BaselineQuery {
        field: BaselineField::Component(component),
        ..query
    };
    build_lens(history, query, Some(side))
}

fn build_lens(
    history: &[HistoryMatch],
    query: BaselineQuery,
    side: Option<Side>,
) -> Result<BaselineReceipt, BaselineError> {
    let side_component = match (&query.field, side) {
        (BaselineField::Component(kind), Some(_)) => Some(*kind),
        (BaselineField::Rating, Some(_)) => return Err(BaselineError::SideRatingForbidden),
        (_, None) => None,
    };
    let mut candidates = Vec::new();
    let mut exclusions = Vec::new();
    for item in history {
        let reason = cohort_reason(item, &query);
        if let Some(reason) = reason {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason,
            });
            continue;
        }
        let value = if let Some(side) = side {
            if let Some(vector) = item.side_vectors.get(&side) {
                let kind = side_component.ok_or(BaselineError::SideRatingForbidden)?;
                vector.components.get(kind).exact()
            } else {
                exclusions.push(CohortExclusion {
                    match_id: item.match_id.clone(),
                    reason: CohortExclusionReason::MissingSideVector,
                });
                continue;
            }
        } else {
            let Some(value) = match_value(item, &query.field) else {
                exclusions.push(CohortExclusion {
                    match_id: item.match_id.clone(),
                    reason: CohortExclusionReason::MissingCanonicalValue,
                });
                continue;
            };
            value
        };
        candidates.push((item, value));
    }
    finalize_baseline(candidates, exclusions, query)
}

fn finalize_baseline(
    mut candidates: Vec<(&HistoryMatch, Rational)>,
    mut exclusions: Vec<CohortExclusion>,
    query: BaselineQuery,
) -> Result<BaselineReceipt, BaselineError> {
    candidates.sort_by(|(left, _), (right, _)| {
        let left_end = left.interval.map_or(0, |interval| interval.end_utc_ms);
        let right_end = right.interval.map_or(0, |interval| interval.end_utc_ms);
        (right_end, &right.demo_hash).cmp(&(left_end, &left.demo_hash))
    });
    let mut seen = BTreeSet::new();
    candidates.retain(|(item, _)| {
        if seen.insert(item.demo_hash.clone()) {
            true
        } else {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::DuplicateDemo,
            });
            false
        }
    });
    if candidates.len() > BASELINE_WINDOW_MATCHES {
        for (item, _) in candidates.drain(BASELINE_WINDOW_MATCHES..) {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::OutsideRecentWindow,
            });
        }
    }
    let selected = candidates.len();
    let maturity = if selected < BASELINE_MINIMUM_MATCHES {
        BaselineMaturity::NotReady {
            selected,
            required: BASELINE_MINIMUM_MATCHES,
        }
    } else if selected < BASELINE_WINDOW_MATCHES {
        BaselineMaturity::Limited { selected }
    } else {
        BaselineMaturity::Recent { selected }
    };
    let stats = if selected < BASELINE_MINIMUM_MATCHES {
        None
    } else {
        let maximum = match query.field {
            BaselineField::Rating => Rational::from_integer(20_000),
            BaselineField::Component(_) => Rational::ONE,
        };
        Some(robust_stats(
            &candidates
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
            Rational::ZERO,
            maximum,
        )?)
    };
    Ok(BaselineReceipt {
        policy_version: PERSONAL_BASELINE_POLICY,
        query,
        included_match_ids: candidates
            .iter()
            .map(|(item, _)| item.match_id.clone())
            .collect(),
        included_demo_hashes: candidates
            .iter()
            .map(|(item, _)| item.demo_hash.clone())
            .collect(),
        included_calculation_run_ids: candidates
            .iter()
            .map(|(item, _)| item.calculation_run_id.clone())
            .collect(),
        selected_values: candidates.iter().map(|(_, value)| *value).collect(),
        exclusions,
        maturity,
        stats,
    })
}

fn cohort_reason(item: &HistoryMatch, query: &BaselineQuery) -> Option<CohortExclusionReason> {
    if item.series != query.series {
        return Some(CohortExclusionReason::CalculationSeriesMismatch);
    }
    if !item.canonical {
        return Some(CohortExclusionReason::NonCanonicalRun);
    }
    if let Some(reason) = item.trend_exclusion {
        return Some(CohortExclusionReason::MatchRule(reason));
    }
    let Some(interval) = item.interval else {
        return Some(CohortExclusionReason::MissingChronology);
    };
    if interval.start_utc_ms > interval.end_utc_ms {
        return Some(CohortExclusionReason::ReversedChronology);
    }
    if interval.end_utc_ms > query.cutoff_start_utc_ms {
        return Some(CohortExclusionReason::CurrentOrFutureMatch);
    }
    if let BaselineScope::Map(map) = &query.scope {
        match &item.map {
            MapContext::Known(item_map) if item_map == map => {}
            MapContext::Known(_) => return Some(CohortExclusionReason::ScopeMismatch),
            MapContext::Unknown => return Some(CohortExclusionReason::UnknownMap),
        }
    }
    None
}

fn match_value(item: &HistoryMatch, field: &BaselineField) -> Option<Rational> {
    match *field {
        BaselineField::Rating => item.rating.map(RatingBasisPoints::exact),
        BaselineField::Component(kind) => item.components.map(|vector| vector.get(kind).exact()),
    }
}

pub fn median(values: &[Rational]) -> Result<Rational, BaselineError> {
    if values.is_empty() {
        return Err(BaselineError::EmptyStatistics);
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let middle = ordered.len() / 2;
    if ordered.len() % 2 == 1 {
        Ok(ordered[middle])
    } else {
        Ok(ordered[middle - 1]
            .checked_add(ordered[middle])?
            .checked_div(Rational::from_integer(2))?)
    }
}

pub fn robust_stats(
    values: &[Rational],
    minimum: Rational,
    maximum: Rational,
) -> Result<RobustStats, BaselineError> {
    let center = median(values)?;
    let deviations = values
        .iter()
        .map(|value| value.checked_sub(center)?.abs())
        .collect::<Result<Vec<_>, _>>()?;
    let mad = median(&deviations)?;
    let spread = mad.checked_mul(Rational::from_integer(2))?;
    Ok(RobustStats {
        median: center,
        mad,
        lower: center.checked_sub(spread)?.clamp(minimum, maximum),
        upper: center.checked_add(spread)?.clamp(minimum, maximum),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub match_ids: Vec<String>,
    pub start_utc_ms: u64,
    pub end_utc_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionGrouping {
    pub sessions: Vec<Session>,
    pub exclusions: Vec<CohortExclusion>,
}

pub fn group_sessions(history: &[HistoryMatch]) -> SessionGrouping {
    let mut valid = Vec::new();
    let mut exclusions = Vec::new();
    for item in history {
        let Some(interval) = item.interval else {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::MissingChronology,
            });
            continue;
        };
        if interval.start_utc_ms > interval.end_utc_ms {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::ReversedChronology,
            });
            continue;
        }
        valid.push((item, interval));
    }
    valid.sort_by_key(|(item, interval)| {
        (interval.start_utc_ms, interval.end_utc_ms, &item.demo_hash)
    });
    let mut sessions: Vec<Session> = Vec::new();
    for (item, interval) in valid {
        if let Some(current) = sessions.last_mut() {
            if interval.start_utc_ms <= current.end_utc_ms {
                exclusions.push(CohortExclusion {
                    match_id: item.match_id.clone(),
                    reason: CohortExclusionReason::OverlappingChronology,
                });
                continue;
            }
            if interval.start_utc_ms - current.end_utc_ms <= SESSION_GAP_MS {
                current.match_ids.push(item.match_id.clone());
                current.end_utc_ms = interval.end_utc_ms;
                continue;
            }
        }
        sessions.push(Session {
            match_ids: vec![item.match_id.clone()],
            start_utc_ms: interval.start_utc_ms,
            end_utc_ms: interval.end_utc_ms,
        });
    }
    SessionGrouping {
        sessions,
        exclusions,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSample {
    None,
    One,
    Small { selected: usize },
    Summary { selected: usize },
}

pub fn session_sample(selected: usize) -> SessionSample {
    match selected {
        0 => SessionSample::None,
        1 => SessionSample::One,
        2..=4 => SessionSample::Small { selected },
        _ => SessionSample::Summary { selected },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Goal {
    id: String,
    created_at_utc_ms: u64,
    series: CalculationSeriesIdentity,
    field: BaselineField,
    scope: BaselineScope,
    target: Rational,
    baseline_demo_hashes: Vec<String>,
    cancelled: bool,
}

impl Goal {
    pub fn rating(
        goal_id: impl Into<String>,
        created_at_utc_ms: u64,
        series: CalculationSeriesIdentity,
        scope: BaselineScope,
        target: RatingBasisPoints,
        baseline_demo_hashes: Vec<String>,
    ) -> Self {
        Self {
            id: goal_id.into(),
            created_at_utc_ms,
            series,
            field: BaselineField::Rating,
            scope,
            target: target.exact(),
            baseline_demo_hashes,
            cancelled: false,
        }
    }

    pub fn component(
        goal_id: impl Into<String>,
        created_at_utc_ms: u64,
        series: CalculationSeriesIdentity,
        scope: BaselineScope,
        component: ComponentKind,
        target: UnitInterval,
        baseline_demo_hashes: Vec<String>,
    ) -> Self {
        Self {
            id: goal_id.into(),
            created_at_utc_ms,
            series,
            field: BaselineField::Component(component),
            scope,
            target: target.exact(),
            baseline_demo_hashes,
            cancelled: false,
        }
    }

    pub fn cancelled(mut self) -> Self {
        self.cancelled = true;
        self
    }

    pub fn baseline_demo_hashes(&self) -> &[String] {
        &self.baseline_demo_hashes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalStatus {
    InProgress { selected: usize, required: usize },
    Achieved,
    NotAchieved,
    Cancelled,
    VersionChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalReceipt {
    pub goal_id: String,
    pub status: GoalStatus,
    pub selected_match_ids: Vec<String>,
    pub selected_demo_hashes: Vec<String>,
    pub exclusions: Vec<CohortExclusion>,
    pub evaluation_median: Option<Rational>,
}

pub fn evaluate_goal(
    goal: &Goal,
    active_series: &CalculationSeriesIdentity,
    history: &[HistoryMatch],
) -> Result<GoalReceipt, BaselineError> {
    if goal.cancelled {
        return Ok(empty_goal_receipt(goal, GoalStatus::Cancelled));
    }
    if &goal.series != active_series {
        return Ok(empty_goal_receipt(goal, GoalStatus::VersionChanged));
    }
    let mut candidates = Vec::new();
    let mut exclusions = Vec::new();
    for item in history {
        let query = BaselineQuery {
            series: goal.series.clone(),
            cutoff_start_utc_ms: u64::MAX,
            field: goal.field.clone(),
            scope: goal.scope.clone(),
        };
        if let Some(reason) = cohort_reason(item, &query) {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason,
            });
            continue;
        }
        let Some(interval) = item.interval else {
            continue;
        };
        if interval.start_utc_ms <= goal.created_at_utc_ms {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::CurrentOrFutureMatch,
            });
            continue;
        }
        let Some(value) = match_value(item, &goal.field) else {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::MissingCanonicalValue,
            });
            continue;
        };
        candidates.push((item, interval, value));
    }
    candidates.sort_by_key(|(item, interval, _)| {
        (interval.start_utc_ms, interval.end_utc_ms, &item.demo_hash)
    });
    let mut seen = BTreeSet::new();
    candidates.retain(|(item, _, _)| {
        if seen.insert(item.demo_hash.clone()) {
            true
        } else {
            exclusions.push(CohortExclusion {
                match_id: item.match_id.clone(),
                reason: CohortExclusionReason::DuplicateDemo,
            });
            false
        }
    });
    candidates.truncate(GOAL_EVALUATION_MATCHES);
    let evaluation_median = if candidates.is_empty() {
        None
    } else {
        Some(median(
            &candidates
                .iter()
                .map(|(_, _, value)| *value)
                .collect::<Vec<_>>(),
        )?)
    };
    let status = if candidates.len() < GOAL_EVALUATION_MATCHES {
        GoalStatus::InProgress {
            selected: candidates.len(),
            required: GOAL_EVALUATION_MATCHES,
        }
    } else if evaluation_median.is_some_and(|value| value >= goal.target) {
        GoalStatus::Achieved
    } else {
        GoalStatus::NotAchieved
    };
    Ok(GoalReceipt {
        goal_id: goal.id.clone(),
        status,
        selected_match_ids: candidates
            .iter()
            .map(|(item, _, _)| item.match_id.clone())
            .collect(),
        selected_demo_hashes: candidates
            .iter()
            .map(|(item, _, _)| item.demo_hash.clone())
            .collect(),
        exclusions,
        evaluation_median,
    })
}

fn empty_goal_receipt(goal: &Goal, status: GoalStatus) -> GoalReceipt {
    GoalReceipt {
        goal_id: goal.id.clone(),
        status,
        selected_match_ids: Vec::new(),
        selected_demo_hashes: Vec::new(),
        exclusions: Vec::new(),
        evaluation_median: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnitInterval;

    fn series(parser: &str) -> CalculationSeriesIdentity {
        CalculationSeriesIdentity::ofr_v1(
            "local", "commit", parser, "proto", "schema", "metrics", "epoch",
        )
    }

    fn vector(value: Rational) -> ComponentVector {
        let value = UnitInterval::new(value).unwrap();
        ComponentVector {
            direct_damage: value,
            frag_balance: value,
            opening_duels: value,
            trade_kills: value,
            utility: value,
            clutch_conversion: value,
        }
    }

    fn history_match(index: u64, rating: u16) -> HistoryMatch {
        HistoryMatch {
            match_id: format!("match-{index:02}"),
            demo_hash: format!("demo-{index:02}"),
            calculation_run_id: format!("run-{index:02}"),
            series: series("parser-a"),
            canonical: true,
            map: MapContext::Known("de_mirage".into()),
            interval: Some(MatchInterval {
                start_utc_ms: index * 10_000,
                end_utc_ms: index * 10_000 + 5_000,
            }),
            rating: Some(RatingBasisPoints::new(rating).unwrap()),
            components: Some(vector(Rational::HALF)),
            side_vectors: BTreeMap::new(),
            trend_exclusion: None,
        }
    }

    fn query() -> BaselineQuery {
        BaselineQuery {
            series: series("parser-a"),
            cutoff_start_utc_ms: 1_000_000,
            field: BaselineField::Rating,
            scope: BaselineScope::AllMaps,
        }
    }

    #[test]
    fn minimum_and_window_are_exact_and_recent() {
        let nine: Vec<_> = (0..9).map(|i| history_match(i, 10_000)).collect();
        assert!(build_baseline(&nine, query()).unwrap().stats.is_none());
        let twenty_one: Vec<_> = (0..21)
            .map(|i| history_match(i, 10_000 + u16::try_from(i).unwrap()))
            .collect();
        let receipt = build_baseline(&twenty_one, query()).unwrap();
        assert_eq!(receipt.included_match_ids.len(), 20);
        assert!(!receipt.included_match_ids.contains(&"match-00".to_string()));
        assert!(matches!(
            receipt.maturity,
            BaselineMaturity::Recent { selected: 20 }
        ));
    }

    #[test]
    fn golden_median_and_mad_keep_the_valid_extreme() {
        let values = [
            8_800, 9_100, 9_400, 9_600, 9_800, 9_900, 10_000, 10_100, 10_200, 10_400, 10_400,
            10_500, 10_600, 10_800, 10_900, 11_000, 11_200, 11_500, 12_000, 16_200,
        ];
        let exact: Vec<_> = values.into_iter().map(Rational::from_integer).collect();
        let stats = robust_stats(&exact, Rational::ZERO, Rational::from_integer(20_000)).unwrap();
        assert_eq!(stats.median, Rational::from_integer(10_400));
        assert_eq!(stats.mad, Rational::from_integer(550));
        assert_eq!(stats.lower, Rational::from_integer(9_300));
        assert_eq!(stats.upper, Rational::from_integer(11_500));
    }

    #[test]
    fn property_future_and_mixed_series_never_enter_a_baseline() {
        let stable: Vec<_> = (0..10).map(|i| history_match(i, 10_000)).collect();
        let original = build_baseline(
            &stable,
            BaselineQuery {
                cutoff_start_utc_ms: 100_000,
                ..query()
            },
        )
        .unwrap();
        for future_index in 10..30 {
            let mut expanded = stable.clone();
            expanded.push(history_match(future_index, 20_000));
            let next = build_baseline(
                &expanded,
                BaselineQuery {
                    cutoff_start_utc_ms: 100_000,
                    ..query()
                },
            )
            .unwrap();
            assert_eq!(next.included_demo_hashes, original.included_demo_hashes);
            assert_eq!(next.stats, original.stats);
        }
        let mut wrong = history_match(1, 20_000);
        wrong.series = series("parser-b");
        assert!(
            build_baseline(&[wrong], query())
                .unwrap()
                .included_match_ids
                .is_empty()
        );
    }

    #[test]
    fn partial_and_unordered_matches_expose_machine_readable_exclusions() {
        let mut preview = history_match(1, 10_000);
        preview.trend_exclusion = Some(MatchTrendExclusion::Preview);
        let mut missing_time = history_match(2, 10_000);
        missing_time.interval = None;
        let receipt = build_baseline(&[preview, missing_time], query()).unwrap();
        assert_eq!(
            receipt.exclusions,
            vec![
                CohortExclusion {
                    match_id: "match-01".into(),
                    reason: CohortExclusionReason::MatchRule(MatchTrendExclusion::Preview),
                },
                CohortExclusion {
                    match_id: "match-02".into(),
                    reason: CohortExclusionReason::MissingChronology,
                },
            ]
        );
    }

    #[test]
    fn sessions_use_the_exact_ninety_minute_boundary() {
        let mut first = history_match(1, 10_000);
        first.interval = Some(MatchInterval {
            start_utc_ms: 0,
            end_utc_ms: 1_000,
        });
        let mut boundary = history_match(2, 10_000);
        boundary.interval = Some(MatchInterval {
            start_utc_ms: 1_000 + SESSION_GAP_MS,
            end_utc_ms: 2_000 + SESSION_GAP_MS,
        });
        let mut later = history_match(3, 10_000);
        later.interval = Some(MatchInterval {
            start_utc_ms: 2_000 + 2 * SESSION_GAP_MS + 1,
            end_utc_ms: 3_000 + 2 * SESSION_GAP_MS + 1,
        });
        let grouped = group_sessions(&[later, boundary, first]);
        assert_eq!(grouped.sessions.len(), 2);
        assert_eq!(grouped.sessions[0].match_ids.len(), 2);
    }

    #[test]
    fn goal_uses_exactly_the_first_ten_qualifying_future_matches() {
        let goal = Goal::rating(
            "goal",
            0,
            series("parser-a"),
            BaselineScope::AllMaps,
            RatingBasisPoints::new(11_000).unwrap(),
            vec!["baseline".into()],
        );
        let nine: Vec<_> = (1..=9).map(|i| history_match(i, 11_000)).collect();
        assert!(matches!(
            evaluate_goal(&goal, &series("parser-a"), &nine)
                .unwrap()
                .status,
            GoalStatus::InProgress { selected: 9, .. }
        ));
        let ten: Vec<_> = (1..=10).map(|i| history_match(i, 11_000)).collect();
        assert_eq!(
            evaluate_goal(&goal, &series("parser-a"), &ten)
                .unwrap()
                .status,
            GoalStatus::Achieved
        );
        assert_eq!(
            evaluate_goal(&goal, &series("parser-b"), &ten)
                .unwrap()
                .status,
            GoalStatus::VersionChanged
        );
    }

    #[test]
    fn side_context_cannot_create_a_map_side_cohort_or_rating() {
        let result = build_side_component_lens(
            &[],
            BaselineQuery {
                scope: BaselineScope::Map("de_mirage".into()),
                ..query()
            },
            Side::Terrorist,
            ComponentKind::DirectDamage,
        );
        assert_eq!(result, Err(BaselineError::MapSideScopeForbidden));
    }
}
