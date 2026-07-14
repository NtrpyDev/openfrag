# GSI and Demo merge research

Issue: [#22](https://github.com/NtrpyDev/openfrag/issues/22)  
Decision: **do not merge GSI into canonical v1 analytics facts or Receipts.**
The Demo remains the sole post-match authority. GSI may retain a separate,
provisional live-candidate receipt that points to a Demo receipt only after
conservative reconciliation succeeds.

## Sources and boundary

This uses only primary sources: Valve's [Game State Integration reference](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration), the project's pinned [GSI evidence contract](https://github.com/NtrpyDev/openfrag/blob/4245a7145d3bf102bb70aca329f52f4c166edbb6/docs/gsi-evidence-contract.md), its [observed listener-capture resolution](https://github.com/NtrpyDev/openfrag/issues/10#issuecomment-4972999428), and the pinned [demoparser source](https://github.com/LaihoE/demoparser/tree/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311) and [evaluation](https://github.com/NtrpyDev/openfrag/blob/97face7e47c877a78e87e2d3e2b6a42fd8e3066a/docs/demoparser-evaluation.md).

The contract is the current implementation boundary. It was observed on local
official Competitive and Deathmatch, not Premier. In those captures, requested
`allplayers`, `bomb`, `allgrenades`, and `phase_countdowns` were absent; distinct
POSTs could share a provider timestamp; and listener restart did not itself
restore delivery. Therefore this document makes no Premier or event-order claim.

## Field overlap and gaps

| GSI field or observation | Demo evidence | Join value | v1 decision |
|---|---|---|---|
| `player.steamid` | Player roster metadata and event participant SteamIDs | The only stable player identity shared by both pipelines. | Require it to equal the configured local SteamID and the Demo local-player SteamID. It identifies a player, not a match. |
| `player.name` | Player metadata/name fields | Display-only corroboration. Names can change and are not a key. | Never join or persist as the identity key. |
| `provider` game identity and timestamp | Demo header and parser/build metadata | Diagnostics only. No shared match ID or server tick is established. | Retain as live provenance; do not use as a canonical join key. |
| Observed `map`, `round`, and phase | Demo map context, canonical completed rounds, and round-end ticks | A coarse candidate filter when both sides provide it. | Reject an association on a known map mismatch. A matching map/round is necessary but insufficient. |
| Local health, weapons, cumulative kills/deaths, and `round_kills` transitions | Tick properties plus `player_hurt`, `weapon_fire`, `player_death`, and direct per-round counters | GSI can announce a possible live moment; Demo can establish event participants and ordering. | GSI cannot finalize a kill, assist, damage value, opening duel, trade, clutch, or Rating input. |
| GSI money or economy state | Demo purchase events and player/economy properties | The verified GSI contract does not admit money as a trusted live fact. Demo can independently support economy analysis. | No canonical economy decision comes from GSI. Do not add a money field until a capture proves its presence and semantics. |
| `allplayers`, bomb, grenades, enemy state | Demo participant, bomb, projectile, and event evidence | No reliable live overlap in the observed contract. | Treat missing live fields as absent capability, never as empty game state. |
| Party/lobby membership | Reservation data would be required; the pinned parser currently drops its RankReveal message path | Neither current pipeline supplies a validated party fact. | Do not infer it. |

The pinned parser's `DemoOutput` contains ordered `game_events`, tick columns,
roster metadata, and header data. Its custom `round_end` is emitted only for a
one-step round-end counter increment and carries round, winner, reason, and
tick. Those are suitable canonical post-match inputs; a GSI snapshot is not.

## Identifiers and clocks

Persist these separately:

| Item | Value and stability | Rule |
|---|---|---|
| Local player | Configured SteamID, then GSI `player.steamid` and Demo participant SteamID | Require equality. A missing or changed GSI identity is `identity_unknown`, never a new player. |
| Canonical match | Demo SHA-256 plus parser build, generated-protobuf build, and Demo game/header metadata | The Demo hash is the match-artifact identity. GSI supplies no proven equivalent. |
| Live snapshot | Redacted payload hash, arrival ordinal, provider timestamp if present, local monotonic receive time, field-presence bitmap | Diagnostic and candidate provenance only. Do not use a payload hash as a game-event ID. |
| Demo event | Demo hash, parser build, round, tick, ingestion ordinal, participants, raw event payload | Canonical Receipt identity. Entity IDs are parser-local and must not cross the pipeline boundary. |

There are three non-equivalent clocks:

1. **GSI provider time** is producer-supplied snapshot metadata. Captures showed
   repeated values on distinct POSTs, so it is neither an event ID nor a strict
   ordering clock.
2. **Listener receive time** is a local monotonic observation at HTTP arrival.
   Buffering, throttling, heartbeat, outage, and restart behavior make it an
   uncertain upper-bound style observation, not game-event time.
3. **Demo tick** is the parser's ordered in-demo position. It is authoritative
   for Demo event ordering, but the sources establish no durable affine mapping
   from a GSI provider/receive timestamp to a Demo tick.

Consequently, v1 must not convert a GSI timestamp into a Demo tick, claim a
sub-round clip offset is Demo-confirmed, or use a receive-time window as a
metric input. Equal GSI timestamps preserve arrival ordinal only.

## Conservative alignment algorithm

This algorithm may create a secondary link, never a merged event:

1. Accept a GSI snapshot only under the live contract: valid loopback request,
   expected marker, matching configured SteamID, fresh stream, and a seeded
   baseline. The first valid snapshot after startup or recovery cannot emit a
   candidate.
2. Store the live candidate with its redacted snapshot receipt. Do not call it a
   kill, round win, or highlight.
3. Associate a Demo only after the user/import pipeline has identified that
   Demo artifact. Require local SteamID equality. If present on both sides,
   require map equality. If a GSI round is present, require exactly one matching
   canonical Demo round; otherwise leave the candidate unconfirmed.
4. A Demo event may confirm the candidate only if its event type and local-player
   role fit the observed state transition. This is a categorical confirmation,
   not a timestamp-to-tick conversion. Attach the canonical Demo Receipt and
   retain the live receipt as supporting provenance.
5. If any identity, map, round, state-transition, or uniqueness check fails,
   retain `unconfirmed_live_candidate` outside canonical stats, Rating, and
   Demo Receipts. Never choose the nearest tick, interpolate a clock, or impute
   a missing event.

The existing contract's bounded time window can be retained only as a UI or
operator-review hint. It cannot overcome the missing clock crosswalk above.

## Does GSI add a canonical v1 fact?

No. The only facts GSI adds are live operational provenance: the local receiver
saw an identity-matched snapshot at a local receive time and may have started a
clip workflow. That is useful for provisional highlight UX and diagnostics, but
it does not improve the truth of a post-match statistic beyond the imported
Demo. Economy does not change this result because it is not a trusted observed
GSI field and is independently available to Demo analysis.

The v1 Receipt decision is therefore:

- **Canonical analytics Receipt:** Demo-only identity and evidence.
- **Live candidate Receipt:** a separate redacted GSI snapshot hash, provider
  timestamp when present, receive time, ordinal, field-presence bitmap, and
  listener version.
- **Reconciliation edge:** optional relation from a live candidate to one
  canonical Demo Receipt, labelled `confirmed` or `unconfirmed`; it never adds
  a GSI field to the canonical metric numerator, denominator, or event payload.

## Required verification

Before implementation, add fixtures proving the following:

1. Same SteamID but different map or ambiguous round never creates a link.
2. Distinct GSI snapshots with equal provider timestamps preserve ordinal and
   cannot create two canonical events from one Demo event.
3. Startup, stale recovery, changed identity, duplicate hash, listener outage,
   and absent optional fields create no canonical fact.
4. A confirmed candidate retains both receipts, while a failed match retains
   only its live receipt and is excluded from Rating and final highlight labels.
5. Parser or generated-protobuf changes reproduce or version Demo Receipts
   independently of unchanged GSI captures.

This preserves the product boundary: GSI is the live trigger surface and Demo
is the source for post-match analytics.
