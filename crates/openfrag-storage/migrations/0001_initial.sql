CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at_ms INTEGER NOT NULL,
  checksum TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS artifacts (
  sha256 TEXT PRIMARY KEY CHECK(length(sha256) = 64),
  relative_path TEXT UNIQUE,
  byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
  media_type TEXT,
  availability TEXT NOT NULL CHECK(availability IN ('expected','present','missing','deleted')),
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS capture_sessions (
  id TEXT PRIMARY KEY,
  local_steam_id TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  ended_at_ms INTEGER,
  status TEXT NOT NULL CHECK(status IN ('active','ended','degraded','deleted'))
);

CREATE TABLE IF NOT EXISTS recorder_save_attempts (
  id TEXT PRIMARY KEY,
  recorder_request_id TEXT NOT NULL UNIQUE,
  capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  requested_monotonic_ns INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('requested','acknowledged','saved','failed')),
  acknowledgement_count INTEGER NOT NULL DEFAULT 0,
  verified_artifact_sha256 TEXT REFERENCES artifacts(sha256),
  actual_start_monotonic_ns INTEGER,
  actual_end_monotonic_ns INTEGER,
  error_code TEXT,
  CHECK((status = 'saved' AND verified_artifact_sha256 IS NOT NULL)
     OR (status <> 'saved' AND verified_artifact_sha256 IS NULL))
);

CREATE TABLE IF NOT EXISTS clips (
  id TEXT PRIMARY KEY,
  artifact_sha256 TEXT NOT NULL REFERENCES artifacts(sha256),
  capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  disposition TEXT NOT NULL CHECK(disposition IN ('saved','in_review','kept','deleted')),
  title TEXT,
  favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1)),
  provenance TEXT NOT NULL CHECK(provenance IN ('raw_auto','raw_manual','consolidated','trim_derivative')),
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS matches (
  id TEXT PRIMARY KEY,
  demo_sha256 TEXT NOT NULL UNIQUE REFERENCES artifacts(sha256),
  local_steam_id TEXT NOT NULL,
  map_name TEXT,
  imported_at_ms INTEGER NOT NULL,
  canonical_run_id TEXT,
  status TEXT NOT NULL CHECK(status IN ('imported','parsed','quarantined','deleted'))
);

CREATE TABLE IF NOT EXISTS analysis_runs (
  id TEXT PRIMARY KEY,
  match_id TEXT NOT NULL REFERENCES matches(id),
  parser_commit TEXT NOT NULL,
  parser_build TEXT NOT NULL,
  generated_proto_build TEXT NOT NULL,
  requested_schema_hash TEXT NOT NULL,
  metric_definition_version TEXT NOT NULL,
  formula_id TEXT NOT NULL,
  evidence_semantics_epoch TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed')),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(match_id, parser_commit, parser_build, generated_proto_build,
    requested_schema_hash, metric_definition_version, formula_id, evidence_semantics_epoch)
);

CREATE TABLE IF NOT EXISTS rounds (
  id TEXT PRIMARY KEY,
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id),
  round_number INTEGER NOT NULL,
  end_tick INTEGER NOT NULL,
  winner TEXT CHECK(winner IN ('T','CT')),
  UNIQUE(analysis_run_id, round_number)
);

CREATE TABLE IF NOT EXISTS candidate_reconciliations (
  id TEXT PRIMARY KEY,
  candidate_id TEXT NOT NULL,
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id),
  round_id TEXT REFERENCES rounds(id),
  status TEXT NOT NULL CHECK(status IN ('confirmed','unconfirmed','rejected')),
  reason_code TEXT NOT NULL,
  decided_at_ms INTEGER NOT NULL,
  UNIQUE(candidate_id, analysis_run_id),
  CHECK((status = 'confirmed' AND round_id IS NOT NULL) OR status <> 'confirmed')
);

CREATE INDEX IF NOT EXISTS clips_artifact ON clips(artifact_sha256);
CREATE INDEX IF NOT EXISTS save_attempts_session ON recorder_save_attempts(capture_session_id, status);
