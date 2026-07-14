# GSI capability reference: what CS2 actually tells us

Status: research note for [GSI capability reference: what CS2 actually tells
us](https://github.com/NtrpyDev/openfrag/issues/3). This is intentionally a
narrow reference to Counter-Strike Game State Integration, not an
implementation guide.

## Source and confidence rules

The only public specification used here is Valve Developer Community's
[Counter-Strike: Global Offensive Game State Integration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration)
page. It documents Counter-Strike GSI configuration and is the closest Valve
primary reference located for CS2. The page title still says CS:GO, so claims
below concern the documented GSI protocol, not an assertion that every field is
present in every current CS2 build or mode.

- **Documented** means the Valve page specifies the setting, component, or
  behavior.
- **Sample observation** means it appears in the Valve page's example payload
  and is not treated here as unconditional.
- **Unknown** means the primary material reviewed does not establish it. It is
  not a negative claim about the game.

## Local CS2 build evidence

**Local-build evidence, not a schema specification.** The research machine has
an installed Counter-Strike 2 App 730 at
`/mnt/shared/SteamLibrary/steamapps/common/Counter-Strike Global Offensive`.
Its `appmanifest_730.acf` reports build ID `24134959`; its
`game/csgo/steam.inf` reports ClientVersion and ServerVersion `2000873`,
PatchVersion `1.41.6.9`, and VersionDate `Jul 09 2026`.

No `gamestate_integration_*.cfg` file was found in that installation. A string
inspection of its
`game/csgo/bin/linuxsteamrt64/libclient.so` found the configuration filename
pattern and these renderer names:

```text
provider_v1                 map_v1                       map_round_wins_v1
player_id_v1                player_state_v1              player_match_stats_v1
player_weapons_v1           player_position_v1           round_v1
bomb_v1                     phase_countdowns_v1          allplayers_id_v1
allplayers_state_v1         allplayers_match_stats_v1    allplayers_weapons_v1
allplayers_position_v1      allgrenades_v1               tournamentdraft_v1
precision_time              precision_position           precision_vector
```

This is first-party local binary evidence that this one CS2 build retains the
named integration renderers and precision settings. It does **not** establish
the JSON/VDF payload shape, which fields are populated, role or mode
availability, emission cadence, compatibility with later builds, or any
network-security behavior. Those remain governed only by the Valve
documentation or marked unknown below.

The same binary contains these likely GSI JSON key strings:
`activity`, `appid`, `armor`, `assists`, `bomb`, `burning`, `clan`, `deaths`,
`defusekit`, `equip_value`, `flashed`, `forward`, `health`, `helmet`, `kills`,
`mode`, `money`, `mvps`, `name`, `observer_slot`, `phase`, `phase_ends_in`,
`position`, `round`, `round_killhs`, `round_kills`, `score`, `smoked`,
`state`, `steamid`, `team`, `timestamp`, and `version`. This merely
corroborates the published example and must not be used as a definitive schema:
generic strings may have another call site in the binary.

## Setup and transport

| Item | Status | Reference |
|---|---|---|
| Configuration location | **Documented.** Place a VDF configuration file named `gamestate_integration_*.cfg` in the game's `cfg` directory. The documented filename prefix is how the game discovers an integration. | [Valve: setup and sample configuration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration) |
| Receiver | **Documented.** `uri` is the endpoint to which the game sends game-state HTTP POST requests. The sample uses a loopback HTTP URI. | [Valve: configuration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration) |
| Authentication | **Documented.** A configured `auth` block can contain a `token`; the request payload includes it under `auth.token`, allowing the receiver to reject a mismatched sender. | [Valve: authentication](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Authentication) |
| Delivery deadline | **Documented.** `timeout` configures the HTTP request timeout in seconds. A timeout is a delivery bound, not an end-to-end latency guarantee. | [Valve: configuration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration) |
| Network security | **Unknown.** The reviewed primary documentation does not specify TLS support, certificate validation, request signing, replay protection, source-IP restriction, or token rotation. Treat the token as a shared secret and bind a development receiver to loopback unless transport behavior has been validated in the target CS2 build. | [Valve: configuration and authentication](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration) |

Minimal documented shape, with values chosen by the operator:

```vdf
"openfrag GSI"
{
  "uri"       "http://127.0.0.1:3000/gsi"
  "timeout"   "5.0"
  "buffer"    "0.1"
  "throttle"  "0.5"
  "heartbeat" "60.0"
  "auth" { "token" "replace-with-a-random-local-secret" }
  "data" { "provider" "1" "map" "1" "round" "1" "player_id" "1" "player_state" "1" }
}
```

The key names and VDF structure are documented. The example values are sample
operator choices, not a recommended production latency profile. See Valve's
[sample configuration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Sample_Configuration).

## Emission, pacing, and change detection

| Control | Status | What it means |
|---|---|---|
| `buffer` | **Documented.** The game buffers state changes for the configured seconds before sending, so nearby changes can be coalesced. | [Valve: buffer and throttle](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling) |
| `throttle` | **Documented.** It sets a minimum interval between state transmissions. It is not a sample frequency guarantee. | [Valve: buffer and throttle](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling) |
| `heartbeat` | **Documented.** It requests a periodic full state update even when no state changes. | [Valve: heartbeats](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Heartbeats) |
| Change deltas | **Documented.** GSI can include `previously` and `added` objects describing changed or newly added data, alongside the current state. Receivers must therefore tolerate partial changed-data structures. | [Valve: game state components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components) |
| Precision | **Documented.** `output` supports `precision_time`, `precision_position`, and `precision_vector` settings. These control serialized numeric precision, not the game's simulation precision or a latency budget. | [Valve: configuration](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration) |
| Exact emission order, retry policy, queue limit, dropped-update signal, and clock origin | **Unknown.** The reviewed primary reference does not define them. | [Valve: buffering, throttling, and heartbeats](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling) |

### Latency consequence

**Documented inference.** A state change cannot be assumed to arrive faster
than the configured buffering and throttling behavior, plus game scheduling,
HTTP transport, receiver processing, and any timeout or retry behavior. The
documentation defines the first two controls but does not publish a maximum
end-to-end delay. GSI is therefore suitable for coarse live state and UI
automation only after an empirical latency test. It is not, on the cited
evidence alone, a tick-accurate or event-complete analytics feed. [Valve:
buffering and throttling](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling)

## Requested components and documented fields

The `data` block selects components. The list below reports the documented
component names, then separates stable documented shape from fields shown only
in Valve's example data. Requesting a component does not make it available in
every role or mode. [Valve: game-state components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components)

| Component | Documented purpose and fields | Sample observation | Role or mode boundary |
|---|---|---|---|
| `provider` | Identifies the game integration. The documented schema includes provider identity, application ID, version, Steam ID, and timestamp. | The example shows `name`, `appid`, `version`, `steamid`, and `timestamp`. | No role-specific guarantee is documented. [Valve: components and example data](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components) |
| `map` | Describes the current map and match context. | The example includes map `name`, `mode`, `phase`, round number, team scores, spectators, and series information. | Exact map fields and availability by mode are not guaranteed by the reviewed page. [Valve: example data](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Example_Data) |
| `round` | Describes round state. | The example uses a phase value and shows a winning team after a round. | Do not infer a complete round-event log from phase transitions. [Valve: round data](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components) |
| Player data | The documented payload groups local or observed-player identity, activity, state, weapons, position, and match statistics under `player`. The inspected CS2 build retains split renderers `player_id_v1`, `player_state_v1`, `player_match_stats_v1`, `player_weapons_v1`, and `player_position_v1`; use the matching split names in `data`, not a speculative `player` selector. | The example shows health, armor, helmet, flash/smoke/burn state, money, round kills, equipment value, weapon ammo/state, and kills/assists/deaths/MVPs/score. | Valve distinguishes the player being played from the player being observed. It does not promise opponent state to a normal player. [Valve: player data](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Player) |
| All-player data | The documented payload provides player information indexed by Steam ID when GSI is running on a spectator client. The inspected CS2 build retains `allplayers_id_v1`, `allplayers_state_v1`, `allplayers_match_stats_v1`, `allplayers_weapons_v1`, and `allplayers_position_v1`. | The example shows player-style state and weapons for multiple IDs. | This is the documented spectator-only expansion. A playing client must not be designed to receive full opponent data. [Valve: allplayers](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#All_Players) |
| `bomb` | Describes bomb state. | The example shows state, position, countdown, and planter/player identity when relevant. | It is state reporting, not a bomb-event history or defuse timing guarantee. [Valve: bomb](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Bomb) |
| `phase_countdowns` | Describes the current phase countdown. | The example shows current phase and seconds remaining. | It is not a server-clock synchronization protocol. [Valve: phase countdowns](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Phase_Countdowns) |
| `allgrenades` | Requests grenade information. | The example documents grenade entries with owner/type/position and lifetime-style fields. | The reviewed material does not establish that it is a complete projectile or damage feed in every mode. [Valve: all grenades](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#All_Grenades) |

### Player, spectator, and mode visibility

**Documented.** GSI exposes local-player data when playing and can expose all
players when spectating. This is a deliberate information boundary. Build the
receiver to treat `player` as perspective-dependent and only enable
all-player analytics on an authorized spectator or broadcast setup. [Valve:
all players](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#All_Players)

**Unknown.** The reviewed primary source does not provide a complete matrix for
Competitive, Premier, Casual, Deathmatch, practice, demo playback, GOTV, and
every spectator permission. Validate each target mode with captured payloads
before claiming feature support.

The identity check is part of correctness, not just diagnostics. The documented
`player` component is perspective-dependent; the exact CS2 transition after
death is an empirical question. For local attribution, capture testing must
establish a trusted local-account identity and compare it with the payload
player ID; do not treat provider metadata alone as an identity proof.

## Phase reference

These are state snapshots, not callbacks. A receiver should keep its own
state machine and detect transitions from successive payloads.

| Subject | What GSI can report | What it cannot establish by itself |
|---|---|---|
| Map and match | Map name, mode, phase, round and score-related context are documented or shown in Valve example data. | A canonical match identifier, authoritative match result, or complete historical score timeline is not specified in the reviewed page. [Valve: map](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Map) |
| Round | Round phase and associated end information are documented as state. | Opening duel, clutch, trade, round-start exact timestamp, and every kill sequence require derived logic and may be ambiguous after coalescing. [Valve: round](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Round) |
| Bomb | Bomb state and countdown-style information can be present. | A complete plant, drop, pickup, defuse, explode event ledger or exact server timing is not specified. [Valve: bomb](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Bomb) |
| Player | Local or observed-player state, weapons and aggregate match stats are available under the documented role boundary. | Enemy positions for a playing client, aim path, input, shot trajectory, hitbox, damage attribution, and visibility are not documented GSI outputs. [Valve: player and allplayers](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Player) |

## Hard analytics limits

The limits below are deliberately conservative. “Not supported” means the
needed primitive is not documented in the reviewed GSI component schema, not
that no other CS2 interface could supply it.

| Question | GSI verdict | Reason |
|---|---|---|
| Show current health, equipment, weapon, round phase, score, bomb state, and a local overlay | **Supported as live state, subject to role and component selection.** | These are documented GSI components or fields. [Valve: components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components) |
| Detect a likely local kill, death, plant, or round transition | **Possible as derived, lossy state inference.** | Payloads are buffered/throttled state snapshots, with optional deltas, not a documented ordered event stream. [Valve: buffering and components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling) |
| Produce authoritative opening-duel, trade, clutch, utility-damage, or death-context statistics | **Not established by GSI alone.** | The required complete, ordered per-player event and attribution semantics are not specified, and playing clients have a documented all-player visibility boundary. [Valve: player/allplayers boundary](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#All_Players) |
| Measure crosshair placement, time-to-damage, spray accuracy, recoil control, or tick-level movement | **Not supported by the documented GSI surface.** | The reviewed schema does not document per-tick view angles, input commands, bullet impacts, hitboxes, or an exact game-tick stream. [Valve: game-state components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components) |
| Treat GSI as anti-cheat-safe or approved for every competitive environment | **Unknown.** | The reviewed GSI reference does not make policy, anti-cheat, or tournament-compliance promises. |

## Consequences for Auto Highlight detection

The documented surface supports a conditional, conservative trigger model. It
does not support the complete founding Auto Highlight ruleset without later
Demo confirmation.

| Proposed trigger | GSI-only verdict | Required handling |
|---|---|---|
| 3k, 4k, or ace | **Sample-based candidate.** A captured change to the example-payload `round_kills` field plus a round-phase transition may suggest a count. Buffering, reset order, identity changes, and missed transitions still need capture testing. | The listener spike must prove the counter and round-end ordering in Premier. Save a candidate Clip, then let the Demo provide the authoritative label and Receipt. |
| Clutch win 1v2+ | **No-go as an authoritative GSI trigger.** A playing client is not promised complete live opponent roster and alive-state data. | Do not claim or label a clutch from GSI alone. A broader round-end candidate may be classified after Demo parsing. |
| Knife kill | **No-go as an authoritative GSI trigger.** The documented surface has current weapon state and cumulative kill counters, not a reliable per-kill weapon, attacker, or victim event. | Do not label a knife kill from GSI alone. Confirm it from the Demo before applying the label or Receipt. |
| MVP or assist | **No-go as an attributed event.** Match statistics can contain cumulative `mvps` and `assists`, but not the associated event, recipient context, reason, or exact award time. | Treat changes as display hints only until Demo reconciliation. |

On receipt of a conservative state transition, the receiver can request a
Replay Buffer save, but GSI cannot be the final evidence for the highlight
type. The Demo remains the post-match source of truth.

## Receiver requirements

1. Parse each POST as a snapshot plus optional `previously` and `added` deltas.
   Do not assume every component or field is present. [Valve: components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Game_State_Components)
2. Select only needed components and store a local last-known state. The
   documented buffer, throttle, and heartbeat behavior makes transition logic
   receiver-owned. [Valve: buffering and heartbeats](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Buffering_and_Throttling)
3. Require the configured token, limit listener exposure, and do not put a
   long-lived secret in logs. Token comparison is documented; transport
   protections beyond that remain unknown. [Valve: authentication](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Authentication)
4. Capture representative payloads for each intended role and mode before
   making product claims. The official reference establishes the spectator
   distinction but not a complete CS2 mode matrix. [Valve: allplayers](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#All_Players)
5. Bind the receiver to loopback, accept only the configured path and `POST`,
   impose a small body limit and parser depth limit, and treat every field as
   untrusted. The token is a shared request marker copied from the cfg, not a
   Valve identity proof, signature, encryption layer, or replay defense.

## Bottom line

GSI is a Valve-documented HTTP state feed that can power local live overlays
and coarse state-driven automations. Its documented controls intentionally
buffer and throttle transmissions, and its information is perspective-bound.
Use it for live state, not as the sole source for exact post-match analytics or
input/aim telemetry. [Valve: configuration and components](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Game_State_Integration#Configuration)
