# LaihoE/demoparser evaluation

Pinned source is upstream commit [`ba39cc44cd5abfd7f34df2b3c0a7dd3630048311`](https://github.com/LaihoE/demoparser/tree/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311), inspected 2026-07-14. It is a Rust parser with Python, Node, and WASM bindings, not a separately published stable Rust crate.

## Rust API, build, and parse model

The internal `parser` crate is version 0.1.1. Its public modules expose `parse_demo::Parser`, `ParserInputs`, `ParsingMode`, and `DemoOutput`; callers provide wanted player and game props, events, tick filters, and flags, then call `Parser::parse_demo(&[u8]) -> Result<DemoOutput, DemoParserError>` ([Cargo.toml](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/Cargo.toml), [parse_demo.rs](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/src/parse_demo.rs)). `DemoOutput` includes columnar ticks, game events, projectiles, roster metadata, and header data.

Parsing is two-pass: packet indexing and descriptors/events first, then packet/entity data. Rayon can parallelize full-packet ranges ([parser README](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/README.md)). Multi-threading is not universally safe for stateful/query-dependent props, so the ingest facade must classify requests, force single-thread mode where needed, and avoid requesting every prop by default. Narrow queries and selected tick ranges reduce memory and dataframe materialization; full-demo, all-player, all-prop queries should be treated as an explicit high-memory plan.

Builds are not hermetic: the protobuf build script clones the mutable
GameTracking-CS2 repository, and the parser build script runs the generator
during the build ([protobuf build script](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/csgoproto/build.rs),
[parser build script](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/build.rs)).
That means a repeat build can change field mappings without changing the
parser source revision. Vendor a pinned fork, pinned proto inputs, generated
Rust, and the exact toolchain; disable network builds in CI and release builds.

## Speed evidence

Upstream reports 6.14 seconds for 4.6 GB across 50 demos on a Ryzen 5900X/Samsung 980 Pro and 14.00 seconds on an i5-1335G7/Toshiba XG6 ([README](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311)). Local verification of the representative 60.6 MB fixture measured single-thread median 0.424 s and multi-thread median 0.105 s, with 333 tests passing. Those timings are machine-specific and are not an SLA.

## Update resilience and maintenance

The pinned project is active, but schema and generated-proto changes remain
maintenance-sensitive ([commit history](https://github.com/LaihoE/demoparser/commits/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311)).
Recent reports show parser breakage and compatibility work:
[Recent CS2 demos use CMsgServerUserCmd.delta_data, causing usercmd data to be lost](https://github.com/LaihoE/demoparser/issues/340),
[EntityNotFound on a PRACC-server demo](https://github.com/LaihoE/demoparser/issues/339),
[UnknownDemoCmd(34) on Faceit demos after a CS2 patch](https://github.com/LaihoE/demoparser/issues/330),
[IllegalPathOp on a POV demo from a Valve server](https://github.com/LaihoE/demoparser/issues/329),
[EntityNotFound for patch 14152 demos](https://github.com/LaihoE/demoparser/issues/321),
and [Fix EntityNotFound on demos with more than 256 classes](https://github.com/LaihoE/demoparser/pull/326).
Safeguards: pin parser and generated inputs, maintain a fixture corpus across
maps/builds, quarantine parse errors and suspicious empty outputs, and never
turn a parser failure into an empty `Match`.

## v1 stat feasibility

| Stat | Exposed evidence | Required openfrag logic | Verdict |
|---|---|---|---|
| Opening duels | `player_death`, `weapon_fire`, tick and round fields. | First-kill ordering and team classification. | **Feasible, derived.** |
| Clutches | Deaths, `round_end`, round number, alive/team tick props, plus the upstream 1vX example ([example](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/examples/1vX/main.py)). | Define alive transition and event-order policy, then identify winner and 1vX state. | **Feasible, but transition and event-order policy-sensitive.** |
| Utility | `player_hurt` damage/weapon fields, grenade trajectories, and utility example ([example](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/examples/util_dmg/main.py)). | Attribute HE/molotov/flash effects to throwers. | **Damage and flash feasible; smoke and area-denial metrics not established.** |
| Death context | Death event, attacker/victim state, positions, angles, health, armor, weapon, and round state. | Join snapshots and nearby events; coaching/spectator labels need explicit policy. | **Feasible, with ambiguous coaching labels.** |
| Trades | Attacker, victim, tick, and round context. | Define trade window and teammate/opponent matching. | **Feasible, policy-dependent.** |
| Crosshair placement | Per-tick pitch/yaw, positions, and aim punch ([aim example](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/examples/analyse_aim/main.py)). | Need map geometry, hitboxes, and line-of-sight engine. | **Approximate only; no direct crosshair-on-target field.** |
| Time-to-damage | `weapon_fire` and `player_hurt` ticks. | Pair shots to hurts and calculate tick deltas. | **Tick-level shot-to-hurt feasible; exposure and subtick timing are not exposed.** |
| Spray accuracy | Custom `fire_bullets` and `player_bullet_hit` events can support shot/hit sequences, with angles and recoil-related props. | Pair events, model recoil/spread, and validate weapon semantics. | **Supported in principle, but exact pairing and recoil quality are high-risk.** |

## Receipt contract

Every metric result must retain: demo hash; parser commit and generated-proto/build metadata; map and game-build metadata; round; event tick plus ingestion ordinal; raw event payload; participant SteamIDs/entity IDs; contributing tick snapshots; metric definition/version/parameters; endpoint references; and geometry/raycast version evidence where applicable. This makes each dashboard number auditable and lets parser or policy changes invalidate receipts rather than silently rewriting history.

## Decision

Adopt a pinned, vendored or forked demoparser Rust core behind a narrow `demo_ingest` facade. Use it for event and ordinary derived stats, with explicit query plans and single-thread fallback for stateful props. Build the analysis layer for all definitions and receipts. Treat crosshair placement as approximate until map/hitbox/LOS support exists; treat subtick exposure unavailable; and gate spray accuracy behind fixture validation. Pin all generated inputs, run the fixture corpus after CS2 updates, quarantine failures or suspicious empties, and never represent ingestion failure as a valid empty match.

The upstream project is MIT licensed ([LICENSE](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/LICENSE)).
