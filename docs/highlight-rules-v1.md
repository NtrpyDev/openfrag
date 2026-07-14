# Highlight rules v1

Auto Highlights are provisional while a match is live and final only after Demo reconciliation. GSI supplies buffered snapshots, never an ordered or complete event log. It may raise a candidate from trusted health, weapon, and cumulative kill transitions, but it never finalizes a multikill, ace, clutch, or knife label.

## Decision fights

The broad policy clips every kill; the conservative policy clips only moments with a useful user-facing rule. Winner: candidates begin at three local kills in one eligible round, with ace, clutch, and knife labels assigned only by Demo. The broad policy saves immediately on every trigger; the safer policy waits for round end so the clip includes setup and outcome. Winner: auto saves queue at round end, while Manual Flag saves immediately. The broad policy makes each trigger a clip; the safer policy merges nearby triggers. Winner: one clip per merged highlight window.

## Rules and evidence

| Rule | Final Demo condition | Live provisional trigger |
|---|---|---|
| Multikill | Local player has at least 3 enemy kills in one eligible round. | Trusted cumulative kill increase reaches 3 in the current round, with a valid local identity. |
| Four-kill | Same as multikill with at least 4 kills. | Cumulative increase reaches 4. |
| Ace | Local player has 5 enemy kills in one eligible round. | Cumulative increase reaches 5, but display remains “Ace candidate, awaiting Demo confirmation.” |
| Clutch | Demo shows the local player became the sole living teammate against at least 1 opponent and their team won the round. `1v2+` is highlighted; `1v1` is retained as a clutch only when the configured minimum opponent count is met. | A trusted health/death transition suggests the local player is the last living teammate. Do not infer teammate identity from `observer_slot`. |
| Knife kill | Demo `player_death` identifies a knife weapon and the local player as attacker. | A weapon or kill transition can raise “possible knife candidate” only; GSI alone cannot prove weapon attribution. |

The v1 configured minimum for a named clutch is `1v2`; a `1v1` is an unlabelled close-round candidate. Team kills, suicides, world deaths, warmup, knife rounds, administrative resets, and unconfirmed rounds do not satisfy these rules.

## Timing, merging, and saving

Every auto candidate carries its first trusted transition, last transition, round identity, snapshot receipts, and a Demo-confirmation status. Merge candidates in the same round when their event windows overlap or their starts are no more than 10 seconds apart. The merged window starts at the earliest candidate start and ends at the latest candidate end.

Auto save policy uses 15 seconds of pre-roll and 10 seconds of post-roll around the merged window, implemented through the supervised gpu-screen-recorder Replay Buffer. The save is queued at round end so the round outcome and complete setup are present. If round end is not observed, keep the candidate provisional and do not silently save a final Auto Highlight.

Manual Flag is independent: the hotkey asks the recorder to save immediately with 15 seconds of pre-roll and 10 seconds of post-roll. A Manual Flag in an auto window merges into that pending save; otherwise it creates its own clip. Manual Flags are never relabelled as Auto Highlights without Demo evidence.

## Naming and metadata

Use deterministic names: `YYYYMMDD-HHMMSS_<map>_r<round>_<kind>_<short-id>`. `<kind>` is `candidate`, `multikill`, `ace`, `clutch`, `knife`, or `manual`; final labels replace `candidate` only after reconciliation. Clip metadata stores Match ID, round, start/end monotonic times, recorder save request ID, Demo hash when available, rule version, and all contributing Receipts.

## Failure and duplicate behavior

Serialize recorder save requests. A recorder warm-up or degraded state makes the UI say **“Highlight detected, recording is not ready.”** A recorder exit, signal failure, invalid output path, or timeout leaves the candidate Receipt intact, marks the clip failed, and never creates a fake file or empty Clip. Restart the supervised child according to the recorder control policy, then permit later saves.

Deduplicate by Match ID, round, rule version, candidate interval, and recorder output hash. Repeated GSI snapshots cannot create another candidate. A reparse may add a final label or new Receipt but must not duplicate a Clip; a changed rule version creates a new explicit candidate calculation.

## Verification vectors

1. Three kills in one eligible round create one provisional multikill candidate and one round-end save.
2. Five kills create one merged candidate with final Demo ace label, not five clips.
3. Kills at 9 seconds and 18 seconds merge; kills at 19 seconds and 30 seconds merge only when the configured windows overlap; a later round never merges.
4. A suspected last-survivor state creates a clutch candidate; Demo confirms `1v2` win, rejects a loss, and leaves `1v1` unnamed.
5. A knife-like GSI transition remains provisional; Demo weapon data confirms or rejects the knife label.
6. Manual Flag during an auto window yields one merged recorder save; Manual Flag outside it yields one immediate save.
7. Round-end absence, recorder warm-up, recorder crash, invalid path, and signal timeout each retain evidence and show a visible failed or provisional state.
8. Duplicate snapshots, duplicate save acknowledgements, and a parser rerun produce no duplicate Clip.
9. A 2k round, team kill, suicide, warmup, knife round, and administrative reset produce no qualifying final label.
