CREATE TABLE unavailable_rating_results (
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id),
  steam_id TEXT NOT NULL REFERENCES players(steam_id),
  formula_id TEXT NOT NULL,
  reason_code TEXT NOT NULL,
  receipt_id TEXT NOT NULL REFERENCES receipts(id),
  PRIMARY KEY(analysis_run_id, steam_id, formula_id)
);

CREATE TRIGGER unavailable_rating_receipt_same_run BEFORE INSERT ON unavailable_rating_results BEGIN
  SELECT CASE WHEN (SELECT analysis_run_id FROM receipts WHERE id=NEW.receipt_id) <> NEW.analysis_run_id
    THEN RAISE(ABORT,'unavailable rating receipt run mismatch') END;
END;

DROP TRIGGER canonical_run_valid;
CREATE TRIGGER canonical_run_valid BEFORE UPDATE OF canonical_run_id ON matches WHEN NEW.canonical_run_id IS NOT NULL BEGIN
  SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM analysis_runs WHERE id=NEW.canonical_run_id AND match_id=NEW.id AND status='succeeded')
    THEN RAISE(ABORT,'canonical run is not a succeeded match run') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM player_rating_vectors v JOIN receipts e ON e.id=v.receipt_id
    WHERE v.analysis_run_id=NEW.canonical_run_id AND v.steam_id=NEW.local_steam_id AND e.analysis_run_id=NEW.canonical_run_id
    UNION ALL
    SELECT 1 FROM unavailable_rating_results u JOIN receipts e ON e.id=u.receipt_id
    WHERE u.analysis_run_id=NEW.canonical_run_id AND u.steam_id=NEW.local_steam_id AND e.analysis_run_id=NEW.canonical_run_id
  ) THEN RAISE(ABORT,'canonical run lacks local rating receipt') END;
END;
