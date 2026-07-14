# openfrag Rating v1

## Purpose

openfrag Rating is a private, per-match self-improvement score for comparing the local player's own Premier matches over time. It is a reproducible summary of recorded actions, not a measure of skill, effort, or causal responsibility. It is not a player rank, percentile, or win predictor.

The public formula identifier is `ofr-1.0.0`. Every displayed component and the aggregate must link to its Receipts.

## Decisions settled by debate

Each row records the strongest competing policies and the adopted winner.

| Decision | Competing policies | Winner | Reason |
|---|---|---|---|
| Purpose | personal improvement vs player comparison | Personal improvement | The local product has no honest global benchmark or role model. |
| Team outcome | direct win credit vs outcome shown separately | Separate outcome, 49 to 44 | A win is observable but does not allocate individual causality. |
| Formula shape | flat event ledger vs weighted component vector | Component vector, 10 to 8 | Components expose correlated concepts, missing evidence, and Receipts. |
| Component breadth | K/D/A only vs six bounded components | Six components, 10 to 9 | K/D/A alone omits proven opening, trade, utility, and clutch evidence. |
| Normalization | personal history, team-relative values, or fixed opportunity anchors | Fixed anchors, 10 to 8 | Personal baselines rewrite meaning and team-relative values depend on teammates. |
| Weights | equal, learned, or published policy weights | Published policy weights, 10 to 8 | Equal weights hide reliability differences and learned weights recreate a mystery score. |
| Scale | percentage, letter grade, raw points, or bounded index | `0.00` to `2.00`, neutral anchor `1.00`, 10 to 9 | It avoids percentile and school-grade implications while remaining trendable. |
| Outliers | clamp only the aggregate vs clamp each component | Clamp each component, 10 to 8 | One ace, long overtime, or parser anomaly cannot dominate the score. |
| No opening or clutch opportunity | zero vs neutral component | Neutral `0.5`, 10 to 8 | Lack of an opportunity is not failure or missing evidence. |
| Zero utility events | neutral vs observed zero | Observed zero, 10 to 8 | An eligible match with no recorded utility contribution is evidence, not absence. |
| Trade credit | trade kills plus deaths traded vs the player's trade kills only | Trade kills only, 10 to 8 | A teammate trading the player's death is context, not an action by the player. |
| Trade denominator | share of kills vs per-round rate | Per-round rate, 10 to 7 | A kill-share denominator can make an added non-trade kill lower the total Rating. |
| Missing required evidence | impute, renormalize, or suppress | Suppress the Rating, 10 to 8 | Missing parser evidence must never become zero or increase remaining weights. |
| Small samples | score every match, require a full match, or use thresholds | Eight-round preview and twelve-round trend threshold, 10 to 9 | It preserves partial evidence without presenting it as normally comparable. |
| Overtime | exclude or score normally | Score normally, 10 to 8 | Per-round rates already remove match-length advantage. |
| Roster disruption | score normally vs quarantine trends | Quarantine affected trends, 10 to 8 | Disconnects and prolonged team imbalance distort personal comparison. |
| Map, side, role, party, and opponent adjustments | infer adjustments vs show context only | Context only, 10 to 7 | The v1 evidence does not support honest adjustment models. |
| Reparses and formula changes | overwrite history vs append versioned runs | Append versioned runs, 10 to 7 | Silent replacement destroys reproducibility and auditability. |

## Eligible rounds

Let `R` be the number of eligible rounds for the local player. A round is eligible when all of these are true:

- it is not warmup;
- at `round_freeze_end`, the player has a T or CT controller, a valid pawn, and `life_state` reports alive;
- the parser observes exactly one custom completed `round_end`, guarded by a one-step `round_end_count` increment, with a recognized T or CT winner.

The player state at `round_freeze_end` is the latest entity state at or before the event's `(tick, ingestion_ordinal)`. A missing or stale controller-to-pawn mapping invalidates the round. An unexpected respawn, in-round team change, or irreconcilable disconnect invalidates the round rather than creating a guessed classification.

Regulation and overtime rounds use identical scoring rules. All component inputs are calculated from exactly this eligible-round set.

Eligibility is fixed before component extraction. If any required event stream fails integrity or attribution across that fixed set, suppress the entire Rating. Never remove a round merely because its component evidence is unfavorable or malformed.

Enemy health damage is derived from `player_hurt.dmg_health`, capped by the victim's immediately preceding health snapshot. Each event requires attacker, victim, and team-at-event attribution. Self-damage, team damage, world damage, and damage beyond preceding health do not score.

## Components

Every component is clamped to the inclusive range `[0, 1]` before weighting.

### Direct damage, 30 percent

Formula version `ofr-1.0.0` classifies the event weapon strings `hegrenade`, `inferno`, `molotov`, and `incgrenade` as damaging utility under versioned taxonomy `damage-event-taxonomy-1`. `UD` is enemy health damage from those strings. `DD` is all scored enemy health damage minus `UD`, making the two damage buckets exhaustive.

```text
D = clamp(DD / (160 * R), 0, 1)
```

Eighty direct damage per eligible round produces the component midpoint `0.5`. This is a published policy anchor, not a claim about expected or average player performance. Always show raw direct damage per round beside the component.

### Frag balance, 30 percent

`K` is enemy kills. `X` is player deaths, including enemy, suicide, and world deaths but excluding administrative resets.

Derive both from the ordered `player_death` ledger using team-at-event identity. A missing attacker, victim, team relation, or stable ingestion ordinal makes the Rating unavailable when that field is required to classify the event.

```text
F = clamp(0.5 + (K - X) / (2 * R), 0, 1)
```

Equal kills and deaths produce `0.5`.

### Opening duel results, 15 percent

The opening duel is the first qualifying enemy-attacker and enemy-victim death among all players in an eligible round, ordered by `(tick, ingestion_ordinal)`. Team kills, suicides, and world deaths do not create an opening duel. The player records an opening win as the attacker or an opening loss as the victim. If the first qualifying event cannot be ordered or attributed, the Rating is unavailable.

```text
O = opening_wins / (opening_wins + opening_losses)
```

If the player has no opening-duel participation, `O = 0.5`.

### Trade kills, 10 percent

A trade kill occurs when the player kills an opponent who killed a teammate no more than `5.000` Demo seconds earlier in the same round. The opponent must remain alive immediately before the trade kill. Demo seconds equal the tick difference divided by the Demo tick rate. One kill may trade at most one death and is assigned to the most recent eligible teammate death by `(tick, ingestion_ordinal)`. `TK` is the number of the player's trade kills. Missing tick-rate or liveness evidence makes the Rating unavailable.

```text
T = clamp(2 * TK / R, 0, 1)
```

The component midpoint is `0.25` trade kills per eligible round. This is a published policy anchor, not an estimate of average performance. A teammate trading the player's death is shown as death context but does not add Rating credit. This component reports recorded trade kills, not trade skill.

### Utility events, 10 percent

`UD` is attributed enemy health damage from the utility strings in `damage-event-taxonomy-1`. `FA` is a death event where `assistedflash` is true, the assister identity is the local player, and the assister and attacker are teammates at the death event. A disagreement among those fields makes the Rating unavailable.

```text
U_damage = clamp(UD / (30 * R), 0, 1)
U_flash  = clamp(FA / (0.5 * R), 0, 1)
U        = 0.70 * U_damage + 0.30 * U_flash
```

Zero measured utility events produce `U = 0` only when utility attribution coverage passes every integrity check. Missing utility evidence makes the Rating unavailable.

### Clutch conversion, 5 percent

A clutch opportunity begins at the first event-ordered transition in an eligible round where the player is their team's sole living player and at least one opponent is alive. The living roster starts from the validated `round_freeze_end` snapshot and updates through ordered deaths and disconnects. A round creates at most one opportunity. The opportunity converts when the player's team wins that round, including a planted-bomb win after the player dies.

```text
C = clutch_round_wins / clutch_opportunities
```

If the player has no clutch opportunity, `C = 0.5`. This is a bounded clutch outcome, not a claim about clutch skill. It is the only round-outcome fact in the Rating because it is conditioned on the player being the sole surviving teammate. Raw round wins, match wins, and round differential remain context only.

## Aggregate

```text
S = 0.30D + 0.30F + 0.15O + 0.10T + 0.10U + 0.05C
RatingExact = 2 * S
rating_bp = round_half_up(10000 * RatingExact)
display_centi = floor((rating_bp + 50) / 100)
```

Store component numerators and denominators as exact integers and evaluate component ratios and the weighted sum as exact rational values or defined fixed-point integer operations. Binary floating-point evaluation is not canonical. `round_half_up` uses integer numerator and denominator division. Display `display_centi / 100`. Never derive another value from the displayed number.

The scale is a summary convention, not a probability, percentile, or claim that `1.00` is the population average.

## Worked example

For `R = 20`, suppose the player records `1,600` direct damage, `18` kills, `16` deaths, three opening wins and two opening losses, four trade kills, `300` utility damage, two flash assists, and one conversion from two clutch opportunities.

```text
D = 0.5000
F = 0.5500
O = 0.6000
T = 0.4000
U = 0.4100
C = 0.5000

RatingExact = 1.0220
rating_bp = 10220
Displayed Rating = 1.02
```

The match result does not alter this calculation.

## Availability and irregular matches

- `R < 8`: Rating unavailable even when all observed evidence is valid.
- `8 <= R < 12`: Preview Rating based on complete evidence for the eligible rounds, excluded from trends.
- `R >= 12`: rated when all integrity rules pass.
- If required events are missing, malformed, contradictory, or fail reconciliation, the Rating is unavailable at every sample size. Do not impute a value or renormalize weights.
- A live round is missed when the local SteamID occupies the Match roster but lacks a valid T or CT controller, pawn, or state snapshot at `round_freeze_end`. Missing any live round, including a leading round before joining or trailing round after leaving, makes the result a Preview Rating and excludes it from trends.
- If either team begins more than one live round with fewer than five identified active, non-spectator player accounts, mark the Match roster-imbalanced and exclude its Rating from trends.
- A surrender produces a Preview Rating only and is excluded from trends. A forfeit or administrative termination without a canonical winner sequence makes the Rating unavailable.
- Include overtime in the canonical whole-Match calculation, retain an overtime flag, and expose regulation and overtime vectors as diagnostics.
- The canonical Match vector is calculated once from pooled raw inputs across all eligible rounds. T-side, CT-side, regulation, and overtime vectors are diagnostic only and never combine to create the canonical Rating. Do not apply hidden side correction.
- Show map, side, party-data availability, opponent, match result, round differential, and rolling win rate as context. Party data remains unavailable until its dedicated validation succeeds. None directly changes `ofr-1.0.0`.

## Receipt contract

The round ledger is the evidence layer. Each eligible or excluded round retains:

- Demo hash, parser build, generated-protobuf build, game build, and formula version;
- round number, event tick plus stable ingestion ordinal, and player side;
- component input events, participant SteamIDs or entity IDs, and required snapshots;
- raw event bytes when retained, or an immutable event hash plus parsed fields;
- derived metric definition and parameters, including the trade window;
- inclusion or exclusion status and reason;
- optional linked Clip and time range.

Each component expands to its contributing and excluded rounds. The aggregate expands to all six components and the exact weighted sum. A Receipt proves what was recorded and how it was calculated, not why an event happened.

## Versioning and comparability

A rating calculation identity contains:

```text
demo_hash
parser_build
generated_proto_build
metric_definition_version
formula_version
```

The same identity must reproduce identical component inputs, Receipts, and `rating_bp`.

- Runs are append-only. Never overwrite an earlier result.
- A parser or generated-schema change creates a new run even when the displayed Rating is unchanged.
- Promote a new parser run only after component-level and Receipt-level diffs against the prior canonical run pass review.
- A formula or metric-definition change requires recomputing the complete retained local corpus into a separate coherent trend series.
- Never mix formula versions in one trend line.
- Preserve previous results and explain why a canonical result was recomputed.
- Record the game build. If a CS2 update changes input semantics, start a new canonical run and rebuild the retained corpus before comparing trends.

## Invariants

- Every component lies in `[0, 1]`; Rating lies in `[0, 2]`.
- Six component values of `0.5` produce exactly `1.0000`.
- Weights sum exactly to `1.00`.
- Duplicating every eligible round leaves Rating unchanged.
- Adding valid direct enemy damage cannot lower `D`.
- Adding an enemy kill without another death cannot lower `F`.
- Adding a non-trade enemy kill without another death cannot lower Rating.
- Adding a trade kill cannot lower `F`, `T`, or Rating.
- Map and side labels do not alter the formula. Event ordering may alter `O`, `T`, and `C` only through their published definitions.
- No global corpus, network state, clock time, or future Match can alter an existing `ofr-1.0.0` result.

## Verification requirements

The formula is not ready for production until a pinned fixture corpus proves all of these:

- every eligible round has exactly one recognized custom `round_end`, and every excluded round has one enumerated reason;
- local-player state at `round_freeze_end` handles missing pawns, spectators, stale controller mappings, late joins, disconnects, halftime, and overtime deterministically;
- round-ledger `K` and `X` reconcile to direct per-round and full-Match parser totals when those totals are available;
- every scored damage event belongs to exactly one eligible round, has attacker and victim identity plus team relation, and enters exactly one of `DD` or `UD`;
- `damage-event-taxonomy-1` covers fixtures for bullets, knife, taser, HE, molotov, incendiary, smoke, flash, decoy, self, team, world, lethal, and excess reported damage;
- flash-assist fixtures verify the boolean, assister identity, attacker identity, and team relationship together;
- same-tick deaths, suicide, world death, team kill, multiple candidate trade deaths, missing tick rate, lone-player transitions, disconnects, and planted-bomb wins reproduce identical `O`, `T`, and `C` from `(tick, ingestion_ordinal)`;
- zero-opportunity opening and clutch components produce `0.5`, while complete utility coverage with zero events and zero trade kills produces `0`;
- sample, surrender, roster-imbalance, and missing-round rules produce the required unavailable, Preview Rating, rated, and trend-exclusion states;
- golden formula cases verify clamping, exact-rational evaluation, half-up rounding, and the worked example;
- property tests verify every invariant, including that a non-trade kill cannot lower Rating;
- repeated parsing with the same calculation identity produces byte-identical component inputs, Receipts, and `rating_bp`;
- parser and formula upgrades produce append-only runs and never mix calculation versions in one trend.

## Evidence boundary

The pinned parser evaluation found opening duels, clutches, utility damage and flash attribution, death context, and trades feasible from Demo events and snapshots. Crosshair placement, exposure timing, spray quality, inferred role, opponent strength, and party adjustment do not enter v1 ([demoparser evaluation](https://github.com/NtrpyDev/openfrag/blob/97face7e47c877a78e87e2d3e2b6a42fd8e3066a/docs/demoparser-evaluation.md)).
