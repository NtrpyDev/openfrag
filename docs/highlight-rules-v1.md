# Highlight rules v1

Auto Highlights are provisional while a match is live and final only after Demo reconciliation. GSI supplies buffered snapshots, never an ordered or complete event log. It may raise a candidate from trusted health, weapon, and cumulative kill transitions, but it never finalizes a multikill, ace, clutch, or knife label.

## Decision fights

The broad policy clips every kill; the conservative policy clips only moments with a useful user-facing rule. Winner: candidates begin at three local kills in one eligible round, with ace, clutch, and knife labels assigned only by Demo. The broad policy saves immediately on every trigger; the safer policy saves a provisional replay at every observed official round end, then filters it after Demo import. Winner: round-end provisional capture preserves clutch and single-knife coverage because trusted GSI has no teammate state or reliable knife attribution. The broad policy makes each trigger a clip; the safer policy merges nearby triggers. Winner: one clip per merged highlight window.

## Rules and evidence

| Rule | Final Demo condition | Live provisional trigger |
|---|---|---|
| Multikill | Local player has at least 3 enemy kills in one eligible round. | Trusted cumulative kill increase reaches 3 in the current round, with a valid local identity. |
| Four-kill | Same as multikill with at least 4 kills. | Cumulative increase reaches 4. |
| Ace | Local player has 5 enemy kills in one eligible round. | Cumulative increase reaches 5, but display remains “Ace candidate, awaiting Demo confirmation.” |
| Clutch | Demo shows the local player became the sole living teammate against at least 1 opponent and their team won the round. `1v2+` is highlighted; `1v1` is retained as a clutch only when the configured minimum opponent count is met. | No automatic clutch-specific trigger. Capture the observed official round-end provisional replay and let Demo filtering decide. Do not infer teammate identity from `observer_slot`. |
| Knife kill | Demo `player_death` identifies a knife weapon and the local player as attacker. | A weapon or kill transition can raise “possible knife candidate” only; GSI alone cannot prove weapon attribution. |

The v1 configured minimum for a named clutch is `1v2`; a `1v1` is an unlabelled close-round candidate. Team kills, suicides, world deaths, and unconfirmed rounds do not satisfy these rules. No knife-round or administrative exclusion is applied without an explicit Demo evidence predicate.

## Timing, merging, and saving

Every auto candidate carries its first trusted transition, last transition, round identity, snapshot receipts, a Capture Session ID, and a Demo-confirmation status. It has no Match ID before Demo association. An observed official round end creates a provisional round-capture candidate even when no kill trigger fired. Merge candidates in the same round when their event windows overlap or their starts are no more than 10 seconds apart. The merged window starts at the earliest candidate start and ends at the latest candidate end. Keep provisional round captures until Demo import or 30 days, subject to a 2 GiB byte budget per Capture Session; evict oldest unconfirmed media first while preserving candidate metadata and Receipts. Expired items show **“Demo not imported; provisional replay expired.”**

The replay buffer is 60 seconds, adopting the recorder decision's coverage over the smaller 30-second alternative. Auto save is delayed 10 seconds after observed round end, then signals gpu-screen-recorder once. The desired final trim is from `candidate_start - 15s` through `round_end + 10s`; the raw save ends at signal time and includes no future beyond that point. At signal time, up to 50 seconds before round end is available, so older candidates are honestly trimmed to the earliest available timestamp and marked `pre_roll_truncated`; never claim coverage outside the raw file. If round end is not observed, retain the candidate or round capture but do not silently save a final Auto Highlight.

Manual Flag is independent and immediate: the hotkey signals the recorder at flag time, producing a raw replay ending at the signal with up to 60 seconds of preceding buffer. The desired Manual Flag trim is `flag_time - 15s` through `flag_time`; there is no future post-roll because the save is immediate. It cannot merge into a future auto save before that save exists. When the later auto save is produced, consolidate overlapping raw captures only if both files exist and together cover the desired union; otherwise retain separate Clips. Manual Flags are never relabelled as Auto Highlights without Demo evidence.

## Naming and metadata

Use deterministic names: `YYYYMMDD-HHMMSS_<map>_r<round>_<kind>_<capture-session-id>_<short-id>`. `<kind>` is `candidate`, `multikill`, `ace`, `clutch`, `knife`, or `manual`; final labels replace `candidate` only after reconciliation. Clip metadata stores Capture Session ID, observed map/round, Match ID and Demo hash when associated, start/end monotonic times, recorder save request ID, rule version, and all contributing Receipts.

## Failure and duplicate behavior

Serialize recorder save requests. A recorder warm-up or degraded state makes the UI say **“Highlight detected, recording is not ready.”** A recorder exit, signal failure, invalid output path, or timeout leaves the candidate Receipt intact, marks the clip failed, and never creates a fake file or empty Clip. Restart the supervised child according to the recorder control policy, then permit later saves.

Deduplicate pre-import candidates by Capture Session ID, observed map/round, rule version, candidate interval, and recorder output hash. After association, add Match ID and Demo hash to the identity. Repeated GSI snapshots cannot create another candidate. A reparse may add a final label or new Receipt but must not duplicate a Clip; a changed rule version creates a new explicit candidate calculation. Round-end provisional captures are storage-budgeted and expire after 30 days if no Demo arrives.

## Verification vectors

1. Three kills in one eligible round create one provisional multikill candidate and one round-end save.
2. Five kills create one merged candidate with final Demo ace label, not five clips.
3. Kills at 9 seconds and 18 seconds merge; kills at 19 seconds and 30 seconds merge only when the configured windows overlap; a later round never merges.
4. A zero-kill round-end capture remains provisional; Demo confirms or rejects a `1v2` win, and a bomb outcome does not create a clutch without the Demo alive-state predicate.
5. A single knife kill is covered by the round-end provisional capture; Demo weapon data confirms or rejects the knife label.
6. Manual Flag during an auto window saves immediately; the later auto save is produced independently, then overlapping raw files consolidate only when their union is fully covered. Manual Flag outside it yields one immediate save.
7. Missing round end retains a provisional candidate without a final Auto Highlight.
8. A delayed auto save at round-end plus 10 seconds produces the desired trim from a 60-second buffer, while a candidate older than the available 50 seconds records `pre_roll_truncated`.
9. A Manual Flag saves immediately with no future post-roll and trims only preceding media.
10. Recorder warm-up, recorder crash, invalid path, and signal timeout each retain evidence and show a visible failed or provisional state.
11. Duplicate snapshots, duplicate save acknowledgements, and a parser rerun produce no duplicate Clip; pre-import identity uses Capture Session ID until Match association.
12. Team kill, suicide, warmup, knife round, and administrative reset receive no final label unless their exact Demo predicates satisfy a named rule.
