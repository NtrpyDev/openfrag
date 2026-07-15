//! Durable application service for local Demo import, parsing, and rating analysis.

pub mod reconcile;

use openfrag_analysis::{AnalysisReceipt, AnalysisUnavailable, analyze};
use openfrag_import::{
    MAX_DEMO_BYTES, ParseDiagnostic, ParseDiagnosticCategory, ParseStage, ParsedOutput,
    ParserError, ParserProgress, ParserProgressPhase,
};
use openfrag_storage::{
    AnalysisIdentity, ImportJobId, ImportPhase, Layout, ParserProgressUpdate, Storage,
};
use reconcile::{
    ReconciliationCandidate, ReconciliationDecision, ReconciliationPersistence,
    reconcile_demo_candidates,
};
use std::{fmt::Debug, fs, io::Read, path::Path};

const DEMO_STAMP: &[u8] = b"PBDEMS2\0";

pub trait ParserBackend {
    /// Parses one validated, content-addressed Demo.
    ///
    /// # Errors
    /// Returns a stable parser error description when evidence extraction fails.
    fn parse(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, ParserError>;
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
    Parser(Box<ParseDiagnostic>),
    Storage(String),
    Reconciliation(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciledImportOutcome {
    pub import: ImportOutcome,
    pub reconciliation: Vec<ReconciliationDecision>,
}

trait PostAnalysisHook {
    fn reconcile(
        &mut self,
        parsed: &ParsedOutput,
        local_steam_id: u64,
    ) -> Result<Vec<ReconciliationDecision>, PipelineError>;
}

struct PersistenceHook<'a, P> {
    candidates: &'a [ReconciliationCandidate],
    persistence: &'a mut P,
}

impl<P> PostAnalysisHook for PersistenceHook<'_, P>
where
    P: ReconciliationPersistence,
    P::Error: Debug,
{
    fn reconcile(
        &mut self,
        parsed: &ParsedOutput,
        local_steam_id: u64,
    ) -> Result<Vec<ReconciliationDecision>, PipelineError> {
        reconcile_demo_candidates(parsed, local_steam_id, self.candidates, self.persistence)
            .map_err(|error| PipelineError::Reconciliation(format!("{error:?}")))
    }
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
        self.import_inner(request, None)
            .map(|outcome| outcome.import)
    }

    /// Runs import and persists Demo-backed reconciliation after durable analysis succeeds.
    ///
    /// # Errors
    /// Returns import failures or the injected reconciliation persistence error.
    pub fn import_and_reconcile<P>(
        &mut self,
        request: ImportRequest<'_>,
        candidates: &[ReconciliationCandidate],
        persistence: &mut P,
    ) -> Result<ReconciledImportOutcome, PipelineError>
    where
        P: ReconciliationPersistence,
        P::Error: Debug,
    {
        let mut hook = PersistenceHook {
            candidates,
            persistence,
        };
        self.import_inner(request, Some(&mut hook))
    }

    fn import_inner(
        &mut self,
        request: ImportRequest<'_>,
        hook: Option<&mut dyn PostAnalysisHook>,
    ) -> Result<ReconciledImportOutcome, PipelineError> {
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
        match self.run_leased(&job, &demo_sha256, request, hook) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let diagnostic_json = error.diagnostic_json();
                let _ = self.storage.finish_import_with_diagnostic(
                    &job,
                    request.worker,
                    ImportPhase::Failed,
                    Some(error.code()),
                    Some(error.remediation_code()),
                    None,
                    diagnostic_json.as_deref(),
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
        hook: Option<&mut dyn PostAnalysisHook>,
    ) -> Result<ReconciledImportOutcome, PipelineError> {
        let artifact = self.storage.artifact(demo_sha256).map_err(storage_error)?;
        let relative = artifact
            .relative_path
            .ok_or(PipelineError::Storage("artifact path absent".into()))?;
        let parser_path = self.storage.layout().root.join(relative);
        let storage = &*self.storage;
        let mut progress_error = None;
        let mut last_persisted = None;
        let parsed_result = self.parser.parse(&parser_path, &mut |parser_progress| {
            let progress_bp = parser_progress_basis_points(parser_progress);
            let phase_changed =
                last_persisted.is_none_or(|(phase, _)| phase != parser_progress.phase);
            let advanced = last_persisted.is_none_or(|(_, previous)| progress_bp >= previous + 10);
            if progress_error.is_none()
                && (phase_changed
                    || advanced
                    || parser_progress.phase == ParserProgressPhase::Finalize)
            {
                progress_error = storage
                    .update_parser_progress(
                        job,
                        request.worker,
                        ParserProgressUpdate {
                            phase: parser_progress.phase.code(),
                            bytes_done: i64::try_from(parser_progress.bytes_consumed)
                                .unwrap_or(i64::MAX),
                            bytes_total: i64::try_from(parser_progress.total_bytes)
                                .unwrap_or(i64::MAX),
                            frames_done: i64::try_from(parser_progress.frames).unwrap_or(i64::MAX),
                            events_emitted: i64::try_from(parser_progress.events_emitted)
                                .unwrap_or(i64::MAX),
                            progress_bp,
                            heartbeat_at_ms: now_ms(),
                        },
                    )
                    .err();
                last_persisted = Some((parser_progress.phase, progress_bp));
            }
        });
        if let Some(error) = progress_error {
            return Err(storage_error(error));
        }
        let parsed = match parsed_result {
            Ok(parsed) => parsed,
            Err(error) => {
                self.storage
                    .quarantine_artifact(demo_sha256, error.diagnostic.category.code())
                    .map_err(storage_error)?;
                return Err(PipelineError::Parser(error.diagnostic));
            }
        };
        if parsed.participants.is_empty() {
            self.storage
                .quarantine_artifact(demo_sha256, "suspicious_empty_roster")
                .map_err(storage_error)?;
            return Err(PipelineError::Parser(Box::new(internal_parse_diagnostic(
                ParseDiagnosticCategory::MissingEvidence,
                ParseStage::Finalize,
                "empty participant roster quarantined",
            ))));
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
                let reconciliation = match hook {
                    Some(hook) => hook.reconcile(&parsed, request.local_steam_id)?,
                    None => Vec::new(),
                };
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
                return Ok(ReconciledImportOutcome {
                    import: ImportOutcome::Deduplicated {
                        job_id: job.clone(),
                        demo_sha256: demo_sha256.into(),
                    },
                    reconciliation,
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
                let steam_id = participant.steam_id.get().to_string();
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
        let reconciliation = match hook {
            Some(hook) => hook.reconcile(&parsed, request.local_steam_id)?,
            None => Vec::new(),
        };
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
        Ok(ReconciledImportOutcome {
            import: outcome,
            reconciliation,
        })
    }

    fn persist_unavailable(
        &self,
        run: &openfrag_storage::AnalysisRunId,
        parsed: &ParsedOutput,
        local: u64,
        reason: &AnalysisUnavailable,
    ) -> Result<(), PipelineError> {
        for round in &parsed.rounds {
            let winner = normalize_winner(round.winner);
            self.storage
                .add_round(
                    run,
                    i64::try_from(round.number.get()).map_err(|_| PipelineError::Size)?,
                    i64::from(round.end_tick.get()),
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
            .ok_or_else(|| {
                PipelineError::Parser(Box::new(internal_parse_diagnostic(
                    ParseDiagnosticCategory::InternalInvariant,
                    ParseStage::Finalize,
                    "domain returned no rating",
                )))
            })?
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
fn normalize_winner(winner: Option<i32>) -> Option<&'static str> {
    match winner {
        Some(2) => Some("T"),
        Some(3) => Some("CT"),
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
            Self::Parser(diagnostic) => diagnostic.category.code(),
            Self::Storage(_) => "storage",
            Self::Reconciliation(_) => "reconciliation",
        }
    }

    fn remediation_code(&self) -> &'static str {
        match self {
            Self::Parser(diagnostic) => match diagnostic.category {
                ParseDiagnosticCategory::SourceIo => "retry_source_read",
                ParseDiagnosticCategory::Truncated => "replace_truncated_demo",
                ParseDiagnosticCategory::UnsupportedCommand => "unsupported_demo_version",
                ParseDiagnosticCategory::SchemaDrift => "update_parser_or_quarantine",
                ParseDiagnosticCategory::Corrupt => "replace_corrupt_demo",
                ParseDiagnosticCategory::MissingEvidence => "inspect_missing_demo_evidence",
                ParseDiagnosticCategory::InternalInvariant => "report_parser_invariant",
            },
            Self::InvalidSource | Self::UnsupportedDemo | Self::Size => "choose_valid_demo",
            Self::InsufficientHeadroom => "free_storage_space",
            Self::SourceChanged => "retry_stable_source",
            Self::Storage(_) => "repair_local_storage",
            Self::Reconciliation(_) => "retry_reconciliation",
        }
    }

    fn diagnostic_json(&self) -> Option<String> {
        match self {
            Self::Parser(diagnostic) => Some(diagnostic.to_json().to_string()),
            _ => None,
        }
    }
}

fn internal_parse_diagnostic(
    category: ParseDiagnosticCategory,
    stage: ParseStage,
    source: &str,
) -> ParseDiagnostic {
    ParseDiagnostic {
        category,
        stage,
        game_build: None,
        byte_offset: None,
        frame_index: None,
        tick: None,
        command: None,
        required_item: None,
        observed_counts: std::collections::BTreeMap::new(),
        upstream_source: source.into(),
    }
}

#[cfg(feature = "demoparser")]
#[derive(Clone, Copy, Debug, Default)]
pub struct PinnedParser;

#[cfg(feature = "demoparser")]
impl ParserBackend for PinnedParser {
    fn parse(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, ParserError> {
        openfrag_import::parse_with_pinned_demoparser_with_progress(path, progress)
    }
}

#[cfg(feature = "acceptance-fixtures")]
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceptanceParser;

#[cfg(feature = "acceptance-fixtures")]
impl ParserBackend for AcceptanceParser {
    fn parse(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(ParserProgress),
    ) -> Result<ParsedOutput, ParserError> {
        let total_bytes = std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(1);
        for (phase, fraction, events_emitted) in [
            (ParserProgressPhase::FirstPass, 1, 0),
            (ParserProgressPhase::SecondPass, 2, 48),
            (ParserProgressPhase::Finalize, 3, 48),
        ] {
            progress(ParserProgress {
                phase,
                bytes_consumed: total_bytes.saturating_mul(fraction) / 3,
                total_bytes,
                frames: 12,
                events_emitted,
            });
        }
        Ok(openfrag_analysis::fixtures::complete_parsed_output())
    }
}

fn parser_progress_basis_points(progress: ParserProgress) -> i64 {
    let fraction = (progress.bytes_consumed.min(progress.total_bytes) * 1_000)
        .checked_div(progress.total_bytes)
        .unwrap_or(0);
    let fraction = i64::try_from(fraction).unwrap_or(1_000);
    let (base, span): (i64, i64) = match progress.phase {
        ParserProgressPhase::FirstPass => (3_000, 1_000),
        ParserProgressPhase::SecondPass => (4_000, 2_500),
        ParserProgressPhase::Finalize => (6_500, 500),
    };
    base + fraction * span / 1_000
}
