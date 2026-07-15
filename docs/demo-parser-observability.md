# Demo parser observability and benchmark contract

OpenFrag exposes parser progress as phase-local evidence. `first_pass` and
`second_pass` report bytes consumed, total bytes, completed frames, and emitted
requested events at stable frame boundaries. `finalize` reports the completed
counter set after the exact OpenFrag query plan has been assembled. Counters
never decrease within a phase. The import pipeline maps those phases into a
monotonic overall range from 30% through 70%, persists the phase-local counters,
and throttles database writes to changes of at least 0.1 percentage points or a
phase transition.

The dashboard reports both the phase-local byte percentage and overall import
percentage. Frames and emitted events are supporting activity counters. They
are not estimates of remaining wall time.

## Fixed benchmark fixture

The fixture bytes are not redistributed by OpenFrag. A local fixture is accepted
only when its SHA-256 and expected canonical counts match
`benchmarks/parser-fixtures.json`. The current representative fixture is the
Premier Mirage Demo already used by the parser contract tests:

- SHA-256: `84a1a4191302bdd2a3bbb5a727842093744b1fb1a228aeec630369e44b622cb2`
- Participants: 10
- Rounds: 10
- Normalized events: 366
- Event-aligned player snapshots: 2,690
- Query-plan hash: `e13da974e6aba0271959c3d606491e89e6261abbeeaebbb79a5b2221f3d9f957`

Run the benchmark from the repository root:

```sh
scripts/benchmark-parser.sh /path/to/fixed.dem 5
```

The harness builds the release parser and measures the production, canonical,
single-threaded query plan. It records median in-process wall time, whole-process
wall time, user and system CPU time, peak resident memory, normalized-event
throughput, parser counters, query-plan identity, fixture identity, and repeated
output determinism.

Pass a prior result from the same runner as the fourth argument to enable the
regression gate. By default, wall time, CPU, and peak memory may not rise by more
than 20%, and normalized-event throughput may not fall by the corresponding
ratio. Small absolute noise floors apply to avoid alerts from timer precision,
scheduler jitter, and allocator variation. `OPENFRAG_BENCH_MAX_REGRESSION_RATIO`
can tune the relative alert threshold.
Results from unlike machines are retained as observations only. No benchmark
number is a product service-level promise.

The parser's parallel mode is not canonical and is not used by Demo import. It
must not be presented as an OpenFrag production benchmark until deterministic
normalized output has been independently verified.

## First baseline

`benchmarks/results/pc1-parser-baseline.txt` records the first canonical run on
the local Intel Core i7-12700K host with Rust 1.96.0. Across three release-mode
iterations it measured a 0.251116-second median in-process wall time, 1,461.799
normalized events per second, 0.71 seconds of user CPU, 0.03 seconds of system
CPU, and 77,472 KiB peak resident memory. Repeated canonical output was equal.
These values seed same-runner regression comparisons and are not performance
claims for other machines.
