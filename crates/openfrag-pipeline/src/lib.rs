//! Durable application service for local Demo import, parsing, and rating analysis.

use openfrag_analysis::{AnalysisReceipt, AnalysisUnavailable, analyze};
use openfrag_import::{MAX_DEMO_BYTES, ParsedOutput};
use openfrag_storage::{AnalysisIdentity, ImportJobId, ImportPhase, Layout, Storage};
use std::{fs, io::Read, path::Path};

const DEMO_STAMP: &[u8] = b"PBDEMS2\0";

pub trait ParserBackend {
    /// Parses one validated, content-addressed Demo.
    ///
    /// # Errors
    /// Returns a stable parser error description when evidence extraction fails.
    fn parse(&self, path: &Path) -> Result<ParsedOutput, String>;
}

#[derive(Clone, Copy, Debug)]
pub struct ImportRequest<'a> {
    pub source: &'a Path,
    pub local_steam_id: u64,
    pub worker: &'a str,
    pub lease_expires_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineError {
    InvalidSource,
    UnsupportedDemo,
    Size,
    InsufficientHeadroom,
    SourceChanged,
    Parser(String),
    Storage(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportOutcome {
    Deduplicated {
        job_id: ImportJobId,
        demo_sha256: String,
    },
    Ready {
        job_id: ImportJobId,
        demo_sha256: String,
        rating_bp: u16,
    },
    RatingUnavailable {
        job_id: ImportJobId,
        demo_sha256: String,
        reason: AnalysisUnavailable,
    },
}

pub struct ImportService<'a, B> {
    storage: &'a mut Storage,
    parser: B,
}

impl<'a, B: ParserBackend> ImportService<'a, B> {
    pub fn new(storage: &'a mut Storage, parser: B) -> Self {
        Self { storage, parser }
    }

    /// Runs a leased import through its durable terminal commit.
    ///
    /// # Errors
    /// Returns typed validation, parser, or persistence failures. Failed leased jobs are marked failed.
    pub fn import(&mut self, request: ImportRequest<'_>) -> Result<ImportOutcome, PipelineError> {
        let metadata = validate_source(request.source, self.storage.layout())?;
        let before_modified = metadata.modified().ok();
        let mut source =
            fs::File::open(request.source).map_err(|_| PipelineError::InvalidSource)?;
        let staged = self
            .storage
            .stage_from_reader(&mut source)
            .map_err(storage_error)?;
        let after = fs::metadata(request.source).map_err(|_| PipelineError::SourceChanged)?;
        if after.len() != metadata.len() || after.modified().ok() != before_modified {
            return Err(PipelineError::SourceChanged);
        }
        let demo_sha256 = self
            .storage
            .commit_artifact(staged, "dem", Some("application/x-counter-strike-2-demo"))
            .map_err(storage_error)?;
        let job = self
            .storage
            .enqueue_import(&demo_sha256)
            .map_err(storage_error)?;
        self.storage
            .lease_import(&job, request.worker, request.lease_expires_at_ms)
            .map_err(storage_error)?;
        self.storage
            .update_import_progress(
                &job,
                request.worker,
                Some(i64::try_from(metadata.len()).unwrap_or(i64::MAX)),
                Some(i64::try_from(metadata.len()).unwrap_or(i64::MAX)),
                3_000,
                now_ms(),
            )
            .map_err(storage_error)?;
        match self.run_leased(&job, &demo_sha256, request) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let _ = self.storage.finish_import(
                    &job,
                    request.worker,
                    ImportPhase::Failed,
                    Some(error.code()),
                    Some("manual_retry_or_quarantine"),
                    None,
                );
                Err(error)
            }
        }
    }

    #[allow(clippy::collapsible_if, clippy::too_many_lines)]
    fn run_leased(
        &mut self,
        job: &ImportJobId,
        demo_sha256: &str,
        request: ImportRequest<'_>,
    ) -> Result<ImportOutcome, PipelineError> {
        let artifact = self.storage.artifact(demo_sha256).map_err(storage_error)?;
        let relative = artifact
            .relative_path
            .ok_or(PipelineError::Storage("artifact path absent".into()))?;
        let parsed = match self
            .parser
            .parse(&self.storage.layout().root.join(relative))
        {
            Ok(parsed) => parsed,
            Err(error) => {
                self.storage
                    .quarantine_artifact(demo_sha256, "parser_corrupt")
                    .map_err(storage_error)?;
                return Err(PipelineError::Parser(error));
            }
        };
        if parsed.participants.is_empty() {
            self.storage
                .quarantine_artifact(demo_sha256, "suspicious_empty_roster")
                .map_err(storage_error)?;
            return Err(PipelineError::Parser(
                "empty participant roster quarantined".into(),
            ));
        }
        self.storage
            .update_import_progress(job, request.worker, None, None, 7_000, now_ms())
            .map_err(storage_error)?;
        let identity = AnalysisIdentity {
            parser_commit: &parsed.identity.parser_commit,
            parser_build: &parsed.identity.parser_build,
            generated_proto_build: &parsed.identity.generated_proto_build,
            requested_schema_hash: &parsed.identity.requested_schema_hash,
            metric_definition_version: &parsed.identity.metric_definition_version,
            formula_id: &parsed.identity.formula_version,
            evidence_semantics_epoch: &parsed.identity.evidence_semantics_epoch,
        };
        let existing_match = self
            .storage
            .match_for_demo(demo_sha256)
            .map_err(storage_error)?;
        if let Some(match_id) = &existing_match {
            if self
                .storage
                .completed_analysis_for_identity(match_id, &identity)
                .map_err(storage_error)?
            {
                self.storage
                    .finish_import(
                        job,
                        request.worker,
                        ImportPhase::Succeeded,
                        None,
                        None,
                        None,
                    )
                    .map_err(storage_error)?;
                return Ok(ImportOutcome::Deduplicated {
                    job_id: job.clone(),
                    demo_sha256: demo_sha256.into(),
                });
            }
        }
        let (match_id, is_new) = match existing_match {
            Some(match_id) => (match_id, false),
            None => (
                self.storage
                    .import_match(
                        demo_sha256,
                        &request.local_steam_id.to_string(),
                        parsed.metadata.map.as_deref(),
                    )
                    .map_err(storage_error)?,
                true,
            ),
        };
        if is_new {
            for participant in &parsed.participants {
                let steam_id = participant.steam_id.to_string();
                self.storage
                    .upsert_player(&steam_id, participant.name.as_deref())
                    .map_err(storage_error)?;
                self.storage
                    .add_match_participant(&match_id, &steam_id, "unknown")
                    .map_err(storage_error)?;
            }
        }
        let run = self
            .storage
            .begin_analysis(&match_id, &identity)
            .map_err(storage_error)?;
        let outcome = match analyze(&parsed, request.local_steam_id) {
            Ok(analysis) => {
                self.persist_available(job, &run, demo_sha256, request.local_steam_id, &analysis)?
            }
            Err(reason) => {
                self.persist_unavailable(&run, &parsed, request.local_steam_id, &reason)?;
                ImportOutcome::RatingUnavailable {
                    job_id: job.clone(),
                    demo_sha256: demo_sha256.into(),
                    reason,
                }
            }
        };
        self.storage
            .complete_analysis(&run)
            .map_err(storage_error)?;
        self.storage
            .update_import_progress(job, request.worker, None, None, 10_000, now_ms())
            .map_err(storage_error)?;
        self.storage
            .finish_import(
                job,
                request.worker,
                ImportPhase::Succeeded,
                None,
                None,
                None,
            )
            .map_err(storage_error)?;
        Ok(outcome)
    }

    fn persist_unavailable(
        &self,
        run: &openfrag_storage::AnalysisRunId,
        parsed: &ParsedOutput,
        local: u64,
        reason: &AnalysisUnavailable,
    ) -> Result<(), PipelineError> {
        for round in &parsed.rounds {
            let winner = normalize_winner(round.winner.as_deref());
            self.storage
                .add_round(
                    run,
                    i64::try_from(round.number).map_err(|_| PipelineError::Size)?,
                    i64::from(round.end_tick),
                    winner,
                )
                .map_err(storage_error)?;
        }
        let payload = serde_json::json!({"reason": format!("{reason:?}"), "source_sha256": parsed.identity.source_sha256}).to_string();
        let receipt = self
            .storage
            .add_receipt(
                run,
                None,
                "rating_unavailable",
                None,
                None,
                "[]",
                &payload,
                "[]",
                "{\"formula\":\"ofr-1.0.0\"}",
            )
            .map_err(storage_error)?;
        self.storage
            .persist_unavailable_rating(
                run,
                &local.to_string(),
                "ofr-1.0.0",
                &format!("{reason:?}"),
                &receipt,
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn persist_available(
        &self,
        job: &ImportJobId,
        run: &openfrag_storage::AnalysisRunId,
        demo_sha256: &str,
        local: u64,
        analysis: &AnalysisReceipt,
    ) -> Result<ImportOutcome, PipelineError> {
        let mut stored_rounds = Vec::new();
        for round in &analysis.rounds {
            let id = self
                .storage
                .add_round(
                    run,
                    i64::try_from(round.number).map_err(|_| PipelineError::Size)?,
                    i64::from(round.end_tick),
                    winner_label(round.winner),
                )
                .map_err(storage_error)?;
            let payload = serde_json::json!({"eligible": round.eligible, "exclusion": format!("{:?}", round.exclusion), "evidence": round.ordered_evidence_hashes}).to_string();
            let receipt = self
                .storage
                .add_receipt(
                    run,
                    Some(&id),
                    "round_ledger",
                    Some(i64::from(round.end_tick)),
                    Some(i64::try_from(round.number).unwrap_or(i64::MAX)),
                    "[]",
                    &payload,
                    "[]",
                    "{\"formula\":\"ofr-1.0.0\"}",
                )
                .map_err(storage_error)?;
            self.storage
                .write_round_metric(
                    run,
                    &id,
                    &local.to_string(),
                    "eligible",
                    i64::from(round.eligible),
                    1,
                    Some(&receipt),
                )
                .map_err(storage_error)?;
            stored_rounds.push(id);
        }
        let rating_bp = analysis
            .rating_receipt
            .rating_bp
            .ok_or(PipelineError::Parser("domain returned no rating".into()))?
            .get();
        let payload = serde_json::json!({"rating_bp": rating_bp, "metrics": format!("{:?}", analysis.rating_input.metrics), "demo_sha256": demo_sha256}).to_string();
        let rating_receipt = self
            .storage
            .add_receipt(
                run,
                None,
                "ofr-1.0.0",
                None,
                None,
                &format!("[\"{local}\"]"),
                &payload,
                "[]",
                "{\"scale\":\"basis_points\"}",
            )
            .map_err(storage_error)?;
        let vector = self
            .storage
            .persist_available_rating(
                run,
                &local.to_string(),
                "ofr-1.0.0",
                i64::from(rating_bp),
                &rating_receipt,
            )
            .map_err(storage_error)?;
        if let Some(components) = &analysis.rating_receipt.component_receipts {
            for component in components {
                let value_bp =
                    component.exact_numerator.saturating_mul(10_000) / component.exact_denominator;
                let weight_bp = component.weight.numerator().saturating_mul(10_000)
                    / component.weight.denominator();
                let component_receipt = self.storage.add_receipt(run, None, &format!("component:{:?}", component.kind), None, None, &format!("[\"{local}\"]"), &serde_json::json!({"numerator": component.exact_numerator, "denominator": component.exact_denominator}).to_string(), "[]", &serde_json::json!({"weight_numerator": component.weight.numerator(), "weight_denominator": component.weight.denominator()}).to_string()).map_err(storage_error)?;
                self.storage
                    .persist_rating_component(
                        &vector,
                        &format!("{:?}", component.kind),
                        component.exact_numerator,
                        component.exact_denominator,
                        value_bp,
                        weight_bp,
                        &component_receipt,
                    )
                    .map_err(storage_error)?;
            }
        }
        Ok(ImportOutcome::Ready {
            job_id: job.clone(),
            demo_sha256: demo_sha256.into(),
            rating_bp,
        })
    }

    /// Cancels a currently leased job owned by `worker`.
    ///
    /// # Errors
    /// Returns a storage transition error when the lease is absent or owned elsewhere.
    pub fn cancel(&self, job: &ImportJobId, worker: &str) -> Result<(), PipelineError> {
        self.storage
            .finish_import(
                job,
                worker,
                ImportPhase::Cancelled,
                Some("cancelled"),
                None,
                None,
            )
            .map_err(storage_error)
    }

    /// Requeues a failed or cancelled job with its manual retry budget reset.
    ///
    /// # Errors
    /// Returns a storage transition error when the job is not retryable.
    pub fn retry(&self, job: &ImportJobId) -> Result<(), PipelineError> {
        self.storage.reset_import_retry(job).map_err(storage_error)
    }

    /// Requeues leases that expired before `now_ms` for crash recovery.
    ///
    /// # Errors
    /// Returns a persistence error when recovery cannot be committed.
    pub fn recover_expired(&self, now_ms: i64) -> Result<usize, PipelineError> {
        self.storage
            .recover_expired_imports(now_ms)
            .map_err(storage_error)
    }
}

fn validate_source(path: &Path, layout: &Layout) -> Result<fs::Metadata, PipelineError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PipelineError::InvalidSource)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || path.extension().and_then(|value| value.to_str()) != Some("dem")
    {
        return Err(PipelineError::InvalidSource);
    }
    if metadata.len() < DEMO_STAMP.len() as u64 || metadata.len() > MAX_DEMO_BYTES {
        return Err(PipelineError::Size);
    }
    let mut stamp = [0_u8; 8];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut stamp))
        .map_err(|_| PipelineError::InvalidSource)?;
    if stamp != DEMO_STAMP {
        return Err(PipelineError::UnsupportedDemo);
    }
    let available =
        fs2::available_space(&layout.root).map_err(|_| PipelineError::InsufficientHeadroom)?;
    if available < metadata.len().saturating_add(metadata.len() / 10) {
        return Err(PipelineError::InsufficientHeadroom);
    }
    Ok(metadata)
}

#[allow(clippy::needless_pass_by_value)]
fn storage_error(error: openfrag_storage::Error) -> PipelineError {
    PipelineError::Storage(format!("{error:?}"))
}
fn normalize_winner(winner: Option<&str>) -> Option<&'static str> {
    match winner {
        Some("2" | "T") => Some("T"),
        Some("3" | "CT") => Some("CT"),
        _ => None,
    }
}
fn winner_label(winner: i64) -> Option<&'static str> {
    match winner {
        2 => Some("T"),
        3 => Some("CT"),
        _ => None,
    }
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

impl PipelineError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidSource => "invalid_source",
            Self::UnsupportedDemo => "unsupported_demo",
            Self::Size => "size",
            Self::InsufficientHeadroom => "headroom",
            Self::SourceChanged => "source_changed",
            Self::Parser(_) => "parser",
            Self::Storage(_) => "storage",
        }
    }
}

#[cfg(feature = "demoparser")]
#[derive(Clone, Copy, Debug, Default)]
pub struct PinnedParser;

#[cfg(feature = "demoparser")]
impl ParserBackend for PinnedParser {
    fn parse(&self, path: &Path) -> Result<ParsedOutput, String> {
        openfrag_import::parse_with_pinned_demoparser(path).map_err(|error| format!("{error:?}"))
    }
}
