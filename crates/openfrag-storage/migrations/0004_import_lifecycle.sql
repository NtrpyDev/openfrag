ALTER TABLE import_jobs ADD COLUMN bytes_done INTEGER;
ALTER TABLE import_jobs ADD COLUMN bytes_total INTEGER;
ALTER TABLE import_jobs ADD COLUMN heartbeat_at_ms INTEGER;
ALTER TABLE import_jobs ADD COLUMN next_retry_at_ms INTEGER;
ALTER TABLE import_jobs ADD COLUMN remediation_code TEXT;
ALTER TABLE import_jobs ADD COLUMN retry_budget INTEGER NOT NULL DEFAULT 3;
CREATE TRIGGER import_progress_valid BEFORE UPDATE OF progress_bp ON import_jobs BEGIN
  SELECT CASE WHEN NEW.progress_bp < 0 OR NEW.progress_bp > 10000 THEN RAISE(ABORT,'invalid import progress') END;
END;
