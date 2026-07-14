ALTER TABLE artifacts ADD COLUMN missing_reason TEXT;
ALTER TABLE capture_sessions ADD COLUMN game_build TEXT;
ALTER TABLE capture_sessions ADD COLUMN recorder_build TEXT;
ALTER TABLE clips ADD COLUMN recorded_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE clips ADD COLUMN pre_roll_truncated INTEGER NOT NULL DEFAULT 0 CHECK(pre_roll_truncated IN (0,1));
ALTER TABLE clips ADD COLUMN retention_class TEXT NOT NULL DEFAULT 'provisional' CHECK(retention_class IN ('provisional','confirmed','manual'));
ALTER TABLE matches ADD COLUMN game_build TEXT;
ALTER TABLE analysis_runs ADD COLUMN started_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE analysis_runs ADD COLUMN completed_at_ms INTEGER;
ALTER TABLE analysis_runs ADD COLUMN error_code TEXT;

CREATE TABLE gsi_snapshots (
  sha256 TEXT PRIMARY KEY REFERENCES artifacts(sha256), capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  arrival_ordinal INTEGER NOT NULL, received_at_ms INTEGER NOT NULL, provider_timestamp TEXT,
  field_presence_json TEXT NOT NULL, byte_length INTEGER NOT NULL, listener_version TEXT NOT NULL, http_status INTEGER NOT NULL,
  UNIQUE(capture_session_id, arrival_ordinal)
);
CREATE TABLE live_candidates (
  id TEXT PRIMARY KEY, capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id), rule_version TEXT NOT NULL,
  observed_kind TEXT NOT NULL, observed_map TEXT, observed_round INTEGER,
  first_transition_monotonic_ns INTEGER NOT NULL, last_transition_monotonic_ns INTEGER NOT NULL,
  candidate_start_monotonic_ns INTEGER NOT NULL, candidate_end_monotonic_ns INTEGER NOT NULL, save_requested_monotonic_ns INTEGER,
  status TEXT NOT NULL CHECK(status IN ('provisional','confirmed','unconfirmed','expired')),
  CHECK(observed_round IS NULL OR observed_round >= 0), CHECK(last_transition_monotonic_ns >= first_transition_monotonic_ns),
  CHECK(candidate_end_monotonic_ns >= candidate_start_monotonic_ns),
  UNIQUE(capture_session_id, rule_version, observed_kind, first_transition_monotonic_ns, last_transition_monotonic_ns)
);
CREATE TABLE candidate_trigger_receipts (
  candidate_id TEXT NOT NULL REFERENCES live_candidates(id), snapshot_sha256 TEXT NOT NULL REFERENCES gsi_snapshots(sha256),
  transition_ordinal INTEGER NOT NULL, transition_kind TEXT NOT NULL, transition_monotonic_ns INTEGER NOT NULL,
  PRIMARY KEY(candidate_id, snapshot_sha256, transition_ordinal)
);
CREATE TABLE candidate_save_attempts (
  candidate_id TEXT NOT NULL REFERENCES live_candidates(id), save_attempt_id TEXT NOT NULL REFERENCES recorder_save_attempts(id),
  desired_start_monotonic_ns INTEGER NOT NULL, desired_end_monotonic_ns INTEGER NOT NULL,
  PRIMARY KEY(candidate_id, save_attempt_id), CHECK(desired_end_monotonic_ns >= desired_start_monotonic_ns)
);
CREATE TABLE candidate_media (
  candidate_id TEXT NOT NULL REFERENCES live_candidates(id), save_attempt_id TEXT NOT NULL REFERENCES recorder_save_attempts(id),
  clip_id TEXT NOT NULL REFERENCES clips(id), PRIMARY KEY(candidate_id, save_attempt_id, clip_id)
);
CREATE TABLE manual_flags (
  id TEXT PRIMARY KEY, capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id), flagged_monotonic_ns INTEGER NOT NULL, created_at_ms INTEGER NOT NULL
);
CREATE TABLE manual_flag_save_attempts (
  manual_flag_id TEXT NOT NULL REFERENCES manual_flags(id), save_attempt_id TEXT NOT NULL REFERENCES recorder_save_attempts(id),
  desired_start_monotonic_ns INTEGER NOT NULL, desired_end_monotonic_ns INTEGER NOT NULL,
  PRIMARY KEY(manual_flag_id, save_attempt_id), CHECK(desired_end_monotonic_ns >= desired_start_monotonic_ns)
);
CREATE TABLE manual_flag_clips (
  manual_flag_id TEXT NOT NULL REFERENCES manual_flags(id), save_attempt_id TEXT NOT NULL REFERENCES recorder_save_attempts(id),
  clip_id TEXT NOT NULL REFERENCES clips(id), PRIMARY KEY(manual_flag_id, save_attempt_id, clip_id)
);
CREATE TABLE clip_derivations (
  derived_clip_id TEXT NOT NULL REFERENCES clips(id), source_clip_id TEXT NOT NULL REFERENCES clips(id),
  operation TEXT NOT NULL CHECK(operation IN ('consolidate','trim')), trim_start_ms INTEGER, trim_end_ms INTEGER, created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(derived_clip_id, source_clip_id), CHECK(derived_clip_id <> source_clip_id), CHECK(trim_end_ms IS NULL OR trim_start_ms IS NOT NULL), CHECK(trim_end_ms IS NULL OR trim_end_ms >= trim_start_ms)
);
CREATE TABLE clip_exports (
  id TEXT PRIMARY KEY, clip_id TEXT NOT NULL REFERENCES clips(id), target_path TEXT NOT NULL, requested_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER, status TEXT NOT NULL CHECK(status IN ('queued','running','succeeded','failed')), error_code TEXT,
  output_sha256 TEXT, output_byte_length INTEGER CHECK(output_byte_length >= 0), output_media_profile TEXT
);
CREATE TABLE players (steam_id TEXT PRIMARY KEY, display_name TEXT, first_seen_at_ms INTEGER NOT NULL, last_seen_at_ms INTEGER NOT NULL, CHECK(last_seen_at_ms >= first_seen_at_ms));
CREATE TABLE match_players (
  match_id TEXT NOT NULL REFERENCES matches(id), steam_id TEXT NOT NULL REFERENCES players(steam_id),
  team_at_end TEXT CHECK(team_at_end IN ('T','CT','spectator')), participation_status TEXT NOT NULL CHECK(participation_status IN ('full','partial','unknown')),
  PRIMARY KEY(match_id, steam_id)
);
CREATE TABLE receipts (
  id TEXT PRIMARY KEY, analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id), round_id TEXT REFERENCES rounds(id), metric_key TEXT NOT NULL,
  event_tick INTEGER, ingestion_ordinal INTEGER, participant_steam_ids_json TEXT NOT NULL, raw_payload_json TEXT NOT NULL,
  snapshots_json TEXT NOT NULL, parameters_json TEXT NOT NULL, CHECK((event_tick IS NULL) = (ingestion_ordinal IS NULL))
);
CREATE TABLE round_player_metrics (
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id), round_id TEXT NOT NULL REFERENCES rounds(id), steam_id TEXT NOT NULL REFERENCES players(steam_id),
  metric_key TEXT NOT NULL, numerator INTEGER NOT NULL, denominator INTEGER NOT NULL CHECK(denominator >= 0), value_bp INTEGER, receipt_id TEXT REFERENCES receipts(id),
  PRIMARY KEY(analysis_run_id,round_id,steam_id,metric_key)
);
CREATE TABLE match_player_metrics (
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id), steam_id TEXT NOT NULL REFERENCES players(steam_id), metric_key TEXT NOT NULL,
  numerator INTEGER NOT NULL, denominator INTEGER NOT NULL CHECK(denominator >= 0), value_bp INTEGER, receipt_id TEXT REFERENCES receipts(id),
  PRIMARY KEY(analysis_run_id,steam_id,metric_key)
);
CREATE TABLE player_rating_vectors (
  id TEXT PRIMARY KEY, analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id), steam_id TEXT NOT NULL REFERENCES players(steam_id),
  formula_id TEXT NOT NULL, rating_bp INTEGER NOT NULL, receipt_id TEXT REFERENCES receipts(id), UNIQUE(analysis_run_id,steam_id,formula_id)
);
CREATE TABLE player_rating_components (
  rating_vector_id TEXT NOT NULL REFERENCES player_rating_vectors(id), component_key TEXT NOT NULL, numerator INTEGER NOT NULL,
  denominator INTEGER NOT NULL CHECK(denominator >= 0), value_bp INTEGER NOT NULL, weight_bp INTEGER NOT NULL CHECK(weight_bp >= 0), receipt_id TEXT REFERENCES receipts(id),
  PRIMARY KEY(rating_vector_id,component_key)
);
CREATE TABLE import_jobs (
  id TEXT PRIMARY KEY, demo_sha256 TEXT NOT NULL REFERENCES artifacts(sha256), status TEXT NOT NULL CHECK(status IN ('queued','leased','succeeded','failed','cancelled')),
  progress_bp INTEGER NOT NULL DEFAULT 0 CHECK(progress_bp BETWEEN 0 AND 10000), lease_owner TEXT, lease_expires_at_ms INTEGER, error_code TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE TABLE import_attempts (
  id TEXT PRIMARY KEY, import_job_id TEXT NOT NULL REFERENCES import_jobs(id), attempt_number INTEGER NOT NULL, started_at_ms INTEGER NOT NULL,
  finished_at_ms INTEGER, status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed')), error_code TEXT, UNIQUE(import_job_id,attempt_number)
);
CREATE TRIGGER metric_round_same_run BEFORE INSERT ON round_player_metrics BEGIN
  SELECT CASE WHEN (SELECT analysis_run_id FROM rounds WHERE id=NEW.round_id) <> NEW.analysis_run_id THEN RAISE(ABORT,'round metric run mismatch') END;
  SELECT CASE WHEN NEW.receipt_id IS NOT NULL AND (SELECT analysis_run_id FROM receipts WHERE id=NEW.receipt_id) <> NEW.analysis_run_id THEN RAISE(ABORT,'receipt run mismatch') END;
END;
CREATE TRIGGER candidate_media_verified BEFORE INSERT ON candidate_media BEGIN
  SELECT CASE WHEN (SELECT status FROM recorder_save_attempts WHERE id=NEW.save_attempt_id) <> 'saved' THEN RAISE(ABORT,'unsaved attempt') END;
  SELECT CASE WHEN (SELECT verified_artifact_sha256 FROM recorder_save_attempts WHERE id=NEW.save_attempt_id) <> (SELECT artifact_sha256 FROM clips WHERE id=NEW.clip_id) THEN RAISE(ABORT,'attempt artifact mismatch') END;
END;
CREATE INDEX gsi_snapshots_session_receive ON gsi_snapshots(capture_session_id,received_at_ms);
CREATE INDEX import_jobs_lease ON import_jobs(status,lease_expires_at_ms);
