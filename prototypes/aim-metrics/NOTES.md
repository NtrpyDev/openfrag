# Prototype answer

The bundled `de_mirage` fixture produced 1,590 `weapon_fire` events, 264 `player_hurt` events, 205 enemy-firearm hurt events, and no `fire_bullets` or `player_bullet_hit` events through pinned demoparser commit `ba39cc44cd5abfd7f34df2b3c0a7dd3630048311`.

The hurt-tick horizontal pawn-bearing proxy was calculable for all 205 firearm hurts. Its median absolute error was `0.790811°`, p90 was `2.448077°`, p95 was `3.557081°`, and maximum was `17.018341°`. These values are conditioned on successful damage and have no validated relationship or finite error bound to pre-aim, first visibility, hitbox intersection, or the rendered crosshair. The honest v1 product decision is to withhold crosshair placement. If retained for debugging, label it “Hurt-tick horizontal pawn-bearing error: diagnostic, hits only.”

Of 205 firearm hurts, 203 had exactly one same-attacker, canonical-weapon `weapon_fire` at the same Demo tick. Every paired offset was zero ticks. Two were unmatched, and one fire event corresponded to two hurts. This establishes same-tick co-occurrence, not elapsed reaction time: within-tick order, a validated tick duration, first visibility, and exposure onset are absent. The honest v1 decision is to withhold time-to-damage. If retained for debugging, label it “Same-tick shot/hurt co-occurrence: diagnostic, not reaction time.”

The two per-shot custom streams needed to evaluate misses and impacts were empty in this representative fixture. A ratio built from `weapon_fire` and `player_hurt` would conflate damage outcome with accuracy and cannot reconstruct bullet paths, misses, pellets, penetration, spread, or recoil correction. The honest v1 decision is to withhold spray accuracy until a legally usable fixture exposes both custom streams and pairing invariants pass.

## Human reaction needed

Run `node prototypes/aim-metrics/run.js`, inspect pages 2 through 4, and answer one exact question: **Is it acceptable for v1 to show no aim score and keep the two observed proxies in a developer-only diagnostic view?**

If the answer is no, the requested follow-up evidence must be a consented or redistributable Demo with non-empty `fire_bullets` and `player_bullet_hit`, plus versioned map collision, hitbox, and visibility inputs. Renaming the present proxies is not enough.
