ALTER TABLE artifacts ADD COLUMN media_duration_ms INTEGER
  CHECK(media_duration_ms IS NULL OR media_duration_ms > 0);

ALTER TABLE clips ADD COLUMN review_revision INTEGER
  CHECK(review_revision IS NULL OR review_revision >= 0);
ALTER TABLE clips ADD COLUMN origin_kind TEXT
  CHECK(origin_kind IS NULL OR origin_kind IN ('auto','manual'));
ALTER TABLE clips ADD COLUMN manual_flag_time_ms INTEGER
  CHECK(manual_flag_time_ms IS NULL OR manual_flag_time_ms >= 0);

CREATE TABLE clip_origin_receipts (
  clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('trigger','evidence','manual_flag','overlapping_auto')),
  position INTEGER NOT NULL CHECK(position >= 0),
  receipt_id TEXT NOT NULL CHECK(length(trim(receipt_id)) > 0),
  PRIMARY KEY(clip_id, kind, position),
  UNIQUE(clip_id, kind, receipt_id)
);
