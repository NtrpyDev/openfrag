# Aim metrics v1 prototype

> THROWAWAY PROTOTYPE for issue 21. Delete the terminal shell after the metric decision is captured; do not treat this directory as production ingest code.

## Question

Can the pinned Demo parser and a representative bundled Demo support honest v1 definitions for crosshair placement, time-to-damage, and spray accuracy? The prototype compares the tempting proxy definitions with the evidence they actually consume, prints every measurement and pairing Receipt, and withholds a public metric when required geometry, timing, or impact evidence is absent.

## Run

From the repository root:

```sh
node prototypes/aim-metrics/run.js
```

The interactive terminal has five pages. For the complete deterministic state, including all per-event diagnostic records:

```sh
node prototypes/aim-metrics/run.js --json
```

The command expects the pinned demoparser checkout at `/tmp/openfrag-demoparser-audit`, `/tmp/demoparser-upstream`, or `/tmp/demoparser`. Override that lookup with `DEMOPARSER_ROOT`. Override the fixture with `AIM_DEMO_PATH`; doing so changes the evidence source and must not be used on another person's private Demo without permission.

## Evidence source and privacy

The default fixture is `src/parser/test_demo.dem`, tracked by [LaihoE/demoparser](https://github.com/LaihoE/demoparser/tree/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311) since upstream commit `4131a4fc02fda291b22421c20e1ca33f149535a7`. The pinned repository declares the MIT license. openfrag does not copy or redistribute the 60.6 MB fixture; the prototype reads the already-local upstream copy.

The output replaces names and SteamIDs with deterministic, within-Demo pseudonyms. A runtime invariant aborts if a raw SteamID reaches the serialized report. Noah's own match data is not searched or read.

## Compared definitions

- **Crosshair placement:** computes a diagnostic horizontal angle from attacker yaw to the damaged victim's pawn origin at the hurt tick. It withholds “crosshair placement” because there is no map geometry, hitbox, line-of-sight, exposure-start, rendered-crosshair, or miss sample.
- **Time-to-damage:** pairs a firearm hurt only with one exact-tick fire event from the same attacker and canonical weapon. It withholds reaction time and exposure-to-damage because subtick order and visibility onset are absent.
- **Spray accuracy:** checks the parser's `fire_bullets` and `player_bullet_hit` streams. It withholds the metric when those streams are absent; `weapon_fire / player_hurt` is explicitly rejected as a miss, pellet, penetration, and recoil-control measure.

Every page shows its definition, decision, empirical counts, limitations, error statement, and required Receipt inputs. `--json` exposes the full record arrays and a canonical payload fingerprint for repeat-run comparison.
