# GSI live evidence contract for v1

This contract defines what live Game State Integration snapshots may do before a Demo is imported. GSI is a buffered state stream, not an ordered or complete event log. The verified baseline is local official Competitive and Deathmatch on CS2 build 24134959. Premier parity remains unproven and must not be claimed until a user-controlled Premier capture confirms it.

## Decision fights

The optimistic policy treats every POST as an event. The safer policy treats each POST as a snapshot and derives only monotonic or state-transition candidates. Winner: snapshots only. The optimistic policy trusts every requested component. The observed runs omitted `allplayers`, `bomb`, `allgrenades`, and `phase_countdowns`; winner: trust only observed fields. The optimistic policy resumes candidates immediately after reconnect. Winner: seed and suppress the first payload after startup or recovery. The optimistic policy treats GSI labels as final. Winner: all post-match labels and metrics require Demo confirmation.

## Trusted live fields

Only these observed fields may drive provisional Auto Highlight candidates:

- `provider` timestamp and game identity for freshness diagnostics;
- local `player.steamid` and `player.name` for identity checks;
- `player.activity` as a coarse activity label, never as proof of a spectated teammate;
- `player.state.health`, `weapons`, cumulative `match_stats.kills`, cumulative `match_stats.deaths`, and `match_stats.round_kills` when present;
- observed `map`, `round`, and phase fields when present in a payload.

Health transitions, weapon removal, cumulative kill/death increases, and `round_kills` reset can trigger a provisional candidate. A candidate must include the source snapshot hash and receive time. GSI cannot establish exact shot timing, ordered event causality, final assists, party membership, or Demo-grade round labels.

## Identity and freshness

Configure the expected local SteamID from the signed-in local setup. Accept a payload for candidate processing only when its player identity matches that ID. A changed or missing identity is `identity_unknown`, not a new player and not a candidate. Never infer a spectated teammate from `observer_slot`.

Record provider timestamp, local monotonic receive time, payload hash, and byte length. Reject malformed, unauthorized, oversized, or non-JSON requests. Mark the stream stale when no valid POST arrives for three heartbeat intervals or 90 seconds, whichever is shorter. Provider timestamps and receive times must be nondecreasing for diagnostics; equal timestamps are valid and do not imply duplicate events.

Snapshots are not ordered events. Preserve arrival ordinal for audit, but do not reorder by provider time or synthesize missing transitions. Identical payload hashes within a short receive window are duplicates and produce no second candidate. Distinct snapshots with equal provider timestamps remain separate observations.

## Startup, reconnect, and failure

The listener seeds state after startup and recovery. The first valid payload establishes a baseline only. A listener outage, identity change, malformed payload, stale stream, or CS2 restart clears transition baselines. A listener restart alone is not assumed to restore delivery; guide the user to restart CS2 or reload a map.

Return explicit status codes for invalid method/path, bad content type, unauthorized token, oversized body, malformed JSON, and capture-storage failure. Keep serving requests when capture storage fails, but show a visible diagnostic and mark evidence incomplete. Never convert any GSI failure or empty snapshot into an empty Match.

## Provisional versus Demo-confirmed

Live UI may say **“Candidate detected, awaiting Demo confirmation.”** It may show receive time, observed health/weapon/stat transition, map if present, and a link to the redacted snapshot receipt. It must not call a candidate a kill, clutch, round win, or final highlight solely from GSI.

After Demo import, reconcile by Demo hash, local SteamID, map, round, and a bounded time window. The Demo is authoritative for final event labels, ticks, participants, assists, round outcome, and Receipts. If reconciliation fails, retain the candidate as **“Unconfirmed live candidate”** and do not include it in canonical stats.

## Privacy-safe diagnostics

Store redacted payloads or hashes, never the auth token. Diagnostics may retain provider timestamp, receive time, payload hash, byte length, HTTP status, field-presence bitmap, parser/listener version, and error code. Do not log raw names, SteamIDs, weapon inventories, chat, or complete payloads by default. A user-explicit troubleshooting export must be opt-in and clearly marked sensitive.

## Test vectors

1. First valid payload seeds state and emits no candidate.
2. Health 100 to 0 with weapon removal emits one provisional death candidate; repeated identical snapshots emit none.
3. Cumulative kills or deaths increasing by two across one snapshot records the delta but does not invent event order.
4. `round_kills` reset after respawn establishes a new baseline and emits no false kill.
5. Equal provider timestamps with distinct payload hashes preserve arrival order and produce at most the supported transitions.
6. A 35-second outage followed by listener restart marks stale and seeds the first recovered payload; no candidate is emitted until a later transition.
7. Missing `allplayers`, `bomb`, `allgrenades`, or `phase_countdowns` is reported as absent capability, never as an empty value.
8. A payload with a different local SteamID or only `observer_slot` changes is `identity_unknown` and cannot create a candidate.
9. Invalid token, wrong content type, oversized body, malformed JSON, and capture-write failure each produce their documented diagnostic without process death.
10. Demo reconciliation confirms a candidate, rejects a mismatch, and preserves an unconfirmed candidate outside canonical Rating and Receipts.
