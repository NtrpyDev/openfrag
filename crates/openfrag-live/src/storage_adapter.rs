use crate::{
    CandidateRecord, CaptureKind, CaptureRecord, CaptureStatus, EvidenceStore, LiveDiagnostic,
    MANUAL_FLAG_SAVE_JOIN_REQUIREMENT,
};
use openfrag_capture::SaveDisposition;
use openfrag_domain::CandidateTrigger;
use openfrag_gsi::EvidenceReceipt;
use openfrag_storage::{CaptureSessionId, LiveCandidateId, ManualFlagId, SaveAttemptId, Storage};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

#[derive(Clone)]
struct ReceiptMetadata {
    snapshot_sha256: String,
    sequence: i64,
    received_at_ns: i64,
    trigger: Option<CandidateTrigger>,
}

struct CandidateMetadata {
    id: LiveCandidateId,
    record: CandidateRecord,
    trigger_receipts: HashSet<String>,
}

struct SaveAttemptMetadata {
    id: SaveAttemptId,
    generation: u64,
    failed: bool,
}

struct State {
    storage: Storage,
    receipts: HashMap<String, ReceiptMetadata>,
    candidates: HashMap<String, CandidateMetadata>,
    save_attempts: HashMap<String, SaveAttemptMetadata>,
    manual_flags: HashMap<String, ManualFlagId>,
    candidate_joins: HashSet<(String, String)>,
}

/// Conservative adapter from live coordination evidence to the existing `SQLite` contract.
///
/// Live coordinator transitions are projected onto the durable `SQLite` evidence model.
pub struct StorageEvidenceStore {
    session: CaptureSessionId,
    state: Mutex<State>,
}

impl StorageEvidenceStore {
    /// Creates a durable capture session owned by this adapter.
    ///
    /// # Errors
    ///
    /// Returns a persistence diagnostic when the session cannot be created.
    pub fn new(storage: Storage, local_steam_id: &str) -> Result<Self, LiveDiagnostic> {
        let session = storage
            .create_capture_session(local_steam_id)
            .map_err(persistence)?;
        Ok(Self {
            session,
            state: Mutex::new(State {
                storage,
                receipts: HashMap::new(),
                candidates: HashMap::new(),
                save_attempts: HashMap::new(),
                manual_flags: HashMap::new(),
                candidate_joins: HashSet::new(),
            }),
        })
    }

    #[must_use]
    pub fn capture_session_id(&self) -> &str {
        self.session.as_str()
    }
}

impl EvidenceStore for StorageEvidenceStore {
    fn persist_receipt(&self, receipt: &EvidenceReceipt) -> Result<(), LiveDiagnostic> {
        let id = receipt_id(receipt);
        let mut state = self.lock()?;
        if state.receipts.contains_key(&id) {
            return Ok(());
        }
        let bytes = serde_json::to_vec(receipt)
            .map_err(|error| LiveDiagnostic::Persistence(error.to_string()))?;
        let staged = state.storage.stage_artifact(&bytes).map_err(persistence)?;
        let snapshot_sha256 = state
            .storage
            .commit_artifact(staged, "json", Some("application/json"))
            .map_err(persistence)?;
        let sequence = i64::try_from(receipt.sequence)
            .map_err(|_| LiveDiagnostic::Unsupported("GSI sequence exceeds SQLite range"))?;
        let received_at_ms = duration_ms(receipt)?;
        let presence = serde_json::to_string(&receipt.presence)
            .map_err(|error| LiveDiagnostic::Persistence(error.to_string()))?;
        state
            .storage
            .record_gsi_snapshot(
                &self.session,
                &snapshot_sha256,
                sequence,
                received_at_ms,
                &presence,
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(persistence)?;
        state.receipts.insert(
            id,
            ReceiptMetadata {
                snapshot_sha256,
                sequence,
                received_at_ns: millis_to_nanos(received_at_ms)?,
                trigger: receipt_trigger(receipt),
            },
        );
        Ok(())
    }

    fn upsert_candidate(&self, candidate: &CandidateRecord) -> Result<(), LiveDiagnostic> {
        let mut state = self.lock()?;
        persist_candidate(&mut state, &self.session, candidate).map(|_| ())
    }

    fn upsert_capture(&self, capture: &CaptureRecord) -> Result<(), LiveDiagnostic> {
        if !capture.final_labels.is_empty() {
            return Err(LiveDiagnostic::Unsupported(
                "final highlight labels have no live capture column",
            ));
        }
        let mut state = self.lock()?;
        let candidate_id = match &capture.candidate {
            Some(candidate) => Some(persist_candidate(
                &mut state,
                &self.session,
                &CandidateRecord {
                    candidate: candidate.clone(),
                    final_labels: capture.final_labels.clone(),
                },
            )?),
            None => None,
        };
        if capture.status == CaptureStatus::Scheduled {
            return Ok(());
        }
        if matches!(capture.status, CaptureStatus::Cancelled(_))
            && !state.save_attempts.contains_key(&capture.id)
        {
            return Ok(());
        }
        let window = capture
            .window
            .ok_or(LiveDiagnostic::Unsupported("capture window"))?;
        let requested_at_ms = capture
            .raw_coverage
            .map_or(window.requested_at_ms, |coverage| coverage.end_ms);
        let attempt = ensure_attempt(&mut state, &self.session, capture, requested_at_ms)?;
        if let Some(candidate_id) = candidate_id {
            join_candidate_once(&mut state, &candidate_id, &attempt, window)?;
        }
        if capture.kind == CaptureKind::ManualFlag {
            ensure_manual_flag(&mut state, &self.session, capture, window)?;
            return Err(LiveDiagnostic::StorageRequirement(
                MANUAL_FLAG_SAVE_JOIN_REQUIREMENT,
            ));
        }
        match &capture.status {
            CaptureStatus::Requesting
            | CaptureStatus::SaveRequested(SaveDisposition::Signalled) => {}
            CaptureStatus::SaveRequested(SaveDisposition::Coalesced) => state
                .storage
                .acknowledge_save(&attempt)
                .map_err(persistence)?,
            CaptureStatus::Retryable(message) => {
                state
                    .storage
                    .fail_save(&attempt, message)
                    .map_err(persistence)?;
                if let Some(metadata) = state.save_attempts.get_mut(&capture.id) {
                    metadata.failed = true;
                }
            }
            CaptureStatus::Cancelled(reason) => {
                state
                    .storage
                    .fail_save(&attempt, &format!("cancelled:{reason:?}"))
                    .map_err(persistence)?;
                if let Some(metadata) = state.save_attempts.get_mut(&capture.id) {
                    metadata.failed = true;
                }
            }
            CaptureStatus::Scheduled => unreachable!(),
        }
        Ok(())
    }
}

impl StorageEvidenceStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, LiveDiagnostic> {
        self.state
            .lock()
            .map_err(|_| LiveDiagnostic::Persistence("storage adapter lock poisoned".into()))
    }
}

#[allow(clippy::too_many_lines)]
fn persist_candidate(
    state: &mut State,
    session: &CaptureSessionId,
    record: &CandidateRecord,
) -> Result<LiveCandidateId, LiveDiagnostic> {
    let candidate = &record.candidate;
    if candidate.capture_session_id != session.as_str() {
        return Err(LiveDiagnostic::Unsupported(
            "candidate belongs to another capture session",
        ));
    }
    if !record.final_labels.is_empty() {
        return Err(LiveDiagnostic::Unsupported(
            "final highlight labels have no live candidate column",
        ));
    }
    if let Some(existing) = state.candidates.get(&candidate.round_id) {
        let id = existing.id.clone();
        let mut persisted = existing.trigger_receipts.clone();
        for receipt_id in &candidate.receipt_ids {
            if persisted.contains(receipt_id) {
                continue;
            }
            let receipt = state
                .receipts
                .get(receipt_id)
                .ok_or(LiveDiagnostic::Unsupported(
                    "candidate receipt is not hydrated in this adapter",
                ))?;
            let trigger = receipt
                .trigger
                .ok_or(LiveDiagnostic::Unsupported("candidate receipt trigger"))?;
            state
                .storage
                .add_candidate_trigger(
                    &id,
                    &receipt.snapshot_sha256,
                    receipt.sequence,
                    trigger_name(trigger),
                    receipt.received_at_ns,
                )
                .map_err(persistence)?;
            persisted.insert(receipt_id.clone());
        }
        if let Some(existing) = state.candidates.get_mut(&candidate.round_id) {
            existing.record = record.clone();
            existing.trigger_receipts = persisted;
        }
        return Ok(id);
    }
    let receipt_id = candidate
        .receipt_ids
        .first()
        .ok_or(LiveDiagnostic::Unsupported("candidate without receipt"))?;
    let receipt = state
        .receipts
        .get(receipt_id)
        .cloned()
        .ok_or(LiveDiagnostic::Unsupported(
            "candidate receipt is not hydrated in this adapter",
        ))?;
    let trigger = receipt
        .trigger
        .or_else(|| candidate.triggers.first().copied())
        .ok_or(LiveDiagnostic::Unsupported("candidate without trigger"))?;
    let (observed_map, observed_round) = parse_round_id(&candidate.round_id)?;
    let id = state
        .storage
        .create_candidate(
            session,
            &candidate.rule_version,
            trigger_name(trigger),
            Some(observed_map),
            Some(observed_round),
            receipt.received_at_ns,
            receipt.received_at_ns,
            millis_u64_to_nanos(candidate.range.start_ms)?,
            millis_u64_to_nanos(candidate.range.end_ms)?,
        )
        .map_err(persistence)?;
    let mut trigger_receipts = HashSet::new();
    for receipt_id in &candidate.receipt_ids {
        let receipt = state
            .receipts
            .get(receipt_id)
            .ok_or(LiveDiagnostic::Unsupported(
                "candidate receipt is not hydrated in this adapter",
            ))?;
        let trigger = receipt
            .trigger
            .ok_or(LiveDiagnostic::Unsupported("candidate receipt trigger"))?;
        state
            .storage
            .add_candidate_trigger(
                &id,
                &receipt.snapshot_sha256,
                receipt.sequence,
                trigger_name(trigger),
                receipt.received_at_ns,
            )
            .map_err(persistence)?;
        trigger_receipts.insert(receipt_id.clone());
    }
    state.candidates.insert(
        candidate.round_id.clone(),
        CandidateMetadata {
            id: id.clone(),
            record: record.clone(),
            trigger_receipts,
        },
    );
    Ok(id)
}

fn ensure_attempt(
    state: &mut State,
    session: &CaptureSessionId,
    capture: &CaptureRecord,
    requested_at_ms: u64,
) -> Result<SaveAttemptId, LiveDiagnostic> {
    if let Some(existing) = state.save_attempts.get(&capture.id)
        && !existing.failed
    {
        return Ok(existing.id.clone());
    }
    let generation = state
        .save_attempts
        .get(&capture.id)
        .map_or(0, |existing| existing.generation.saturating_add(1));
    let request_id = if generation == 0 {
        capture.id.clone()
    } else {
        format!("{}:retry:{generation}", capture.id)
    };
    let attempt = state
        .storage
        .request_save(session, &request_id, millis_u64_to_nanos(requested_at_ms)?)
        .map_err(persistence)?;
    state.save_attempts.insert(
        capture.id.clone(),
        SaveAttemptMetadata {
            id: attempt.clone(),
            generation,
            failed: false,
        },
    );
    Ok(attempt)
}

fn join_candidate_once(
    state: &mut State,
    candidate: &LiveCandidateId,
    attempt: &SaveAttemptId,
    window: openfrag_domain::CaptureWindow,
) -> Result<(), LiveDiagnostic> {
    let key = (candidate.as_str().to_owned(), attempt.as_str().to_owned());
    if state.candidate_joins.contains(&key) {
        return Ok(());
    }
    state
        .storage
        .join_candidate_save(
            candidate,
            attempt,
            millis_u64_to_nanos(window.desired.start_ms)?,
            millis_u64_to_nanos(window.desired.end_ms)?,
        )
        .map_err(persistence)?;
    state.candidate_joins.insert(key);
    Ok(())
}

fn ensure_manual_flag(
    state: &mut State,
    session: &CaptureSessionId,
    capture: &CaptureRecord,
    window: openfrag_domain::CaptureWindow,
) -> Result<ManualFlagId, LiveDiagnostic> {
    if let Some(flag) = state.manual_flags.get(&capture.id) {
        return Ok(flag.clone());
    }
    let flag = state
        .storage
        .create_manual_flag(session, millis_u64_to_nanos(window.requested_at_ms)?)
        .map_err(persistence)?;
    state.manual_flags.insert(capture.id.clone(), flag.clone());
    Ok(flag)
}

fn receipt_trigger(receipt: &EvidenceReceipt) -> Option<CandidateTrigger> {
    receipt.facts.iter().find_map(|fact| match fact {
        openfrag_gsi::TransitionFact::RoundKillDelta { current, delta, .. }
            if *current >= 3 && *delta > 0 =>
        {
            Some(CandidateTrigger::KillMilestone)
        }
        openfrag_gsi::TransitionFact::RoundEnd { .. } => {
            Some(CandidateTrigger::OfficialRoundEndCapture)
        }
        _ => None,
    })
}

fn parse_round_id(round_id: &str) -> Result<(&str, i64), LiveDiagnostic> {
    let (map, round) = round_id
        .rsplit_once(':')
        .ok_or(LiveDiagnostic::Unsupported("candidate round identity"))?;
    let round = round
        .parse::<i64>()
        .map_err(|_| LiveDiagnostic::Unsupported("candidate round number"))?;
    if map.is_empty() || round < 0 {
        return Err(LiveDiagnostic::Unsupported("candidate round identity"));
    }
    Ok((map, round))
}

const fn trigger_name(trigger: CandidateTrigger) -> &'static str {
    match trigger {
        CandidateTrigger::KillMilestone => "kill_milestone",
        CandidateTrigger::PossibleKnife => "possible_knife",
        CandidateTrigger::OfficialRoundEndCapture => "official_round_end",
    }
}

fn receipt_id(receipt: &EvidenceReceipt) -> String {
    format!("{}:{}", receipt.payload_hash, receipt.sequence)
}

fn duration_ms(receipt: &EvidenceReceipt) -> Result<i64, LiveDiagnostic> {
    i64::try_from(receipt.received_at.as_millis())
        .map_err(|_| LiveDiagnostic::Unsupported("GSI timestamp exceeds SQLite range"))
}

fn millis_to_nanos(value: i64) -> Result<i64, LiveDiagnostic> {
    value
        .checked_mul(1_000_000)
        .ok_or(LiveDiagnostic::Unsupported(
            "timestamp exceeds nanosecond range",
        ))
}

fn millis_u64_to_nanos(value: u64) -> Result<i64, LiveDiagnostic> {
    let value = i64::try_from(value)
        .map_err(|_| LiveDiagnostic::Unsupported("timestamp exceeds SQLite range"))?;
    millis_to_nanos(value)
}

#[allow(clippy::needless_pass_by_value)]
fn persistence(error: openfrag_storage::Error) -> LiveDiagnostic {
    LiveDiagnostic::Persistence(format!("{error:?}"))
}
