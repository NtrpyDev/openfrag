ALTER TABLE import_jobs ADD COLUMN diagnostic_json TEXT;
ALTER TABLE import_attempts ADD COLUMN remediation_code TEXT;
ALTER TABLE import_attempts ADD COLUMN diagnostic_json TEXT;

CREATE UNIQUE INDEX import_attempts_one_running
ON import_attempts(import_job_id)
WHERE status = 'running';

CREATE TRIGGER import_attempt_started
AFTER UPDATE OF status, lease_owner ON import_jobs
WHEN NEW.status = 'leased' AND NEW.lease_owner IS NOT NULL
BEGIN
  UPDATE import_attempts
  SET finished_at_ms = NEW.updated_at_ms,
      status = 'failed',
      error_code = 'lease_expired',
      remediation_code = 'automatic_retry'
  WHERE import_job_id = NEW.id AND status = 'running';

  INSERT INTO import_attempts(id, import_job_id, attempt_number, started_at_ms, status)
  VALUES(
    NEW.id || ':' || printf('%04d', COALESCE((SELECT MAX(attempt_number) FROM import_attempts WHERE import_job_id = NEW.id), 0) + 1),
    NEW.id,
    COALESCE((SELECT MAX(attempt_number) FROM import_attempts WHERE import_job_id = NEW.id), 0) + 1,
    NEW.updated_at_ms,
    'running'
  );
END;

CREATE TRIGGER import_attempt_finished
AFTER UPDATE OF status ON import_jobs
WHEN NEW.status IN ('succeeded', 'failed', 'cancelled')
BEGIN
  UPDATE import_attempts
  SET finished_at_ms = NEW.updated_at_ms,
      status = CASE WHEN NEW.status = 'succeeded' THEN 'succeeded' ELSE 'failed' END,
      error_code = NEW.error_code,
      remediation_code = NEW.remediation_code,
      diagnostic_json = NEW.diagnostic_json
  WHERE import_job_id = NEW.id AND status = 'running';
END;

CREATE TRIGGER import_attempt_interrupted
AFTER UPDATE OF status ON import_jobs
WHEN OLD.status = 'leased' AND NEW.status = 'queued'
BEGIN
  UPDATE import_attempts
  SET finished_at_ms = NEW.updated_at_ms,
      status = 'failed',
      error_code = 'lease_expired',
      remediation_code = 'automatic_retry'
  WHERE import_job_id = NEW.id AND status = 'running';
END;
