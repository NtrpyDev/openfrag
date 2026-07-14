use crate::{
    CandidateRecord, CaptureKind, CaptureRecord, CaptureStatus, EvidenceStore, LiveDiagnostic,
};
use openfrag_capture::SaveDisposition;
use openfrag_domain::CandidateTrigger;
use openfrag_gsi::EvidenceReceipt;
use openfrag_storage::{CaptureSessionId, LiveCandidateId, SaveAttemptId, Storage};
use std::{collections::HashMap, sync::Mutex};

#[derive(Clone)]
struct ReceiptMetadata {
    snapshot_sha256: String,
    sequence: i64,
    received_at_ns: i64,
}

struct CandidateMetadata {
    id: LiveCandidateId,
    record: CandidateRecord,
}

struct State {
    storage: Storage,
    receipts: HashMap<String, ReceiptMetadata>,
    candidates: HashMap<String, CandidateMetadata>,
    save_attempts: HashMap<String, SaveAttemptId>,
}

/// Conservative adapter from live coordination evidence to the existing `SQLite` contract.
///
/// States without a lossless representation are rejected with [`LiveDiagnostic::Unsupported`].
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
        let CaptureStatus::SaveRequested(SaveDisposition::Signalled) = capture.status else {
            return Err(LiveDiagnostic::Unsupported(match capture.status {
                CaptureStatus::Scheduled => "scheduled capture",
                CaptureStatus::Requesting => "transient requesting capture",
                CaptureStatus::SaveRequested(SaveDisposition::Coalesced) => {
                    "coalesced recorder request relationship"
                }
                CaptureStatus::Retryable(_) => "retryable capture lifecycle",
                CaptureStatus::Cancelled(_) => "cancelled capture",
                CaptureStatus::SaveRequested(SaveDisposition::Signalled) => unreachable!(),
            }));
        };
        if capture.kind != CaptureKind::AutoRoundEnd {
            return Err(LiveDiagnostic::Unsupported(
                "manual flag to save-attempt relationship",
            ));
        }
        let candidate = capture
            .candidate
            .as_ref()
            .ok_or(LiveDiagnostic::Unsupported(
                "auto capture without candidate",
            ))?;
        let window = capture.window.ok_or(LiveDiagnostic::Unsupported(
            "save request without capture window",
        ))?;
        let requested_at_ms = capture.raw_coverage.map(|coverage| coverage.end_ms).ok_or(
            LiveDiagnostic::Unsupported("save request without raw coverage"),
        )?;
        let record = CandidateRecord {
            candidate: candidate.clone(),
            final_labels: capture.final_labels.clone(),
        };
        let mut state = self.lock()?;
        let candidate_id = persist_candidate(&mut state, &self.session, &record)?;
        let requested_ns = millis_u64_to_nanos(requested_at_ms)?;
        let attempt = state
            .storage
            .request_save(&self.session, &capture.id, requested_ns)
            .map_err(persistence)?;
        state
            .storage
            .join_candidate_save(
                &candidate_id,
                &attempt,
                millis_u64_to_nanos(window.desired.start_ms)?,
                millis_u64_to_nanos(window.desired.end_ms)?,
            )
            .map_err(persistence)?;
        state.save_attempts.insert(capture.id.clone(), attempt);
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
        if existing.record == *record {
            return Ok(existing.id.clone());
        }
        return Err(LiveDiagnostic::Unsupported(
            "candidate mutation cannot be represented by the insert-only seam",
        ));
    }
    if candidate.receipt_ids.len() != 1 || candidate.triggers.len() != 1 {
        return Err(LiveDiagnostic::Unsupported(
            "candidate receipts and triggers lack a lossless association",
        ));
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
    let trigger = *candidate
        .triggers
        .first()
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
    state.candidates.insert(
        candidate.round_id.clone(),
        CandidateMetadata {
            id: id.clone(),
            record: record.clone(),
        },
    );
    Ok(id)
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
