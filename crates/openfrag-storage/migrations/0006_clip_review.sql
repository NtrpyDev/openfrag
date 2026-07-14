ALTER TABLE clips ADD COLUMN note TEXT;
ALTER TABLE clips ADD COLUMN review_decision TEXT NOT NULL DEFAULT 'pending'
  CHECK(review_decision IN ('pending','keep','reject'));

CREATE TABLE clip_tags (
  clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position >= 0),
  tag TEXT NOT NULL CHECK(length(trim(tag)) > 0),
  PRIMARY KEY(clip_id, position),
  UNIQUE(clip_id, tag)
);
