CREATE TABLE save_attempt_clips (
  save_attempt_id TEXT PRIMARY KEY REFERENCES recorder_save_attempts(id),
  clip_id TEXT NOT NULL UNIQUE REFERENCES clips(id)
);
