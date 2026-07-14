# Automated GSI spike findings

These findings come from the native Linux CS2 client, build 24134959, using the throwaway loopback listener. Raw captures remain untracked because they contain account identity and game-state data.

## Fixtures

- Aimbotz was not a useful fixture. Several minutes of active workshop play continued to report `player.activity: menu` and omitted map, round, weapon, score, and delta state.
- An official local Dust II Competitive match produced `map.name: de_dust2`, `map.mode: competitive`, `player.activity: playing`, live health, weapons, match statistics, round state, and `previously` and `added` deltas.
- An official local Dust II Deathmatch produced the same top-level and player field families with `map.mode: deathmatch`.
- Neither local mode emitted `allplayers`, `bomb`, `allgrenades`, or `phase_countdowns` during the observed runs, despite those components being requested.

## Observed transitions

- Competitive moved from menu to warmup, then `freezetime`, `live`, `over`, and back to `live`. The map round changed from 0 to 1.
- Deathmatch damage produced successive health snapshots of 100, 78, 60, 41, 22, and 0.
- A Deathmatch death changed the persistent death counter from 0 to 1, removed weapons, retained the local provider/player identity relationship, and was followed by health 100 and restored weapons on respawn.
- Deathmatch `round_kills` increased with kills and reset to 0 on respawn while total kills and deaths remained cumulative.
- `observer_slot` changed while health was 0, but the observed player identity remained the local player. The run did not establish that GSI identifies a spectated teammate.
- Multiple distinct posts can share the same whole-second provider timestamp. No identical sanitized-payload hashes were observed, and both provider timestamps and listener receive times were nondecreasing.
- The shortest observed receive gap was 212 ms. This run did not record independent action timestamps, so it cannot claim exact action-to-delivery latency.

## Recovery behavior

The listener was stopped for 35 seconds, long enough to cross the configured 30-second heartbeat, then restarted on `127.0.0.1:27100`. CS2 did not resume posting during an additional 110-second observation window. Restarting CS2 restored delivery immediately. Production must not assume that bringing the listener back is enough; recovery needs an explicit stale state and user-facing restart or map-reload guidance unless a later test proves a reliable retry path.

## Contract implications

- Treat each payload as a state snapshot, not an ordered game event.
- Seed state after startup or recovery. Do not emit a highlight from the first payload.
- Health, weapons, cumulative statistics, and `round_kills` can support live candidates, but Demo evidence must confirm final labels.
- Do not treat `observer_slot` changes as proof that a different player is being spectated.
- Premier was not automated. Public matchmaking would affect other players and cannot be used as an unattended test fixture. The local Competitive result is evidence for the base ruleset, not proof of Premier parity.
