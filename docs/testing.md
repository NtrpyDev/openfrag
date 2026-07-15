# Whole-app verification

OpenFrag has one machine-checked test graph for local development, continuous integration, package channels, and release staging. The graph covers every v1 product surface listed in `tests/verification/coverage.json`. Validation fails if a required surface disappears, a row has no failure mode or verifier, a verifier is unknown, or an evidence file is missing.

## Commands

Install the pinned browser harness once:

```sh
npm ci --ignore-scripts --no-audit --no-fund
npx playwright install chromium
```

Run the fast developer feedback profile:

```sh
./scripts/verify-ci.sh fast
```

Run every v1 verifier, including browser, release reproducibility, and clean AUR and Fedora package environments:

```sh
./scripts/verify-ci.sh
```

The complete profile requires Docker. Clean package containers need distribution package repositories to install their build and runtime dependencies. They always build the exact deterministic source archive prepared by the suite and never fetch OpenFrag source from the network.

## Execution graph

```text
coverage contract
      |
format, metadata, and first-party clippy
      |
shared Rust test graph, daemon binaries, and source archive
      |
      +-- Rust domain tests
      +-- Rust runtime tests
      +-- Rust daemon tests
      +-- parser, schema, fuzz-build, and NVIDIA fixture contracts
      +-- static install and package identity
      +-- release and source reproducibility
      +-- production headless journey
      +-- installed acceptance journey
      +-- real Chromium dashboard journey
      +-- clean AUR build and lifecycle --------+
      +-- clean Fedora 43 build and lifecycle --+ maximum two package builds
      +-- clean Fedora 44 build and lifecycle --+
```

Preparations are serialized because every later shard consumes their exact outputs. Independent verifier shards run concurrently with a default limit of four. Native package builds have a separate limit of two because each clean container compiles the vendored Rust graph and installs a full platform dependency set. No other shard is serialized.

Every shard receives its own HOME, temporary directory, XDG directories, deterministic loopback port, and log. Cargo uses one shared target directory. The production and acceptance-fixture daemons are copied to immutable per-run artifact paths before concurrent execution. Browser traffic outside the daemon's loopback origin is blocked and fails the test.

## Shards

| Shard | Main proof | Profile |
| --- | --- | --- |
| `rust-domain` | GSI, Demo parsing, analysis, Rating, setup, pipeline, and storage | Fast and complete |
| `rust-runtime` | capture, Clip workflow, live coordination, and Global Shortcuts | Fast and complete |
| `rust-daemon` | CLI, API, dashboard contract, adapters, and service behavior | Fast and complete |
| `e2e-headless` | production daemon journey through process and loopback HTTP seams | Fast and complete |
| `parser-contracts` | schema, compatibility manifest, fuzz target builds, script syntax, and NVIDIA fixtures | Complete |
| `package-static` | desktop and service identity, static lifecycle, deterministic archive, and native recipe contract | Complete |
| `release-contracts` | byte-identical vendored source inputs | Complete |
| `e2e-installed` | installed static package from local Demo to Rating, Receipts, Manual Flag, Clip trim, preview, and export | Complete |
| `browser-dashboard` | real Chromium navigation, accessible status, honest failure, and local-only requests | Complete |
| `package-aur-clean` | clean Arch source build, namcap, install, reinstall, smoke, removal, and state preservation | Complete |
| `package-copr-43-clean` | clean Fedora 43 source build, rpmlint, install, reinstall, smoke, removal, and state preservation | Complete |
| `package-copr-44-clean` | clean Fedora 44 source build, rpmlint, install, reinstall, smoke, removal, and state preservation | Complete |

## Timing and budgets

The previous serial `scripts/verify-ci.sh` baseline on PC1 was 180.53 seconds from an empty Cargo target and 22.46 seconds warm. An earlier cold attempt failed after 159.57 seconds when duplicated build and source work exhausted the 16 GB temporary filesystem. With a fresh target, the optimized fast profile completed in 45.32 seconds cold and 3.58 seconds warm on the same host.

Two consecutive complete runs passed in 787.16 and 797.85 seconds. Their observed critical path was serialized preparation followed by the bounded native-package queue: AUR and one Fedora build occupied both package slots, then the second Fedora build used the first available slot. The first run's slowest individual phases were Fedora 44 at 502.16 seconds, Fedora 43 at 499.79 seconds, AUR at 269.91 seconds, shared build preparation at 11.28 seconds, and release reproducibility at 11.10 seconds.

| Profile | Local budget | CI budget | Purpose |
| --- | ---: | ---: | --- |
| Fast | 120 seconds | 180 seconds | frequent developer feedback without browser, package, or release environments |
| Complete | 1,200 seconds | 1,500 seconds | every v1 verifier, including three clean native package builds |

The runner records wall time and every phase duration in `target/openfrag-suite/runs/<run>/summary.json`, prints the observed critical path and five slowest phases, and fails when the applicable budget is exceeded. Local budgets apply by default. `CI=true` selects CI budgets.

## Failure behavior

Parallel verification does not fail fast. Every independent shard is allowed to finish so one failure cannot hide another. Full combined output is retained in one log per phase and printed for every failed phase. A run cannot pass with a missing, cancelled, timed-out, failed, or over-budget shard. There are no retries, ignored results, or assertion reductions.

## Physical qualification boundary

The automated suite proves every observable software contract using public interfaces and deterministic fixtures. Release qualification that requires a real CS2 session, user-approved desktop portal, screen capture, microphone or audio selection, playback, or human observation remains in the `Publish and qualify v1.0.0 on PC1 and PC2` workstream. Those gates cannot be honestly replaced by mocks or unattended SSH checks.
