# Personal baselines and goals v1

This decision defines how openfrag interprets the local player's own `ofr-1.0.0` history. It does not alter any Match Rating, component, eligibility rule, or Receipt defined by the [resolved Rating specification](https://github.com/NtrpyDev/openfrag/blob/c167c7bf800d05df0d541dd438f530cbd649c6b1/docs/openfrag-rating.md). It creates no percentile, player rank, opponent adjustment, or second aggregate score.

All comparisons are descriptive statements about the local player's recorded Matches. They are not estimates of skill, predictions, or causal claims.

## Decisions settled by opposition

Each row gives the strongest case against the adopted policy before naming the winner.

| Decision | Strongest case for an alternative | Winner | Why the winner survives the objection |
|---|---|---|---|
| Reference population | Population percentiles make a number easy to interpret. | Personal history only. | openfrag has no representative, consented population or honest role adjustment. A percentile would claim evidence the product does not have. |
| Baseline shape | A fixed first-season baseline never moves, while exponential decay uses all history and adapts smoothly. | The latest 20 prior comparable Matches. | A fixed cohort becomes stale and depends on an arbitrary start. Decay is harder to audit and gives every old Match a hidden residual weight. A bounded window is legible and adapts without changing old Match results. |
| Baseline center | The arithmetic mean uses every observation and is familiar. | Exact median. | One overtime extreme, exceptional Match, or valid unusual result can move a mean sharply. The median keeps every Match while limiting one Match's leverage. |
| Variation | Standard deviation and a z-score provide familiar thresholds. | Median absolute deviation, with no z-score. | Standard deviation is outlier-sensitive and a z-score would look like a new normalized score. MAD provides a native-unit descriptive range. |
| Minimum history | Show a comparison after the first prior Match so the feature never looks empty. | Require 10 prior comparable Matches. | Fewer Matches invite a confident trend story from a few results. Ten is still a descriptive product threshold, not a statistical guarantee, and the exact count remains visible. |
| Full window | Require all 20 Matches before showing anything. | Show a limited baseline at 10 through 19; call 20 established. | Waiting for 20 delays useful self-comparison too far. Explicit limited-history language preserves the evidence boundary. |
| Current Match | Include the current Match in its own baseline so the line updates immediately. | Prior Matches only. | Self-inclusion shrinks the displayed difference and lets the observation move its own reference. |
| Session baseline | Recalculate the baseline after every Match tonight so it stays current. | Freeze it immediately before the session's first Match. | A moving reference can reverse the night's story while the session is in progress. A frozen cohort makes every session comparison share one Receipt-backed reference. |
| Session boundary | Split at local midnight because users understand calendar dates. | Split after a gap greater than 90 minutes. | Midnight can bisect one play session, daylight-saving changes civil time, and an evening label is not evidence. UTC Match intervals plus a gap rule follow actual play. |
| Map adjustment | Correct the Rating for the map before comparing it. | Keep the primary baseline map-agnostic; offer a separate same-map lens. | There is no supported map difficulty model. A separate cohort exposes context without rewriting `ofr-1.0.0`. |
| Side adjustment | Combine T and CT diagnostics into an adjusted Match score. | Show component-only same-side lenses, never a side Rating. | The Rating contract makes side vectors diagnostic. Recombining them would introduce a second aggregate and imply an unsupported adjustment. |
| Small-context cohorts | Show same-map or same-side medians as soon as two examples exist. | Apply the same 10-Match minimum and 20-Match cap. | Fragmented cohorts are especially easy to overread. Consistent thresholds are simpler and more defensible. |
| Confidence | Bootstrap a confidence interval or label confidence high, medium, or low. | Show exact sample counts and fixed descriptive language only. | Match results are serially dependent and selected by when the player plays. A probability label would need assumptions v1 cannot defend. |
| Valid extremes | Drop values outside the recent range as outliers. | Retain every trend-eligible Match. | A rare ace, bad loss, or overtime Match can be real. Median and MAD limit leverage without deleting valid evidence. |
| Manual exclusions | Let the player remove Matches affected by fatigue, experimentation, or bad teammates. | Allow annotations, not analytic exclusions. | Subjective removal enables hindsight selection and makes the comparison irreproducible. Only Rating-spec evidence rules exclude a Match. |
| Preview and disrupted Matches | Include partial Matches with lower weight to preserve more data. | Show them in the timeline but exclude them from baselines, session aggregates, and goals. | No evidence-backed weight makes a Preview comparable to a rated Match. The Rating spec already defines their trend exclusion. |
| Goal target | Recommend a target from peer percentiles or an opaque model. | The player chooses one native Rating or component threshold. | A self-chosen native value preserves agency and does not invent an external standard. |
| Goal evaluation | Mark success on any single Match that reaches the target. | Evaluate the median of the next 10 qualifying Matches. | A one-Match peak rewards volatility and cherry-picking. A fixed future block is finite, auditable, and aligned with the minimum baseline. |
| Parser upgrade | Mix old and new parser results when their displayed Ratings match. | Rebuild a coherent parser series and never mix calculation identities. | Equal two-decimal displays can hide component or Receipt differences. Reprocessing preserves auditability. |
| Formula or metric change | Carry the old baseline and goals forward because the scale still spans 0 to 2. | Start a new series and require a new goal confirmation. | The same number can have different meaning under a changed formula. Silent continuity would be false. |

## Terms and identities

A **trend-eligible Match** is a Match with all of these properties:

- its canonical `ofr-1.0.0` result is rated, not unavailable or Preview;
- the Rating specification does not exclude it from trends for a missed live round, surrender, roster imbalance, or evidence failure;
- it belongs to the local player;
- it has one selected canonical calculation run in the active calculation series;
- its Demo hash has not already supplied another Match in the cohort;
- it has Receipt-backed UTC Match start and end instants, with start no later than end.

Import time, file creation time, and file modification time are not Match chronology. A Match without trustworthy chronology remains visible but says **"Match time unavailable; excluded from ordered personal history."** Do not guess its position.

A **calculation series** has this identity:

```text
local_player_id
formula_version
metric_definition_version
parser_build
generated_proto_build
evidence_semantics_epoch
```

The game build remains on every Match Receipt. A routine game build change does not split a series. A reviewed game update that changes an input's meaning starts a new `evidence_semantics_epoch`.

A **comparable Match** is a trend-eligible Match in the active calculation series that satisfies the requested scope. The primary scope is all maps. A map scope additionally requires the same canonical map identifier. A side scope uses one complete T-side or CT-side diagnostic component vector from each comparable Match; it never has an aggregate Rating.

## Baseline algorithm

For a Match at time `t`, select comparable Matches that ended before that Match started. Order them by `(match_end_utc, demo_hash)` descending and take at most 20. The Demo hash is only a deterministic tie-breaker. It does not make an untrusted timestamp trustworthy.

For an active session, replace `t` with the first Match's start instant. Every card in that session uses the same frozen cohort. Matches from the current session do not enter its baseline, even after they finish.

Let the selected exact values in ascending order be `x[1]` through `x[n]`. Rating values use stored `rating_bp`, never the displayed centi value. Component values use their canonical exact rational representation.

```text
median(x) = x[(n + 1) / 2]                         when n is odd
median(x) = (x[n / 2] + x[n / 2 + 1]) / 2         when n is even
MAD(x)    = median(abs(x[i] - median(x)))
range     = [median(x) - 2 * MAD(x),
             median(x) + 2 * MAD(x)]
```

Clamp the descriptive range to `[0, 20000]` basis points for Rating and `[0, 1]` for a component. Keep all intermediate values exact. Render a nonnegative Rating center with the Rating specification's half-up rule. For an exact signed difference `d_bp`, render two decimal places with `sign(d_bp) * floor((abs(d_bp) + 50) / 100)` centi-points, using exact rational division. This rounds a tie away from zero and avoids an asymmetric negative display. Never calculate a later value from rendered text.

The range is a description of recent personal variation, not a confidence interval. When `MAD = 0`, show the center and **"Recent values have no measured spread; range label unavailable."** Do not call a later nonzero difference statistically significant.

For `MAD > 0`, an observed value or session median is:

- **above recent range** only when it is strictly greater than the upper bound;
- **within recent range** when it is inside the inclusive bounds;
- **below recent range** only when it is strictly less than the lower bound.

Also show the signed native-unit difference from the baseline median. Do not divide by MAD, combine component differences, award points, assign colors that imply a rank, or call a positive difference improvement without the qualifier **"recorded."**

The Rating baseline is the median of canonical Match Ratings. Component baselines are independent medians of canonical Match component values. Never apply the `ofr-1.0.0` weights to the component medians or their differences. The Rating of a median component vector would be a new score and is forbidden.

## Minimum history and exact language

The sample count always means the number of selected comparable Matches after exclusions and the 20-Match cap, so it is never greater than 20. The UI may show a separate lifetime count, but that count does not change the baseline statistics or sample language.

| Selected history | Behavior | Required text |
|---|---|---|
| `0` through `9` | Withhold center, MAD, range, and directional comparison. Show recent Matches and exclusion reasons. | **"Personal baseline not ready: N of 10 comparable Matches."** |
| `10` through `19` | Show median, MAD, native difference, and range label when MAD is nonzero. | **"Limited personal history (N Matches). Descriptive only; direction may change."** |
| `20` | Show the same statistics. | **"Recent personal history (last 20 Matches). Descriptive, not predictive."** |

Do not use `confident`, `significant`, `average player`, `expected`, `top`, `bottom`, or percentile language. A larger sample does not turn this local window into a population claim.

## Tonight and session comparisons

`Tonight` is the product's active-session card, not a civil-date bucket. Sort trustworthy Match intervals by UTC start time. A Match joins the preceding session when its start is after the preceding Match's end and the gap is at most 90 minutes. A gap greater than 90 minutes starts a new session. Do not split at midnight or a timezone change. Overlapping, reversed, or missing intervals cannot be guessed into a session and remain outside session aggregates with a visible chronology reason.

The Tonight card contains:

- each Match's immutable Rating, component vector, map, side diagnostics, status, and exclusion reason;
- the median Rating and independent component medians among this session's trend-eligible Matches;
- signed differences between those session medians and the frozen baseline medians;
- the frozen baseline cohort count and Receipt link;
- Match result and round differential as context only.

Trend-ineligible Matches remain in the chronological list but do not enter any session median. If the frozen baseline has fewer than 10 Matches, show the session values without a baseline difference or direction.

Use this exact session language:

| Qualifying Matches tonight | Required text |
|---|---|
| `0` | **"No comparable Match tonight."** |
| `1` | **"One comparable Match tonight; no session pattern."** |
| `2` through `4` | **"Small session (N Matches); direction is provisional."** |
| `5` or more | **"Session summary (N Matches); descriptive, not predictive."** |

Closing a session does not rewrite its card. Future sessions may use its eligible Matches when constructing their own prior-only cohorts.

## Map and side context

The primary personal baseline always uses all maps. Map and side views are separate lenses with their own cohort Receipts.

### Same-map lens

For the current canonical map, select the last 20 prior comparable Matches on that map. Require 10 before showing statistics. Show the canonical Rating median and independent component medians under the same algorithm and language as the primary baseline. Label every value **"Same-map personal history"**. Do not use it to adjust, replace, or blend with the primary comparison.

### Same-side lens

For T and CT separately, select complete diagnostic side component vectors from the last 20 prior comparable Matches across all maps. Require 10 vectors for that side. Compare component to matching component only. Show raw denominators and opportunity counts beside opening and clutch values. Do not calculate a T Rating, CT Rating, combined side Rating, map-side cohort, or hidden side correction in v1.

A missing or incomplete side vector suppresses only that side lens for the Match. It does not change the canonical Match Rating or the all-map baseline. The UI may show map and side labels at every sample size, but it must not show a contextual median before the relevant 10-Match threshold.

## Goals

A v1 goal is a prospective test of one value, not a prediction or contract with the player.

The player may create a goal only when its chosen scope already has a ready baseline. The goal record fixes:

```text
goal_id
created_at_utc
calculation_series_identity
field = canonical Rating or one named canonical component
scope = all maps or one canonical map
baseline cohort Demo hashes
baseline exact median and MAD
target exact value
evaluation size = 10 qualifying future Matches
status
```

Rating targets are stored in basis points from `0` through `20000`. Component targets are stored as exact values from `0` through `1`. The UI may show the recent center and let the player enter an absolute target, but it must not recommend a target from other players, infer a target from a percentile, or silently choose a target.

The evaluation cohort is the first 10 trend-eligible Matches after creation that match the fixed scope and calculation series. A Match either qualifies in full or does not enter the cohort. There are no weights, replacement Matches, best-of selection, deadline extension, or manual removal.

Before 10 qualifying Matches, show the current exact median and **"Goal in progress: N of 10 qualifying Matches; outcome not decided."** At 10, mark **Achieved** when the evaluation median is at least the target, otherwise **Not achieved**. Retain the goal, cohort, exclusions, and final Receipt. Cancelling a goal changes it to **Cancelled** but does not delete its record.

Opening and clutch component goals must display their total opportunity denominator for the evaluation cohort because a neutral `0.5` can represent no opportunity. The goal still evaluates the canonical component values; the UI must not translate it into a claim about opening or clutch skill.

Side goals, combined-component goals, weighted goal progress, streak goals, and goals based on Preview Matches are outside v1. A new goal may reuse an old target only as an explicit player choice.

## Valid extremes, partial Matches, and exclusions

No valid high or low Match is automatically excluded. Values outside the descriptive range remain in the ordered cohort and its Receipt. Overtime remains part of the canonical whole-Match value. A win, loss, map, opponent, party state, role claim, fatigue note, or user annotation cannot change inclusion.

The following remain visible but do not enter baselines, contextual cohorts, session aggregates, or goals:

- unavailable Ratings;
- Ratings with fewer than 12 eligible rounds, including 8 through 11 round Previews;
- missed-live-round Previews;
- surrender Previews;
- roster-imbalanced Matches;
- Matches excluded from trends by a future compatible Rating-spec evidence rule;
- duplicate Demo hashes after the selected canonical Match;
- Matches without trustworthy chronology;
- calculation runs outside the active coherent series.

Every exclusion uses the Rating Receipt's machine-readable reason when one exists. Baseline logic must not invent a softer weight or substitute zero. A user annotation is displayed beside the Match and has no analytic effect.

## Parser, formula, and evidence resets

Calculation runs remain append-only. Selecting a new canonical run never deletes the old run or its earlier trend Receipt.

### Parser or generated-protobuf build

A promoted parser or schema build creates a new calculation series. Reparse the retained corpus, compare component and Receipt diffs, and construct baselines solely from selected runs in the new series. Never fill a short new-series cohort with old parser runs, even when the two-decimal Ratings match.

An active goal may be rebound to the rebuilt parser series only when all of these are true:

- formula version, metric-definition version, and evidence semantics are unchanged;
- every Match in its baseline and evaluation-to-date has a promoted new run;
- eligibility, chronology, exact Rating, exact component vector, and goal scope membership are unchanged;
- the rebind Receipt lists every old and new calculation identity;
- the player sees **"Parser updated; goal evidence reproduced without value changes."**

If any condition fails, pause the goal as **Version changed**. Do not replace changed Matches or continue counting until the player explicitly starts a goal in the new series.

### Formula, metric definition, or evidence semantics

A formula-version, metric-definition, or `evidence_semantics_epoch` change is a hard interpretation reset. Recompute the retained corpus into a separate coherent series when possible. The new series becomes baseline-ready only when it independently reaches 10 comparable Matches. Archive old trend cards under their original version.

Goals in the old series end as **Version changed**. Never migrate their target or achieved status automatically, even if the numeric scale is unchanged. The player must confirm a new goal against the new series.

A routine game build change with reviewed unchanged input semantics does not reset the series. Record the build on each Match. If review later finds a semantic change, create a new evidence epoch rather than silently editing the old one.

## Receipt contract

Every rendered baseline, context lens, Tonight comparison, and goal state links to a Receipt containing:

- policy version `personal-baseline-1`;
- local player identity reference and full calculation series identity;
- comparison scope and as-of or frozen session instant;
- deterministic Match ordering key;
- ordered included Match IDs, Demo hashes, exact values, and calculation-run identities;
- considered but excluded Matches with one reason each;
- window cap and minimum-history threshold;
- exact median, MAD, clamped range, and rendered values;
- session boundaries and qualifying session Match IDs when applicable;
- goal target, fixed baseline cohort, evaluation cohort to date, status, and version transitions when applicable.

The Receipt must reproduce the card byte-for-byte from the same selected calculation runs and policy version. It proves which recorded values were compared and how. It does not prove improvement, skill, effort, or causality.

## Worked examples

### Established baseline and a small session

Suppose the 20 prior comparable Ratings, already sorted, are:

```text
0.88, 0.91, 0.94, 0.96, 0.98,
0.99, 1.00, 1.01, 1.02, 1.04,
1.04, 1.05, 1.06, 1.08, 1.09,
1.10, 1.12, 1.15, 1.20, 1.62
```

The valid `1.62` remains in the cohort. The exact median is `1.04`, MAD is `0.055`, and the descriptive range is `[0.93, 1.15]`. Three qualifying Ratings tonight are `1.10`, `1.18`, and `1.22`; their median is `1.18`, which is `+0.14` from the frozen center and above the recent range.

The card must still say **"Small session (3 Matches); direction is provisional."** It may say **"Recorded session Rating is above the recent personal range."** It may not say the player became better, ranks highly, or is likely to sustain the result.

### Partial Match

The same session also contains a 10-eligible-round Preview Rating of `1.30`. Show it in the Match list with its Preview label and exclusion Receipt. The session median remains `1.18`; do not include `1.30` with a fractional weight.

### Goal

With a ready baseline median of `1.04`, the player chooses a Rating target of `1.10`. The first 10 qualifying future Ratings have an exact median of `1.105`. The goal is **Achieved** because `1.105 >= 1.10`. The decision uses stored basis points and exact median arithmetic, not the two-decimal rendering.

## Invariants

- No baseline, session, context, or goal operation changes a canonical Match Rating, component, Receipt, or eligibility result.
- No value from another player enters any calculation.
- A comparison uses only Matches before its cutoff; a session uses only Matches before the session cutoff for its frozen baseline.
- The selected primary cohort contains at most 20 unique Demo hashes and at least 10 before statistics appear.
- Future Matches cannot change a closed Match or session comparison Receipt.
- Reordering imports cannot change chronological cohorts.
- A valid extreme remains included; an evidence exclusion never becomes zero or a fractional weight.
- The median Rating is computed from exact `rating_bp`, never displayed centi values.
- Component medians are never weighted or recombined into a Rating.
- Map and side context never adjusts the primary all-map comparison.
- A side lens contains no aggregate Rating.
- A goal evaluates exactly the first 10 qualifying future Matches in its fixed scope.
- Calculation identities from different series never appear in one cohort.
- Every exclusion and version transition is visible and Receipt-backed.
- No UI text describes a percentile, player rank, statistical significance, prediction, or causal improvement.

## Verification requirements

Implementation is not ready until deterministic fixtures prove all of these:

1. Nine prior eligible Matches withhold the baseline; the tenth enables it with limited-history language; the twentieth changes the language to recent personal history.
2. A twenty-first prior Match evicts exactly the oldest comparable Match, not the oldest imported file.
3. Odd and even exact medians, MAD, clamping, half-basis-point centers, and half-up rendering match golden rational cases.
4. A valid extreme remains included and changes only the statistics permitted by median and MAD.
5. `MAD = 0` suppresses the range label and never divides by zero or emits significance language.
6. A current Match is absent from its own baseline, and every Match in one session uses the identical frozen cohort.
7. A 90-minute gap remains one session, a 90-minute-and-one-second gap starts another, and crossing local midnight alone does not split a session.
8. Missing, reversed, and overlapping Match intervals produce the visible chronology exclusion instead of guessed ordering.
9. Preview, surrender, roster-imbalanced, unavailable, duplicate-Demo, and missing-chronology fixtures remain visible but never enter any aggregate or goal.
10. A valid overtime Match and valid Ratings above or below the descriptive range remain included.
11. Same-map statistics require 10 prior Matches on that map and never replace the primary all-map baseline.
12. T and CT component lenses each require 10 complete prior vectors, expose opportunity denominators, and never emit a side Rating.
13. Tonight cohorts with zero, one, three, and five qualifying Matches use the exact required sample language.
14. A goal remains undecided for its first nine qualifying Matches and resolves from the exact median of the tenth; ineligible and out-of-scope Matches do not consume a slot.
15. Cancelling a goal retains its evidence, and no manual Match exclusion changes a completed or in-progress cohort.
16. A parser upgrade with a fully identical rebuilt corpus can rebind a goal with a rebind Receipt; any exact value, eligibility, chronology, or coverage change pauses it.
17. Formula, metric-definition, and evidence-epoch changes start separate histories and end old goals as Version changed.
18. Repeated calculation from the same selected runs and policy version produces byte-identical Receipts and rendered cards.
19. Property tests confirm that adding future Matches cannot change an earlier comparison and that no cohort mixes calculation series.
20. UI snapshot tests reject percentile, rank, significance, prediction, unqualified improvement, and high/medium/low confidence claims.
