ALTER TABLE clips ADD COLUMN reviewed_at_ms INTEGER
  CHECK(reviewed_at_ms IS NULL OR reviewed_at_ms >= 0);
