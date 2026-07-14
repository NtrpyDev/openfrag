use std::collections::BTreeSet;

pub const HIGHLIGHT_RULE_VERSION: &str = "highlight-rules-1";
pub const CANDIDATE_MERGE_GAP_MS: u64 = 10_000;
pub const REPLAY_BUFFER_MS: u64 = 60_000;
pub const AUTO_POST_ROLL_MS: u64 = 10_000;
pub const DESIRED_PRE_ROLL_MS: u64 = 15_000;
pub const PROVISIONAL_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
pub const CAPTURE_SESSION_BUDGET_BYTES: u64 = 2 * 1_024 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TimeRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl TimeRange {
    pub const fn overlaps(self, other: Self) -> bool {
        self.start_ms <= other.end_ms && other.start_ms <= self.end_ms
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CandidateTrigger {
    KillMilestone,
    PossibleKnife,
    OfficialRoundEndCapture,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HighlightCandidate {
    pub capture_session_id: String,
    pub round_id: String,
    pub rule_version: String,
    pub range: TimeRange,
    pub contributor_starts_ms: BTreeSet<u64>,
    pub triggers: BTreeSet<CandidateTrigger>,
    pub receipt_ids: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightError {
    ReversedRange,
}

impl HighlightCandidate {
    pub fn new(
        capture_session_id: impl Into<String>,
        round_id: impl Into<String>,
        range: TimeRange,
        trigger: CandidateTrigger,
        receipt_id: impl Into<String>,
    ) -> Result<Self, HighlightError> {
        if range.start_ms > range.end_ms {
            return Err(HighlightError::ReversedRange);
        }
        Ok(Self {
            capture_session_id: capture_session_id.into(),
            round_id: round_id.into(),
            rule_version: HIGHLIGHT_RULE_VERSION.into(),
            range,
            contributor_starts_ms: BTreeSet::from([range.start_ms]),
            triggers: BTreeSet::from([trigger]),
            receipt_ids: BTreeSet::from([receipt_id.into()]),
        })
    }

    fn can_merge(&self, other: &Self) -> bool {
        if self.capture_session_id != other.capture_session_id
            || self.round_id != other.round_id
            || self.rule_version != other.rule_version
        {
            return false;
        }
        self.range.overlaps(other.range)
            || self.contributor_starts_ms.iter().any(|left| {
                other
                    .contributor_starts_ms
                    .iter()
                    .any(|right| left.abs_diff(*right) <= CANDIDATE_MERGE_GAP_MS)
            })
    }

    fn merged(mut self, other: Self) -> Self {
        self.range.start_ms = self.range.start_ms.min(other.range.start_ms);
        self.range.end_ms = self.range.end_ms.max(other.range.end_ms);
        self.contributor_starts_ms
            .extend(other.contributor_starts_ms);
        self.triggers.extend(other.triggers);
        self.receipt_ids.extend(other.receipt_ids);
        self
    }
}

pub fn merge_candidates(mut candidates: Vec<HighlightCandidate>) -> Vec<HighlightCandidate> {
    candidates.sort_by(candidate_order);
    candidates.dedup();
    loop {
        let mut merge_pair = None;
        'outer: for left in 0..candidates.len() {
            for right in left + 1..candidates.len() {
                if candidates[left].can_merge(&candidates[right]) {
                    merge_pair = Some((left, right));
                    break 'outer;
                }
            }
        }
        let Some((left, right)) = merge_pair else {
            break;
        };
        let other = candidates.remove(right);
        let current = candidates.remove(left);
        candidates.push(current.merged(other));
        candidates.sort_by(candidate_order);
    }
    candidates
}

fn candidate_order(left: &HighlightCandidate, right: &HighlightCandidate) -> std::cmp::Ordering {
    (
        &left.capture_session_id,
        &left.round_id,
        &left.rule_version,
        left.range,
        &left.receipt_ids,
    )
        .cmp(&(
            &right.capture_session_id,
            &right.round_id,
            &right.rule_version,
            right.range,
            &right.receipt_ids,
        ))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FinalHighlightLabel {
    Multikill,
    FourKill,
    Ace,
    Clutch,
    Knife,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DemoHighlightEvidence {
    pub enemy_kills: u8,
    pub clutch_opponents_at_start: Option<u8>,
    pub team_won_round: bool,
    pub enemy_knife_kill: bool,
}

pub fn final_labels(evidence: DemoHighlightEvidence) -> BTreeSet<FinalHighlightLabel> {
    let mut labels = BTreeSet::new();
    if evidence.enemy_kills >= 5 {
        labels.insert(FinalHighlightLabel::Ace);
    } else if evidence.enemy_kills == 4 {
        labels.insert(FinalHighlightLabel::FourKill);
    } else if evidence.enemy_kills == 3 {
        labels.insert(FinalHighlightLabel::Multikill);
    }
    if evidence.team_won_round
        && evidence
            .clutch_opponents_at_start
            .is_some_and(|count| count >= 2)
    {
        labels.insert(FinalHighlightLabel::Clutch);
    }
    if evidence.enemy_knife_kill {
        labels.insert(FinalHighlightLabel::Knife);
    }
    labels
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureWindow {
    pub requested_at_ms: u64,
    pub desired: TimeRange,
    pub available: TimeRange,
    pub pre_roll_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoCaptureDecision {
    AwaitingRoundEnd,
    Save(CaptureWindow),
}

pub fn auto_capture_window(
    candidate_start_ms: u64,
    round_end_ms: Option<u64>,
    recorder_available_from_ms: u64,
) -> AutoCaptureDecision {
    let Some(round_end_ms) = round_end_ms else {
        return AutoCaptureDecision::AwaitingRoundEnd;
    };
    let requested_at_ms = round_end_ms.saturating_add(AUTO_POST_ROLL_MS);
    let desired = TimeRange {
        start_ms: candidate_start_ms.saturating_sub(DESIRED_PRE_ROLL_MS),
        end_ms: requested_at_ms,
    };
    let buffer_start = requested_at_ms.saturating_sub(REPLAY_BUFFER_MS);
    let available_start = desired
        .start_ms
        .max(buffer_start)
        .max(recorder_available_from_ms);
    AutoCaptureDecision::Save(CaptureWindow {
        requested_at_ms,
        desired,
        available: TimeRange {
            start_ms: available_start,
            end_ms: requested_at_ms,
        },
        pre_roll_truncated: available_start > desired.start_ms,
    })
}

pub fn manual_capture_window(flag_ms: u64, recorder_available_from_ms: u64) -> CaptureWindow {
    let desired = TimeRange {
        start_ms: flag_ms.saturating_sub(DESIRED_PRE_ROLL_MS),
        end_ms: flag_ms,
    };
    let available_start = desired.start_ms.max(recorder_available_from_ms);
    CaptureWindow {
        requested_at_ms: flag_ms,
        desired,
        available: TimeRange {
            start_ms: available_start,
            end_ms: flag_ms,
        },
        pre_roll_truncated: available_start > desired.start_ms,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionalMedia {
    pub media_id: String,
    pub created_at_ms: u64,
    pub bytes: u64,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionReason {
    Retain,
    ExpiredAfterThirtyDays,
    EvictedBySessionBudget,
}

pub fn retention_decisions(
    items: &[ProvisionalMedia],
    now_ms: u64,
) -> Vec<(String, RetentionReason)> {
    let mut decisions: Vec<_> = items
        .iter()
        .map(|item| {
            let reason = if !item.confirmed
                && now_ms.saturating_sub(item.created_at_ms) >= PROVISIONAL_RETENTION_MS
            {
                RetentionReason::ExpiredAfterThirtyDays
            } else {
                RetentionReason::Retain
            };
            (item.media_id.clone(), reason)
        })
        .collect();
    let mut retained_bytes: u64 = items
        .iter()
        .zip(&decisions)
        .filter(|(_, (_, reason))| *reason == RetentionReason::Retain)
        .map(|(item, _)| item.bytes)
        .fold(0, u64::saturating_add);
    let mut candidates: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(index, item)| !item.confirmed && decisions[*index].1 == RetentionReason::Retain)
        .collect();
    candidates.sort_by_key(|(_, item)| (item.created_at_ms, &item.media_id));
    for (index, item) in candidates {
        if retained_bytes <= CAPTURE_SESSION_BUDGET_BYTES {
            break;
        }
        decisions[index].1 = RetentionReason::EvictedBySessionBudget;
        retained_bytes = retained_bytes.saturating_sub(item.bytes);
    }
    decisions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(round: &str, start: u64) -> HighlightCandidate {
        HighlightCandidate::new(
            "session",
            round,
            TimeRange {
                start_ms: start,
                end_ms: start + 1_000,
            },
            CandidateTrigger::KillMilestone,
            format!("receipt-{start}"),
        )
        .unwrap()
    }

    #[test]
    fn candidates_merge_by_same_round_overlap_or_ten_second_start_gap() {
        assert_eq!(
            merge_candidates(vec![candidate("r1", 9_000), candidate("r1", 18_000)]).len(),
            1
        );
        assert_eq!(
            merge_candidates(vec![candidate("r1", 19_000), candidate("r1", 30_000)]).len(),
            2
        );
        assert_eq!(
            merge_candidates(vec![candidate("r1", 9_000), candidate("r2", 18_000)]).len(),
            2
        );
    }

    #[test]
    fn property_candidate_merging_is_idempotent_and_order_independent() {
        let input = vec![
            candidate("r1", 27_000),
            candidate("r1", 9_000),
            candidate("r1", 18_000),
        ];
        let once = merge_candidates(input);
        let twice = merge_candidates(once.clone());
        assert_eq!(once, twice);
        assert_eq!(once.len(), 1);
    }

    #[test]
    fn demo_evidence_assigns_only_final_supported_labels() {
        let labels = final_labels(DemoHighlightEvidence {
            enemy_kills: 5,
            clutch_opponents_at_start: Some(2),
            team_won_round: true,
            enemy_knife_kill: true,
        });
        assert_eq!(
            labels,
            BTreeSet::from([
                FinalHighlightLabel::Ace,
                FinalHighlightLabel::Clutch,
                FinalHighlightLabel::Knife,
            ])
        );
    }

    #[test]
    fn capture_timing_exposes_truncation_and_never_claims_future_media() {
        let AutoCaptureDecision::Save(window) = auto_capture_window(10_000, Some(70_000), 30_000)
        else {
            panic!("round end should produce a save");
        };
        assert_eq!(window.requested_at_ms, 80_000);
        assert_eq!(
            window.desired,
            TimeRange {
                start_ms: 0,
                end_ms: 80_000
            }
        );
        assert_eq!(
            window.available,
            TimeRange {
                start_ms: 30_000,
                end_ms: 80_000
            }
        );
        assert!(window.pre_roll_truncated);
        assert_eq!(manual_capture_window(50_000, 0).available.end_ms, 50_000);
    }

    #[test]
    fn retention_expires_then_evicts_oldest_unconfirmed_media() {
        let items = vec![
            ProvisionalMedia {
                media_id: "old".into(),
                created_at_ms: 0,
                bytes: 1,
                confirmed: false,
            },
            ProvisionalMedia {
                media_id: "a".into(),
                created_at_ms: PROVISIONAL_RETENTION_MS,
                bytes: CAPTURE_SESSION_BUDGET_BYTES,
                confirmed: false,
            },
            ProvisionalMedia {
                media_id: "b".into(),
                created_at_ms: PROVISIONAL_RETENTION_MS + 1,
                bytes: 1,
                confirmed: false,
            },
        ];
        let decisions = retention_decisions(&items, PROVISIONAL_RETENTION_MS + 1);
        assert_eq!(decisions[0].1, RetentionReason::ExpiredAfterThirtyDays);
        assert_eq!(decisions[1].1, RetentionReason::EvictedBySessionBudget);
        assert_eq!(decisions[2].1, RetentionReason::Retain);
    }
}
