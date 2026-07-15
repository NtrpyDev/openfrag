ALTER TABLE import_jobs ADD COLUMN work_phase TEXT;
ALTER TABLE import_jobs ADD COLUMN frames_done INTEGER;
ALTER TABLE import_jobs ADD COLUMN events_emitted INTEGER;

CREATE TRIGGER import_parser_progress_valid
BEFORE UPDATE OF work_phase, frames_done, events_emitted ON import_jobs
BEGIN
  SELECT CASE
    WHEN NEW.work_phase IS NOT NULL
      AND NEW.work_phase NOT IN ('first_pass', 'second_pass', 'finalize')
    THEN RAISE(ABORT, 'invalid parser work phase')
  END;
  SELECT CASE
    WHEN NEW.frames_done IS NOT NULL AND NEW.frames_done < 0
    THEN RAISE(ABORT, 'invalid parser frame progress')
  END;
  SELECT CASE
    WHEN NEW.events_emitted IS NOT NULL AND NEW.events_emitted < 0
    THEN RAISE(ABORT, 'invalid parser event progress')
  END;
END;
