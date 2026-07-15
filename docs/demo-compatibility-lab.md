# Demo compatibility lab

The compatibility lab protects openfrag's normalized Demo evidence from parser, schema, and threading regressions without committing private Demo bytes to the repository.

## Corpus manifest

[`benchmarks/parser-fixtures.json`](../benchmarks/parser-fixtures.json) is the versioned fixture manifest. Local Demo fixtures are addressed by SHA-256 and record provenance, CS2 build cohort, map, mode, edge-case tags, golden counts, normalized-evidence hashes, receipt-hash aggregates, and expected threading behavior. The manifest explicitly marks the Demo bytes as non-redistributable.

Store authorized local corpus files as `<sha256>.dem` in a directory outside the repository. Check the committed manifest without private files:

```console
cargo run -p openfrag-import --features demoparser --bin openfrag-compat-lab -- \
  check-manifest benchmarks/parser-fixtures.json
```

Run the complete golden verification when the local corpus is available:

```console
cargo run --release -p openfrag-import --features demoparser --bin openfrag-compat-lab -- \
  verify benchmarks/parser-fixtures.json /path/to/corpus
```

Use `inspect <demo>` to produce candidate golden values for an authorized fixture. Review provenance and coverage before editing the manifest.

## Current coverage

The initial local corpus covers four Valve matchmaking Demos, CS2 builds 13984 and 14169, and maps Ancient, Cache, and Mirage. It includes same-tick normalized events. Every fixture is parsed twice by the canonical single-threaded path and twice by the experimental parallel path. The observed parallel results are deterministic and evidence-equivalent to canonical results.

Three repository-owned synthetic cases cover a truncated frame, an unsupported command, and a short header. The manifest's `coverage_gaps` field lists missing maps, modes, sources, and match edge cases. Those gaps are explicit so that a passing lab run cannot be mistaken for exhaustive Demo compatibility.

## Schema drift

The normalized event schema is generated from production definitions:

```console
cargo run -p openfrag-import --bin openfrag-schema -- \
  --check schema/openfrag-demo-evidence-1.schema.json
```

An intentional normalized-schema change requires regenerating the schema with `--write`, reviewing it, and updating the manifest schema hash and affected golden fixture hashes in the same change.

## Fuzzing

The raw decoder fuzz target accepts at most 16 MiB per input. The normalizer target accepts at most 64 KiB of encoded fields. Each libFuzzer run uses a 10-second per-input timeout and a 2 GiB RSS limit. CI compiles both targets:

```console
scripts/fuzz-demo-parser.sh check
```

Run bounded local fuzz sessions with cargo-fuzz installed:

```console
OPENFRAG_FUZZ_SECONDS=60 scripts/fuzz-demo-parser.sh
```

Increase the session duration for dedicated fuzz campaigns, but keep the committed per-input length, timeout, and memory limits aligned with the manifest.
