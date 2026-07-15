#!/usr/bin/env python3
"""Run and validate OpenFrag's bounded whole-app verification shards."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
import argparse
from datetime import UTC, datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
from threading import Semaphore
import time
from typing import Any


REQUIRED_SURFACES = frozenset(
    {
        "aur_package",
        "capture_supervision",
        "clip_workflow",
        "copr_package",
        "daemon_cli",
        "dashboard",
        "desktop_service",
        "import_analysis",
        "licensing_privacy",
        "loopback_api",
        "manual_flag",
        "release_artifacts",
        "setup",
        "static_install",
        "steam_gsi",
        "storage_recovery",
        "upgrade_uninstall",
    }
)


class ConfigurationError(ValueError):
    """The suite manifest or coverage inventory is incomplete."""


@dataclass(frozen=True)
class PhaseResult:
    """Observable result for one preparation or verifier shard."""

    phase_id: str
    duration_seconds: float
    returncode: int
    log: Path


@dataclass(frozen=True)
class SuiteResult:
    """Aggregate result with enough detail to prove every shard finished."""

    profile: str
    succeeded: bool
    duration_seconds: float
    budget_seconds: int
    budget_exceeded: bool
    executed_shards: set[str]
    failed_shards: set[str]
    failed_preparations: set[str]
    missing_shards: set[str]
    logs: dict[str, Path]
    timings: dict[str, float]


def _mapping(value: Any, label: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ConfigurationError(f"{label} must be an object")
    return value


def _sequence(value: Any, label: str) -> Sequence[Any]:
    if isinstance(value, (str, bytes)) or not isinstance(value, Sequence):
        raise ConfigurationError(f"{label} must be an array")
    return value


def _nonempty_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ConfigurationError(f"{label} must be a nonempty string")
    return value


def _validate_phase(phase: Any, *, label: str, known_profiles: set[str]) -> str:
    item = _mapping(phase, label)
    phase_id = _nonempty_string(item.get("id"), f"{label}.id")
    profiles = {
        _nonempty_string(profile, f"{label}.profiles")
        for profile in _sequence(item.get("profiles"), f"{label}.profiles")
    }
    unknown_profiles = profiles - known_profiles
    if unknown_profiles:
        raise ConfigurationError(
            f"{phase_id} uses unknown profiles: {', '.join(sorted(unknown_profiles))}"
        )
    commands = _sequence(item.get("commands"), f"{label}.commands")
    if not commands:
        raise ConfigurationError(f"{phase_id} has no commands")
    for command_index, command in enumerate(commands):
        arguments = _sequence(command, f"{label}.commands[{command_index}]")
        if not arguments:
            raise ConfigurationError(f"{phase_id} has an empty command")
        for argument in arguments:
            _nonempty_string(argument, f"{phase_id} command argument")
    timeout = item.get("timeout_seconds")
    if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout <= 0:
        raise ConfigurationError(f"{phase_id} must have a positive timeout_seconds")
    return phase_id


def validate_configuration(
    manifest: Any,
    coverage: Any,
    *,
    repository: Path | None = None,
) -> None:
    """Reject an incomplete manifest or uncovered product contract."""
    manifest_object = _mapping(manifest, "manifest")
    coverage_object = _mapping(coverage, "coverage")
    if manifest_object.get("version") != 1 or coverage_object.get("version") != 1:
        raise ConfigurationError("manifest and coverage versions must both be 1")

    profiles_object = _mapping(manifest_object.get("profiles"), "manifest.profiles")
    known_profiles = set(profiles_object)
    if not {"fast", "complete"}.issubset(known_profiles):
        raise ConfigurationError("manifest must define fast and complete profiles")
    for profile_id, profile in profiles_object.items():
        profile_object = _mapping(profile, f"profile {profile_id}")
        for budget_key in ("budget_seconds", "ci_budget_seconds"):
            budget = profile_object.get(budget_key)
            if not isinstance(budget, int) or isinstance(budget, bool) or budget <= 0:
                raise ConfigurationError(
                    f"profile {profile_id} must have a positive {budget_key}"
                )

    phase_ids: set[str] = set()
    resource_limits_object = _mapping(
        manifest_object.get("resource_limits", {}), "manifest.resource_limits"
    )
    resource_limits: dict[str, int] = {}
    for resource_id, limit in resource_limits_object.items():
        _nonempty_string(resource_id, "resource id")
        if not isinstance(limit, int) or isinstance(limit, bool) or limit <= 0:
            raise ConfigurationError(f"resource {resource_id} must have a positive limit")
        resource_limits[resource_id] = limit
    for kind in ("preparations", "shards"):
        phases = _sequence(manifest_object.get(kind), f"manifest.{kind}")
        if not phases:
            raise ConfigurationError(f"manifest.{kind} must not be empty")
        for index, phase in enumerate(phases):
            phase_id = _validate_phase(
                phase,
                label=f"manifest.{kind}[{index}]",
                known_profiles=known_profiles,
            )
            if phase_id in phase_ids:
                raise ConfigurationError(f"duplicate phase id {phase_id}")
            phase_ids.add(phase_id)
            resource = _mapping(phase, "phase").get("resource")
            if resource is not None and resource not in resource_limits:
                raise ConfigurationError(f"{phase_id} uses unknown resource {resource}")

    shards = {
        _mapping(shard, "shard")["id"]: _mapping(shard, "shard")
        for shard in _sequence(manifest_object.get("shards"), "manifest.shards")
    }
    requirements = _sequence(coverage_object.get("requirements"), "coverage.requirements")
    requirement_ids: set[str] = set()
    covered_surfaces: set[str] = set()
    referenced_shards: set[str] = set()
    for index, requirement in enumerate(requirements):
        item = _mapping(requirement, f"coverage.requirements[{index}]")
        requirement_id = _nonempty_string(item.get("id"), "requirement id")
        if requirement_id in requirement_ids:
            raise ConfigurationError(f"duplicate requirement id {requirement_id}")
        requirement_ids.add(requirement_id)
        surface = _nonempty_string(item.get("surface"), f"{requirement_id}.surface")
        covered_surfaces.add(surface)
        _nonempty_string(item.get("behavior"), f"{requirement_id}.behavior")
        failure_modes = _sequence(item.get("failure_modes"), f"{requirement_id}.failure_modes")
        if not failure_modes:
            raise ConfigurationError(f"{requirement_id} has no failure modes")
        for failure_mode in failure_modes:
            _nonempty_string(failure_mode, f"{requirement_id}.failure_modes")
        evidence = _sequence(item.get("evidence"), f"{requirement_id}.evidence")
        if not evidence:
            raise ConfigurationError(f"{requirement_id} has no test evidence")
        for reference in evidence:
            evidence_reference = _nonempty_string(reference, f"{requirement_id}.evidence")
            evidence_path = evidence_reference.split("::", 1)[0]
            path = Path(evidence_path)
            if path.is_absolute() or ".." in path.parts:
                raise ConfigurationError(
                    f"{requirement_id} has unsafe evidence path {evidence_path}"
                )
            if repository is not None and not (repository / path).is_file():
                raise ConfigurationError(f"missing evidence file {evidence_path}")
        verifiers = _sequence(item.get("verifiers"), f"{requirement_id}.verifiers")
        if not verifiers:
            raise ConfigurationError(f"{requirement_id} has no verifier")
        for verifier in verifiers:
            verifier_id = _nonempty_string(verifier, f"{requirement_id}.verifiers")
            if verifier_id not in shards:
                raise ConfigurationError(
                    f"{requirement_id} references unknown verifier {verifier_id}"
                )
            referenced_shards.add(verifier_id)

    missing_surfaces = REQUIRED_SURFACES - covered_surfaces
    if missing_surfaces:
        raise ConfigurationError(
            f"missing required surfaces: {', '.join(sorted(missing_surfaces))}"
        )
    for shard_id in sorted(referenced_shards):
        profiles = _sequence(shards[shard_id].get("profiles"), f"shard {shard_id}.profiles")
        if "complete" not in profiles:
            raise ConfigurationError(f"verifier shard {shard_id} is missing from the complete profile")


def _replace_placeholders(argument: str, values: Mapping[str, str]) -> str:
    result = argument
    for key, value in values.items():
        result = result.replace("{" + key + "}", value)
    return result


def _phase_environment(
    base: Mapping[str, str],
    *,
    phase_id: str,
    phase_index: int,
    port_base: int,
    run_root: Path,
) -> dict[str, str]:
    run_key = hashlib.sha256(str(run_root).encode()).hexdigest()[:8]
    temporary_parent = Path(base.get("OPENFRAG_SUITE_TMP_ROOT", base.get("TMPDIR", "/tmp")))
    work = temporary_parent / f"openfrag-suite-{run_key}-{phase_index}"
    shutil.rmtree(work, ignore_errors=True)
    home = work / "home"
    temporary = work / "tmp"
    for directory in (
        home,
        temporary,
        work / "data",
        work / "config",
        work / "cache",
        work / "runtime",
    ):
        directory.mkdir(parents=True, exist_ok=True)
    environment = dict(base)
    environment.update(
        {
            "HOME": str(home),
            "TMPDIR": str(temporary),
            "XDG_CACHE_HOME": str(work / "cache"),
            "XDG_CONFIG_HOME": str(work / "config"),
            "XDG_DATA_HOME": str(work / "data"),
            "XDG_RUNTIME_DIR": str(work / "runtime"),
            "OPENFRAG_TEST_PORT": str(port_base + phase_index),
            "OPENFRAG_TEST_SHARD": phase_id,
            "OPENFRAG_TEST_WORK_ROOT": str(work),
        }
    )
    return environment


def _run_phase(
    phase: Mapping[str, Any],
    *,
    phase_index: int,
    repository: Path,
    run_root: Path,
    base_environment: Mapping[str, str],
    placeholders: Mapping[str, str],
    port_base: int,
) -> PhaseResult:
    phase_id = str(phase["id"])
    log = run_root / f"{phase_id}.log"
    environment = _phase_environment(
        base_environment,
        phase_id=phase_id,
        phase_index=phase_index,
        port_base=port_base,
        run_root=run_root,
    )
    started = time.monotonic()
    returncode = 0
    deadline = started + int(phase["timeout_seconds"])
    with log.open("w", encoding="utf-8") as output:
        for command in phase["commands"]:
            arguments = [
                _replace_placeholders(str(argument), placeholders) for argument in command
            ]
            output.write(f"$ {' '.join(arguments)}\n")
            output.flush()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                output.write("phase timeout expired before command start\n")
                returncode = 124
                break
            try:
                process = subprocess.Popen(
                    arguments,
                    cwd=repository,
                    env=environment,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                try:
                    returncode = process.wait(timeout=remaining)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    output.write(f"phase timed out after {phase['timeout_seconds']} seconds\n")
                    returncode = 124
            except OSError as error:
                output.write(f"could not launch command: {error}\n")
                returncode = 127
            if returncode != 0:
                break
    shutil.rmtree(environment["OPENFRAG_TEST_WORK_ROOT"], ignore_errors=True)
    return PhaseResult(
        phase_id=phase_id,
        duration_seconds=time.monotonic() - started,
        returncode=returncode,
        log=log,
    )


def _run_limited_phase(
    phase: Mapping[str, Any],
    *,
    semaphore: Semaphore | None,
    phase_index: int,
    repository: Path,
    run_root: Path,
    base_environment: Mapping[str, str],
    placeholders: Mapping[str, str],
    port_base: int,
) -> PhaseResult:
    if semaphore is None:
        return _run_phase(
            phase,
            phase_index=phase_index,
            repository=repository,
            run_root=run_root,
            base_environment=base_environment,
            placeholders=placeholders,
            port_base=port_base,
        )
    with semaphore:
        return _run_phase(
            phase,
            phase_index=phase_index,
            repository=repository,
            run_root=run_root,
            base_environment=base_environment,
            placeholders=placeholders,
            port_base=port_base,
        )


def run_suite(
    manifest: Any,
    coverage: Any,
    *,
    profile: str,
    repository: Path,
    results_root: Path,
    jobs: int,
    enforce_budget: bool = True,
) -> SuiteResult:
    """Run serialized preparations followed by every independent shard."""
    validate_configuration(manifest, coverage, repository=repository)
    manifest_object = _mapping(manifest, "manifest")
    profiles = _mapping(manifest_object["profiles"], "manifest.profiles")
    if profile not in profiles:
        raise ConfigurationError(f"unknown profile {profile}")
    if jobs <= 0:
        raise ConfigurationError("jobs must be positive")
    results_root.mkdir(parents=True, exist_ok=True)
    artifacts = results_root / "artifacts"
    artifacts.mkdir(exist_ok=True)
    cargo_target = Path(
        os.environ.get(
            "CARGO_TARGET_DIR",
            str(repository / "target"),
        )
    ).resolve()
    cargo_target.mkdir(parents=True, exist_ok=True)
    placeholders = {
        "artifacts": str(artifacts),
        "cargo_target": str(cargo_target),
        "repo": str(repository),
        "run": str(results_root),
    }
    base_environment = os.environ.copy()
    original_home = Path.home()
    base_environment.update(
        {
            "CARGO_HOME": os.environ.get("CARGO_HOME", str(original_home / ".cargo")),
            "CARGO_TARGET_DIR": str(cargo_target),
            "CARGO_TERM_COLOR": "never",
            "OPENFRAG_TEST_ARTIFACTS": str(artifacts),
            "OPENFRAG_TEST_REPOSITORY": str(repository),
            "OPENFRAG_TEST_RUN_ROOT": str(results_root),
            "PLAYWRIGHT_BROWSERS_PATH": os.environ.get(
                "PLAYWRIGHT_BROWSERS_PATH", str(original_home / ".cache/ms-playwright")
            ),
            "PYTHONDONTWRITEBYTECODE": "1",
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(original_home / ".rustup")),
        }
    )
    preparations = [
        _mapping(item, "preparation")
        for item in _sequence(manifest_object["preparations"], "manifest.preparations")
        if profile in _sequence(_mapping(item, "preparation")["profiles"], "preparation.profiles")
    ]
    shards = [
        _mapping(item, "shard")
        for item in _sequence(manifest_object["shards"], "manifest.shards")
        if profile in _sequence(_mapping(item, "shard")["profiles"], "shard.profiles")
    ]
    expected_shards = {str(shard["id"]) for shard in shards}
    phase_results: list[PhaseResult] = []
    started = time.monotonic()
    failed_preparations: set[str] = set()
    port_base = 20_000 + (
        int(hashlib.sha256(str(results_root).encode()).hexdigest()[:8], 16) % 10_000
    )
    for index, preparation in enumerate(preparations):
        result = _run_phase(
            preparation,
            phase_index=index,
            repository=repository,
            run_root=results_root,
            base_environment=base_environment,
            placeholders=placeholders,
            port_base=port_base,
        )
        phase_results.append(result)
        if result.returncode != 0:
            failed_preparations.add(result.phase_id)
            break

    executed_shards: set[str] = set()
    failed_shards: set[str] = set()
    if not failed_preparations:
        resource_limits = _mapping(
            manifest_object.get("resource_limits", {}), "manifest.resource_limits"
        )
        resource_semaphores = {
            resource_id: Semaphore(int(limit))
            for resource_id, limit in resource_limits.items()
        }
        with ThreadPoolExecutor(max_workers=min(jobs, len(shards))) as executor:
            futures = {
                executor.submit(
                    _run_limited_phase,
                    shard,
                    semaphore=resource_semaphores.get(str(shard.get("resource"))),
                    phase_index=len(preparations) + index,
                    repository=repository,
                    run_root=results_root,
                    base_environment=base_environment,
                    placeholders=placeholders,
                    port_base=port_base,
                ): str(shard["id"])
                for index, shard in enumerate(shards)
            }
            for future in as_completed(futures):
                result = future.result()
                phase_results.append(result)
                executed_shards.add(result.phase_id)
                if result.returncode != 0:
                    failed_shards.add(result.phase_id)

    duration = time.monotonic() - started
    profile_object = _mapping(profiles[profile], f"profile {profile}")
    budget_key = "ci_budget_seconds" if os.environ.get("CI") == "true" else "budget_seconds"
    budget = int(profile_object[budget_key])
    budget_exceeded = enforce_budget and duration > budget
    missing_shards = expected_shards - executed_shards
    return SuiteResult(
        profile=profile,
        succeeded=not (
            failed_preparations or failed_shards or missing_shards or budget_exceeded
        ),
        duration_seconds=duration,
        budget_seconds=budget,
        budget_exceeded=budget_exceeded,
        executed_shards=executed_shards,
        failed_shards=failed_shards,
        failed_preparations=failed_preparations,
        missing_shards=missing_shards,
        logs={result.phase_id: result.log for result in phase_results},
        timings={result.phase_id: result.duration_seconds for result in phase_results},
    )


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ConfigurationError(f"could not load {path}: {error}") from error


def _write_summary(result: SuiteResult, path: Path) -> None:
    payload = {
        "profile": result.profile,
        "succeeded": result.succeeded,
        "duration_seconds": round(result.duration_seconds, 3),
        "budget_seconds": result.budget_seconds,
        "budget_exceeded": result.budget_exceeded,
        "executed_shards": sorted(result.executed_shards),
        "failed_shards": sorted(result.failed_shards),
        "failed_preparations": sorted(result.failed_preparations),
        "missing_shards": sorted(result.missing_shards),
        "timings": {
            phase_id: round(duration, 3)
            for phase_id, duration in sorted(result.timings.items())
        },
        "logs": {
            phase_id: str(log) for phase_id, log in sorted(result.logs.items())
        },
    }
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run OpenFrag's machine-checked whole-app verification suite."
    )
    parser.add_argument("profile", choices=("fast", "complete", "validate"))
    parser.add_argument("--jobs", type=int, help="Maximum concurrent verifier shards.")
    parser.add_argument("--no-budget", action="store_true", help="Report but do not enforce the time budget.")
    parser.add_argument("--manifest", type=Path, default=Path("tests/verification/shards.json"))
    parser.add_argument("--coverage", type=Path, default=Path("tests/verification/coverage.json"))
    parser.add_argument("--results-dir", type=Path, help="Use an exact results directory.")
    return parser


def main() -> int:
    arguments = _parser().parse_args()
    repository = Path(__file__).resolve().parents[1]
    manifest_path = arguments.manifest
    coverage_path = arguments.coverage
    if not manifest_path.is_absolute():
        manifest_path = repository / manifest_path
    if not coverage_path.is_absolute():
        coverage_path = repository / coverage_path
    try:
        manifest = _load_json(manifest_path)
        coverage = _load_json(coverage_path)
        validate_configuration(manifest, coverage, repository=repository)
        if arguments.profile == "validate":
            print(
                f"coverage map: PASS ({len(coverage['requirements'])} requirements, "
                f"{len(manifest['shards'])} shards)"
            )
            return 0
        profile = arguments.profile
        default_jobs = int(manifest.get("max_jobs", min(4, os.cpu_count() or 1)))
        jobs = arguments.jobs if arguments.jobs is not None else default_jobs
        if arguments.results_dir is None:
            stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
            results_root = repository / "target/openfrag-suite/runs" / f"{profile}-{stamp}-{os.getpid()}"
        else:
            results_root = arguments.results_dir.resolve()
        result = run_suite(
            manifest,
            coverage,
            profile=profile,
            repository=repository,
            results_root=results_root,
            jobs=jobs,
            enforce_budget=not arguments.no_budget,
        )
    except ConfigurationError as error:
        print(f"configuration error: {error}", file=os.sys.stderr)
        return 2

    _write_summary(result, results_root / "summary.json")
    print(f"\n{profile} suite: {'PASS' if result.succeeded else 'FAIL'}")
    print(
        f"wall={result.duration_seconds:.2f}s budget={result.budget_seconds}s "
        f"critical_path={result.duration_seconds:.2f}s jobs={jobs}"
    )
    print("slowest phases:")
    for phase_id, duration in sorted(
        result.timings.items(), key=lambda item: item[1], reverse=True
    )[:5]:
        print(f"  {phase_id}: {duration:.2f}s")
    print(f"results: {results_root}")
    if not result.succeeded:
        if result.failed_preparations:
            print(f"failed preparations: {', '.join(sorted(result.failed_preparations))}")
        if result.failed_shards:
            print(f"failed shards: {', '.join(sorted(result.failed_shards))}")
        if result.missing_shards:
            print(f"missing shards: {', '.join(sorted(result.missing_shards))}")
        if result.budget_exceeded:
            print("time budget exceeded")
        for phase_id in sorted(result.failed_preparations | result.failed_shards):
            print(f"\n--- complete failure log: {phase_id} ---")
            print(result.logs[phase_id].read_text(errors="replace"), end="")
    return 0 if result.succeeded else 1


if __name__ == "__main__":
    raise SystemExit(main())
