# Post-match import pipeline decision

v1 accepts local `.dem` files only. GSI may create provisional candidate records, but a local Demo import is the only trigger for canonical Match, Rating, and Receipt production. The pipeline is durable, idempotent, and local. Every transition writes an append-only attempt record before expensive work; a crash resumes from the last committed state.

## State machine

| State | Meaning and UI copy | Next states |
|---|---|---|
| `awaiting_import` | **“Choose a demo file.”** No file has been accepted. | `validating` |
| `validating` | **“Checking demo file…”** Verify regular file, readable bytes, size, storage headroom, and recognized demo header. Stream in bounded 8 MiB chunks. | `hashing`, `error_corrupt`, `error_unsupported`, `error_size`, `error_io` |
| `hashing` | **“Fingerprinting demo…”** Compute SHA-256 over immutable source bytes. | `deduplicating`, `error_io` |
| `deduplicating` | **“Checking for an existing match…”** Look up the source hash and requested calculation identity. | same calculation succeeded: `ready`; running or interrupted: resume checkpoint; parser/proto/schema/evidence-epoch change: `parsing`; metric-definition or formula-only new run with compatible parsed evidence: `rating`; prior failed run: show failure with explicit retry; absent artifact: `copying` |
| `copying` | **“Copying demo into openfrag storage…”** Copy to a temporary file, fsync, atomically rename, and verify size and hash. | `parsing`, `error_io` |
| `parsing` | **“Reading rounds and events…”** Run the pinned parser with bounded memory and record parser errors. | `rating`, `error_parse` |
| `rating` | **“Calculating Rating and Receipts…”** Run the versioned formula only over validated evidence. | `linking_clips`, `error_analysis` |
| `linking_clips` | **“Linking available clips…”** Reconcile eligible CaptureSessions to the established Demo Match and canonical round identity; never compare clocks or Demo ticks. | `ready`, `error_analysis` only for corrupt metadata |
| `ready` | **“Ready.”** Match, stats, Receipts, and any linked Clips are browsable. | `parsing` only when parser or generated-proto identity changes; `rating` when formula identity alone changes |
| `error_*` | **“Import needs attention.”** Show stable code, human explanation, and retry or remove action. | retry to the documented predecessor, `awaiting_import`, or terminal removal |

Error codes are `error_io`, `error_corrupt`, `error_unsupported`, `error_size`, `error_parse`, and `error_analysis`. Never represent an error as an empty Match.

The v1 source limit is exactly 2 GiB (2,147,483,648 bytes). Validation rejects larger files as `error_size` before parsing. It also requires free storage for the source copy plus a 10 percent headroom reserve; when that preflight fails, return `error_io` with an actionable storage message. Hashing and copying use bounded 8 MiB streaming buffers and never load the complete Demo into memory.

## Progress and recovery

Persist phase progress with every checkpoint. For `validating`, `hashing`, and `copying`, expose `processed_bytes`, `total_bytes`, and `fraction = processed_bytes / total_bytes` using deterministic integer arithmetic, capped at 1.0. For `parsing`, `rating`, and `linking_clips`, expose `indeterminate: true` plus phase-specific work counters and heartbeat time unless a stable total is known. Never fabricate a percentage. A 60-second heartbeat gap is a warning only: show **“Still working. Last update <time>.”** and do not start a concurrent retry while the lease owner is alive. Retry or resume only after the prior worker process ends or its lease expires and a restart declares it abandoned. Recovery keeps the last committed counters and attempt ID.

## Triggers, keys, and reconciliation

An import trigger is a user-selected local path or an explicit Inbox retry. GSI does not bypass `awaiting_import`. Before a Demo exists, create only a `CaptureSession` with a generated session ID, observed map and round, candidate Receipts, and provisional Clips. After a Demo is copied and parsed, the Demo Match is established independently. Reconcile a CaptureSession only when the configured local SteamID is a Demo participant, the map agrees when one was observed, and exactly one canonical Demo round matches the observed round identity plus the categorical round transition. Candidate monotonic time is never compared with Demo ticks or wall time. If no unique match exists, retain **“Awaiting Demo association”** or require explicit user selection.

Idempotency keys are:

- import job: `source_sha256`;
- stored Demo artifact: `source_sha256`;
- calculation run: `source_sha256 + parser_commit + parser_build + generated_proto_build + requested_schema_hash + metric_definition_version + formula_version + evidence_semantics_epoch`;
- Match: `source_sha256` and immutable stored-artifact identity;
- live candidate: `live_candidate_id`;
- recorder save attempt: globally unique `recorder_request_id`, linked separately to one or more live candidates or a Manual Flag;
- verified raw Clip artifact: `raw_media_sha256` only after a successful save and verification;
- reconciliation analysis run: `live_candidate_id + analysis_run_id`;

Every key is unique in storage. Replays of a request return the existing state and attempt ID. A failed recorder save attempt produces no Clip and no raw-media hash. Reconciliation is append-only across analysis runs: after the Demo participant, map, unique round, and categorical transition checks pass, one analysis run adds one canonical round association and Receipt. Recovery is observable through `attempt_id`, lease owner, lease expiry, state-entered time, retry count, next retry time, source hash, parser identity, and last error code. A startup sweep marks abandoned attempts resumable only after lease ownership is proven ended or expired, verifies checkpoints, and removes only uncommitted temporary files.

## Decision fights and winners

Five-round summary: local import beats automatic acquisition because it is supported and auditable; snapshots beat inferred GSI events because the stream is buffered; content hashes beat filenames because retries and duplicates are safe; append-only calculation runs beat overwrite because trends remain reproducible; explicit user deletion beats automatic retention because Receipts and Clips are evidence. These choices are product decisions informed by the acquisition, GSI, and highlight contracts.

### Retries

The optimistic policy is infinite automatic retry. The safer policy is bounded automatic retry for transient I/O only, then visible manual retry. Winner: the initial attempt plus three automatic transient-I/O retries after 2 seconds, 10 seconds, and 60 seconds. Persist the schedule and retry count; reset the automatic budget only after an explicit user retry. No automatic retry applies to corrupt, unsupported, size, parser, or analysis errors.

### Idempotency and crash recovery

The simplest policy is to rerun every stage. The safer policy is content-addressed artifacts and committed checkpoints. Winner: key the source by SHA-256, use atomic temporary-file rename, and make each stage a transaction. On restart, verify checkpoint inputs and continue; delete abandoned temp files. A completed hash plus parser/formula identity is never parsed twice unless explicitly requested.

### Duplicate handling

The simple policy is to create a second Match for every import. The safer policy is exact artifact deduplication by source SHA-256. Winner: identical bytes open the existing Demo artifact and report **“Already imported.”** Parser, generated-proto, requested-schema, metric-definition, formula, and evidence-epoch identities select calculation runs only; they never make a second Demo artifact.

### Corrupt and unsupported input

The permissive policy is to salvage whatever bytes parse. The safer policy rejects before storage or marks a parse attempt failed. Winner: reject unreadable, truncated, non-demo, and unsupported-version files with a specific error and remediation. Preserve the original only when the user explicitly asks to retain a failed import for diagnosis.

### Parser-version reruns

Overwriting old stats is simple but destroys trend reproducibility. Winner: retain immutable parser commit, parser build, generated-proto, requested-schema, metric-definition, formula, and evidence-semantics identities in every calculation run. A parser, generated-proto, requested-schema, or evidence-epoch change resumes at `parsing`; a metric-definition or formula-only change resumes at `rating`. The same full identity returns the existing run. A rerun writes new components, Receipts, and Rating beside the old run; the UI labels one run canonical and allows comparison. Never mix versions in one trend line.

### Inbox visibility

The minimalist UI shows only successful Matches. Winner: Inbox lists every active import and terminal error with state, progress, filename, timestamp, retry action, and stable error code. `ready` items remain discoverable through Matches; errors remain until dismissed or removed so failures cannot disappear.

### Deletion and retention

Automatic deletion saves disk but can destroy evidence. Winner: never delete a source demo or Receipt automatically while its Match is retained. User deletion is explicit, confirms the affected Match, stats, Receipts, and Clips, and leaves an audit tombstone containing hash and deletion time. Temporary files and failed copies may be garbage-collected after a grace period; raw failed inputs are not retained by default.

## Invariants

- A Match becomes visible as canonical only after validated storage, successful parsing, Rating, and Receipts are committed.
- Every state transition is append-only and includes attempt ID, timestamp, input hash when known, parser/build identity, and error details when applicable.
- A source hash identifies bytes, not a user-supplied filename; artifact deduplication never includes calculation metadata.
- A failed attempt cannot create a zero-stat, empty, or partially canonical Match.
- Atomic copy and verified hash prevent a crash from exposing a partial demo.
- Clip linking is additive and cannot change parsed stats or Rating.
- A rerun never mutates a prior calculation or Receipt.
- `ready` returns the existing calculation when every calculation identity component matches; parser, proto, schema, or evidence-epoch changes enter `parsing`, and metric-definition or formula-only changes enter `rating`.
- One active lease owns a job; a stale heartbeat alone cannot create a second worker. Lease owner and expiry are persisted, and abandonment requires process end, lease expiry, and restart handling.
- Candidate identity, recorder save-attempt identity, verified raw Clip identity, and reconciliation identity are separate. Failed saves produce no Clip identity.
- User deletion cannot remove a demo still referenced by another Match or calculation run without an explicit dependency confirmation.

## Verification cases

1. Import a valid demo, kill the process during copy and parsing, restart, and verify resume without duplicate Match rows.
2. Import the same bytes under two filenames and verify **“Already imported.”**
3. Import a truncated file, non-demo file, unreadable file, over-2-GiB file, and unsupported-version demo; verify stable error state and no canonical Match.
4. Force a transient storage error and verify the initial attempt plus retries at 2, 10, and 60 seconds, then a manual retry that resets the budget.
5. Verify byte-phase counters reach deterministic 1.0, parser/rating/linking remain honestly indeterminate with heartbeats, and a 60-second heartbeat gap shows warning UI without a concurrent worker.
6. Exercise deduplication branches: succeeded same identity returns `ready`, running or interrupted resumes under its lease, absent artifact copies then parses, parser/proto/schema/evidence-epoch changes enter `parsing`, metric-definition/formula-only changes enter `rating`, and failed run shows its prior failure with explicit retry.
7. Rerun a Match with each calculation identity component changed and unchanged; verify exact phase routing and immutable runs.
8. Reconcile a CaptureSession with a Demo participant/local SteamID, agreeing map, unique round identity, and categorical transition; reject nearest-clock-only, ambiguous, map-mismatched, and nonparticipant associations.
9. Link a matching Clip, a missing Clip, and malformed metadata using the established Match and canonical round only; never use Demo ticks or wall-clock proximity.
10. Fail a recorder save and verify no Clip row or raw-media hash exists, then succeed a later request with a distinct save-attempt ID.
11. Delete a Match with dependent calculations or Clips and verify confirmation, dependency warning, and audit tombstone.
12. Confirm Inbox retains dismissed errors until explicit removal and that no error is rendered as an empty Match.
