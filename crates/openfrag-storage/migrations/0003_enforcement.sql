ALTER TABLE candidate_reconciliations ADD COLUMN match_id TEXT REFERENCES matches(id);
ALTER TABLE candidate_reconciliations ADD COLUMN receipt_id TEXT REFERENCES receipts(id);
CREATE TRIGGER match_metric_participant BEFORE INSERT ON match_player_metrics BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM analysis_runs r JOIN match_players p ON p.match_id=r.match_id
    WHERE r.id=NEW.analysis_run_id AND p.steam_id=NEW.steam_id
  ) THEN RAISE(ABORT,'match metric player is not a participant') END;
  SELECT CASE WHEN NEW.receipt_id IS NOT NULL AND (SELECT analysis_run_id FROM receipts WHERE id=NEW.receipt_id) <> NEW.analysis_run_id
    THEN RAISE(ABORT,'match metric receipt run mismatch') END;
END;
CREATE TRIGGER rating_receipt_same_run BEFORE INSERT ON player_rating_vectors BEGIN
  SELECT CASE WHEN NEW.receipt_id IS NOT NULL AND (SELECT analysis_run_id FROM receipts WHERE id=NEW.receipt_id) <> NEW.analysis_run_id
    THEN RAISE(ABORT,'rating receipt run mismatch') END;
END;
CREATE TRIGGER rating_component_receipt_same_run BEFORE INSERT ON player_rating_components BEGIN
  SELECT CASE WHEN NEW.receipt_id IS NOT NULL AND (SELECT analysis_run_id FROM receipts WHERE id=NEW.receipt_id) <> (SELECT analysis_run_id FROM player_rating_vectors WHERE id=NEW.rating_vector_id)
    THEN RAISE(ABORT,'rating component receipt run mismatch') END;
END;
CREATE TRIGGER canonical_run_valid BEFORE UPDATE OF canonical_run_id ON matches WHEN NEW.canonical_run_id IS NOT NULL BEGIN
  SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM analysis_runs WHERE id=NEW.canonical_run_id AND match_id=NEW.id AND status='succeeded')
    THEN RAISE(ABORT,'canonical run is not a succeeded match run') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM player_rating_vectors v JOIN receipts e ON e.id=v.receipt_id
    WHERE v.analysis_run_id=NEW.canonical_run_id AND v.steam_id=NEW.local_steam_id AND e.analysis_run_id=NEW.canonical_run_id
  ) THEN RAISE(ABORT,'canonical run lacks local rating receipt') END;
END;
CREATE TRIGGER reconciliation_valid BEFORE INSERT ON candidate_reconciliations BEGIN
  SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM live_candidates WHERE id=NEW.candidate_id) THEN RAISE(ABORT,'candidate missing') END;
  SELECT CASE WHEN NEW.status='confirmed' AND NOT EXISTS (
    SELECT 1 FROM live_candidates c JOIN capture_sessions s ON s.id=c.capture_session_id JOIN matches m ON m.id=NEW.match_id
    WHERE c.id=NEW.candidate_id AND s.local_steam_id=m.local_steam_id AND (c.observed_map IS NULL OR m.map_name IS NULL OR c.observed_map=m.map_name)
  ) THEN RAISE(ABORT,'candidate identity or map mismatch') END;
  SELECT CASE WHEN NEW.status='confirmed' AND (SELECT count(*) FROM rounds r JOIN live_candidates c ON c.id=NEW.candidate_id WHERE r.analysis_run_id=NEW.analysis_run_id AND r.round_number=c.observed_round) <> 1
    THEN RAISE(ABORT,'candidate round is not unique') END;
  SELECT CASE WHEN NEW.status='confirmed' AND NOT EXISTS (
    SELECT 1 FROM receipts e JOIN rounds r ON r.id=NEW.round_id JOIN live_candidates c ON c.id=NEW.candidate_id
    WHERE e.id=NEW.receipt_id AND e.analysis_run_id=NEW.analysis_run_id AND r.analysis_run_id=NEW.analysis_run_id
      AND e.metric_key=c.observed_kind
  ) THEN RAISE(ABORT,'candidate category receipt mismatch') END;
END;
