# Storage schema and on-disk layout

Issue: [Storage schema: SQLite layout and the clip-to-round link](https://github.com/NtrpyDev/openfrag/issues/13)

Decision: one local SQLite database is the index and provenance ledger. Large,
user-owned artifacts live under the XDG data directory and are addressed by
SHA-256. A live capture session is never a Match; a Match becomes canonical only
when an imported Demo is parsed successfully. This implements the settled
[GSI evidence contract](https://github.com/NtrpyDev/openfrag/blob/4245a7145d3bf102bb70aca329f52f4c166edbb6/docs/gsi-evidence-contract.md), [GSI/Demo merge decision](https://github.com/NtrpyDev/openfrag/blob/8d19e23/docs/gsi-demo-merge-research.md), [highlight rules](https://github.com/NtrpyDev/openfrag/blob/b9053d7011ea010a1cca2140c96648b40607d086/docs/highlight-rules-v1.md), and pinned [Demo Receipt contract](https://github.com/NtrpyDev/openfrag/blob/97face7e47c877a78e87e2d3e2b6a42fd8e3066a/docs/demoparser-evaluation.md).

## Adopted choices

Each choice was tested in five concise rounds: purpose, causal ownership,
incentives, edge cases, and reversibility. The adopted outcome and short
rationale follow.

| Choice | Strongest alternative | Winner and rationale |
|---|---|---|
| One SQLite ledger plus files | Put all media and Demo bytes in SQLite BLOBs | Files keep large artifacts inspectable and cheaply retained; SQLite preserves atomic metadata, constraints, and queryable Receipts. |
| Content hash IDs for artifacts; UUIDv7 for local entities | Paths or integer row IDs as external identity | Renames and retention do not change identity. UUIDv7 is opaque and locally generatable; row IDs remain internal only. |
| CaptureSession before Match | Create a provisional Match from GSI | GSI has no match ID or clock crosswalk. Session ownership prevents a snapshot from becoming analytics truth. |
| Demo-only canonical facts | Merge GSI fields into a post-match event row | Demo has ticks, events, participants, and round result. GSI retains a separate receipt and optional reconciliation edge. |
| Immutable analysis runs | Update Match/round/stats in place on reparse | Parser and formula changes are expected. Append-only runs preserve reproducibility and permit rollback by changing the canonical pointer. |

## Directory layout

All paths are under `${XDG_DATA_HOME:-~/.local/share}/openfrag` with directory
mode `0700`; newly created artifact files use `0600`. Database transactions
commit metadata only after a staged file is fsynced and atomically renamed.

```text
openfrag/
  openfrag.sqlite3
  demos/sha256/ab/<demo_sha256>.dem
  clips/sha256/cd/<clip_sha256>.<ext>
  captures/<capture_session_uuid>/
    provisional/<candidate_uuid>.<ext>
    diagnostics/<snapshot_sha256>.json.redacted
  staging/<uuid>/                         # private, deleted after commit/recovery
  exports/<uuid>/                         # user-requested, not canonical inputs
```

`artifacts.relative_path` is always relative to this root, normalized, and
must not contain `..` or be a symlink. The SHA-256 is computed from exact file
bytes. A same-hash import reuses one artifact row. The original imported Demo
filename is display metadata only, never an address.

## Stable IDs and lifecycle

- `capture_sessions.id`, `live_candidates.id`, `clips.id`, `matches.id`,
  `analysis_runs.id`, and `receipts.id` are UUIDv7 text values.
- `artifacts.sha256` is a lowercase 64-character SHA-256 primary key.
- `players.steam_id` is decimal SteamID64 text and is the sole cross-pipeline
  player key. Names are mutable display history.
- A Match is identified canonically by its imported Demo artifact hash. It may
  have multiple analysis runs, but one `matches.canonical_run_id` at a time.
- Entity IDs, PipeWire IDs, GSI payload hashes, pathnames, and SQLite rowids are
  never cross-pipeline identity keys.

Lifecycle:

```text
CaptureSession -> LiveCandidate -> provisional Clip
                       |                         |
                       +-- optional reconcile --+--> imported Demo -> Match
                                                        -> AnalysisRun -> Round/Receipt
```

Reconciliation is optional and non-destructive. It requires the configured
local SteamID, known map equality when both sides provide one, unique Demo round
when GSI supplied one, and compatible transition category. Failure leaves
`unconfirmed` metadata and media outside canonical analytics.

## SQLite schema

Enable `PRAGMA foreign_keys = ON`, WAL journal mode, and `busy_timeout`. Store
times as UTC integer milliseconds. Demo ticks are integer parser ticks and
never converted from GSI time.

```sql
CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at_ms INTEGER NOT NULL,
  checksum TEXT NOT NULL UNIQUE
);

CREATE TABLE artifacts (
  sha256 TEXT PRIMARY KEY CHECK(length(sha256) = 64),
  kind TEXT NOT NULL CHECK(kind IN ('demo','clip','gsi_receipt','export')),
  relative_path TEXT NOT NULL UNIQUE,
  byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
  media_type TEXT,
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK(deleted_at_ms IS NULL OR deleted_at_ms >= created_at_ms)
);

CREATE TABLE capture_sessions (
  id TEXT PRIMARY KEY,
  started_at_ms INTEGER NOT NULL,
  ended_at_ms INTEGER,
  local_steam_id TEXT NOT NULL,
  game_build TEXT,
  recorder_build TEXT,
  status TEXT NOT NULL CHECK(status IN ('active','ended','degraded','deleted')),
  CHECK(ended_at_ms IS NULL OR ended_at_ms >= started_at_ms)
);

CREATE TABLE gsi_snapshots (
  sha256 TEXT PRIMARY KEY REFERENCES artifacts(sha256),
  capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  arrival_ordinal INTEGER NOT NULL,
  received_at_ms INTEGER NOT NULL,
  provider_timestamp TEXT,
  field_presence_json TEXT NOT NULL,
  byte_length INTEGER NOT NULL,
  listener_version TEXT NOT NULL,
  http_status INTEGER NOT NULL,
  UNIQUE(capture_session_id, arrival_ordinal)
);

CREATE TABLE live_candidates (
  id TEXT PRIMARY KEY,
  capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  source_snapshot_sha256 TEXT NOT NULL REFERENCES gsi_snapshots(sha256),
  created_at_ms INTEGER NOT NULL,
  observed_kind TEXT NOT NULL,
  observed_map TEXT,
  observed_round INTEGER,
  status TEXT NOT NULL CHECK(status IN ('provisional','confirmed','unconfirmed','expired')),
  CHECK(observed_round IS NULL OR observed_round >= 0)
);

CREATE TABLE clips (
  id TEXT PRIMARY KEY,
  artifact_sha256 TEXT NOT NULL UNIQUE REFERENCES artifacts(sha256),
  capture_session_id TEXT NOT NULL REFERENCES capture_sessions(id),
  live_candidate_id TEXT REFERENCES live_candidates(id),
  recorded_at_ms INTEGER NOT NULL,
  start_receive_ms INTEGER,
  end_receive_ms INTEGER,
  provenance TEXT NOT NULL CHECK(provenance IN ('auto_round','manual','consolidated')),
  pre_roll_truncated INTEGER NOT NULL DEFAULT 0 CHECK(pre_roll_truncated IN (0,1)),
  retention_class TEXT NOT NULL CHECK(retention_class IN ('provisional','confirmed','manual')),
  deleted_at_ms INTEGER
);

CREATE TABLE matches (
  id TEXT PRIMARY KEY,
  demo_sha256 TEXT NOT NULL UNIQUE REFERENCES artifacts(sha256),
  imported_at_ms INTEGER NOT NULL,
  local_steam_id TEXT NOT NULL,
  map_name TEXT,
  game_build TEXT,
  canonical_run_id TEXT UNIQUE,
  status TEXT NOT NULL CHECK(status IN ('imported','parsed','quarantined','deleted'))
);

CREATE TABLE analysis_runs (
  id TEXT PRIMARY KEY,
  match_id TEXT NOT NULL REFERENCES matches(id),
  parser_commit TEXT NOT NULL,
  generated_proto_build TEXT NOT NULL,
  parser_build TEXT NOT NULL,
  formula_id TEXT NOT NULL,
  metric_definition_version TEXT NOT NULL,
  requested_schema_hash TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed','superseded')),
  error_code TEXT,
  UNIQUE(match_id, parser_build, generated_proto_build, formula_id,
         metric_definition_version, requested_schema_hash)
);

CREATE TABLE rounds (
  id TEXT PRIMARY KEY,
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id),
  round_number INTEGER NOT NULL CHECK(round_number >= 0),
  start_tick INTEGER,
  end_tick INTEGER NOT NULL,
  winner TEXT CHECK(winner IN ('T','CT')),
  reason TEXT,
  eligibility TEXT NOT NULL CHECK(eligibility IN ('eligible','excluded')),
  exclusion_reason TEXT,
  CHECK((eligibility = 'eligible' AND winner IS NOT NULL)
     OR (eligibility = 'excluded' AND exclusion_reason IS NOT NULL)),
  UNIQUE(analysis_run_id, round_number)
);

CREATE TABLE receipts (
  id TEXT PRIMARY KEY,
  analysis_run_id TEXT NOT NULL REFERENCES analysis_runs(id),
  round_id TEXT REFERENCES rounds(id),
  metric_key TEXT NOT NULL,
  event_tick INTEGER,
  ingestion_ordinal INTEGER,
  participant_steam_ids_json TEXT NOT NULL,
  raw_payload_json TEXT NOT NULL,
  snapshots_json TEXT NOT NULL,
  parameters_json TEXT NOT NULL,
  CHECK((event_tick IS NULL) = (ingestion_ordinal IS NULL))
);

CREATE TABLE candidate_reconciliations (
  live_candidate_id TEXT PRIMARY KEY REFERENCES live_candidates(id),
  match_id TEXT REFERENCES matches(id),
  round_id TEXT REFERENCES rounds(id),
  receipt_id TEXT REFERENCES receipts(id),
  status TEXT NOT NULL CHECK(status IN ('confirmed','unconfirmed','rejected')),
  decided_at_ms INTEGER NOT NULL,
  reason_code TEXT NOT NULL,
  CHECK((status = 'confirmed' AND match_id IS NOT NULL AND round_id IS NOT NULL AND receipt_id IS NOT NULL)
     OR status <> 'confirmed')
);

CREATE INDEX gsi_snapshots_session_receive ON gsi_snapshots(capture_session_id, received_at_ms);
CREATE INDEX live_candidates_session_status ON live_candidates(capture_session_id, status, created_at_ms);
CREATE INDEX clips_retention ON clips(retention_class, recorded_at_ms) WHERE deleted_at_ms IS NULL;
CREATE INDEX analysis_runs_match_status ON analysis_runs(match_id, status, completed_at_ms);
CREATE INDEX rounds_run_number ON rounds(analysis_run_id, round_number);
CREATE INDEX receipts_run_metric ON receipts(analysis_run_id, metric_key, round_id);
```

After a successful run, one transaction sets `analysis_runs.status='succeeded'`,
sets the prior canonical run to `superseded`, and updates
`matches.canonical_run_id`. A database trigger must reject a canonical run that
is not `succeeded` or belongs to a different Match. Application validation must
also require a confirmed reconciliation's run and round to belong to its Match.

## Retention, deletion, and recovery

The [highlight decision](https://github.com/NtrpyDev/openfrag/issues/9#issuecomment-4974137158)
sets provisional media expiry to 30 days or 2 GiB per CaptureSession, deleting
oldest unconfirmed media first. Confirmed clips and manual Flags are retained
until the user deletes them. Deleting media marks its artifact and clip deleted,
removes the file after a transaction commits, and preserves a tombstone,
provenance, and Receipts. Deleting a Demo cascades no canonical facts: mark the
Match deleted and hide its runs, rounds, and Receipts; retain rows for audit
until the user explicitly chooses irreversible metadata purge.

Startup recovery removes abandoned `staging/` entries only after checking no
artifact row references them. Missing expected files mark the artifact missing
and affected Clip or Match degraded or quarantined, never silently empty.

## Migration and version rules

- Each forward-only SQL migration has a sequential version, checksum, and
  transaction. Never edit an applied migration.
- Additive schema changes are compatible. A destructive change creates new
  columns/tables, backfills in a resumable migration, validates counts and
  foreign keys, then removes old data only in a later user-visible release.
- Parser, generated-protobuf, requested field schema, metric definition, or
  formula changes create a new `analysis_runs` row. They never overwrite a
  Receipt or mutate a prior canonical run.
- GSI listener changes version live receipts only. They cannot invalidate Demo
  canonical facts, but may alter future candidate behavior.

## Implementation checks

1. Foreign-key, unique, check, and canonical-run trigger tests cover every
   lifecycle transition.
2. Crash tests cover staged artifact rename, database commit, delete, and
   startup recovery.
3. Reconciliation fixtures cover matching SteamID/map/round, ambiguity,
   mismatch, equal GSI timestamps, and unconfirmed preservation.
4. Reparse fixtures prove old Receipt bytes remain unchanged while the Match
   canonical pointer moves to a successful new run.
5. Retention tests prove the 30-day and 2 GiB provisional policy never deletes
   manual or confirmed media first.

## Unresolved dependencies

- The import pipeline must supply the local SteamID and Demo hash before Match
  creation.
- The parser facade must define exact raw event and snapshot serialization for
  `receipts` and the requested-schema hash.
- Recorder integration must report verified artifact paths, media duration, and
  clip save completion before a `clips` row is committed.
- User-facing irreversible metadata purge, export, encryption-at-rest, and
  multi-PC synchronization are deliberately outside this issue.
