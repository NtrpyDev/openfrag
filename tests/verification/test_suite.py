#!/usr/bin/env python3
"""Contract tests for the whole-app verification manifest."""

from __future__ import annotations

import unittest
from pathlib import Path
import tempfile

from scripts.test_suite import (
    ConfigurationError,
    REQUIRED_SURFACES,
    run_suite,
    validate_configuration,
)


def valid_manifest() -> dict[str, object]:
    return {
        "version": 1,
        "profiles": {
            "fast": {"budget_seconds": 30, "ci_budget_seconds": 60},
            "complete": {"budget_seconds": 120, "ci_budget_seconds": 180},
        },
        "preparations": [
            {
                "id": "prepare",
                "profiles": ["fast", "complete"],
                "commands": [["true"]],
                "timeout_seconds": 10,
            }
        ],
        "shards": [
            {
                "id": "observable",
                "profiles": ["fast", "complete"],
                "commands": [["true"]],
                "timeout_seconds": 10,
            }
        ],
    }


def valid_coverage() -> dict[str, object]:
    return {
        "version": 1,
        "requirements": [
            {
                "id": f"{surface}.contract",
                "surface": surface,
                "behavior": f"The {surface} public contract remains observable.",
                "failure_modes": ["The contract rejects invalid or unavailable state."],
                "verifiers": ["observable"],
                "evidence": ["tests/verification/test_suite.py::ConfigurationContract"],
            }
            for surface in sorted(REQUIRED_SURFACES)
        ],
    }


class ConfigurationContract(unittest.TestCase):
    def test_complete_coverage_and_known_shards_are_accepted(self) -> None:
        validate_configuration(valid_manifest(), valid_coverage())

    def test_required_behavior_without_a_verifier_is_rejected(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"][0]["verifiers"] = []  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "has no verifier"):
            validate_configuration(valid_manifest(), coverage)

    def test_required_behavior_without_test_evidence_is_rejected(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"][0]["evidence"] = []  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "has no test evidence"):
            validate_configuration(valid_manifest(), coverage)

    def test_missing_evidence_file_is_rejected_when_repository_is_known(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"][0]["evidence"] = ["tests/missing.py::contract"]  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "missing evidence file tests/missing.py"):
            validate_configuration(valid_manifest(), coverage, repository=Path.cwd())

    def test_unknown_verifier_is_rejected(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"][0]["verifiers"] = ["missing"]  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "unknown verifier missing"):
            validate_configuration(valid_manifest(), coverage)

    def test_missing_product_surface_is_rejected(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"] = [
            row
            for row in coverage["requirements"]  # type: ignore[assignment]
            if row["surface"] != "release_artifacts"
        ]

        with self.assertRaisesRegex(ConfigurationError, "missing required surfaces: release_artifacts"):
            validate_configuration(valid_manifest(), coverage)

    def test_complete_profile_cannot_omit_a_verifier_shard(self) -> None:
        manifest = valid_manifest()
        manifest["shards"][0]["profiles"] = ["fast"]  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "missing from the complete profile"):
            validate_configuration(manifest, valid_coverage())

    def test_duplicate_ids_are_rejected(self) -> None:
        coverage = valid_coverage()
        coverage["requirements"].append(coverage["requirements"][0])  # type: ignore[union-attr]

        with self.assertRaisesRegex(ConfigurationError, "duplicate requirement id"):
            validate_configuration(valid_manifest(), coverage)

    def test_local_and_ci_budgets_are_both_required(self) -> None:
        manifest = valid_manifest()
        del manifest["profiles"]["complete"]["ci_budget_seconds"]  # type: ignore[index]

        with self.assertRaisesRegex(ConfigurationError, "complete must have a positive ci_budget_seconds"):
            validate_configuration(manifest, valid_coverage())


class RunnerContract(unittest.TestCase):
    def test_independent_shards_reach_a_barrier_concurrently(self) -> None:
        marker_program = (
            "import os,time; from pathlib import Path; "
            "root=Path(os.environ['OPENFRAG_TEST_RUN_ROOT']); "
            "(root / ('ready-' + os.environ['OPENFRAG_TEST_SHARD'])).write_text('ready'); "
            "deadline=time.monotonic()+2; "
            "exec(\"while len(list(root.glob('ready-*'))) < 2 and time.monotonic() < deadline:\\n time.sleep(0.01)\"); "
            "assert len(list(root.glob('ready-*'))) == 2"
        )
        manifest = valid_manifest()
        manifest["shards"] = [  # type: ignore[index]
            {
                "id": shard_id,
                "profiles": ["complete"],
                "commands": [["python3", "-c", marker_program]],
                "timeout_seconds": 5,
            }
            for shard_id in ("parallel-a", "parallel-b")
        ]
        coverage = valid_coverage()
        for requirement in coverage["requirements"]:  # type: ignore[union-attr]
            requirement["verifiers"] = ["parallel-a"]

        with tempfile.TemporaryDirectory() as temporary:
            result = run_suite(
                manifest,
                coverage,
                profile="complete",
                repository=Path.cwd(),
                results_root=Path(temporary),
                jobs=2,
                enforce_budget=False,
            )

        self.assertTrue(result.succeeded)
        self.assertEqual(result.executed_shards, {"parallel-a", "parallel-b"})

    def test_failure_output_is_preserved_and_other_shards_finish(self) -> None:
        manifest = valid_manifest()
        manifest["shards"] = [  # type: ignore[index]
            {
                "id": "fails",
                "profiles": ["complete"],
                "commands": [["python3", "-c", "print('specific failure'); raise SystemExit(7)"]],
                "timeout_seconds": 5,
            },
            {
                "id": "finishes",
                "profiles": ["complete"],
                "commands": [["python3", "-c", "print('still finished')"]],
                "timeout_seconds": 5,
            },
        ]
        coverage = valid_coverage()
        for requirement in coverage["requirements"]:  # type: ignore[union-attr]
            requirement["verifiers"] = ["fails"]

        with tempfile.TemporaryDirectory() as temporary:
            result = run_suite(
                manifest,
                coverage,
                profile="complete",
                repository=Path.cwd(),
                results_root=Path(temporary),
                jobs=2,
                enforce_budget=False,
            )
            failure_log = result.logs["fails"].read_text()
            success_log = result.logs["finishes"].read_text()

        self.assertFalse(result.succeeded)
        self.assertEqual(result.failed_shards, {"fails"})
        self.assertEqual(result.executed_shards, {"fails", "finishes"})
        self.assertIn("specific failure", failure_log)
        self.assertIn("still finished", success_log)

    def test_resource_limit_serializes_only_the_named_boundary(self) -> None:
        lock_program = (
            "import os,time; from pathlib import Path; "
            "lock=Path(os.environ['OPENFRAG_TEST_RUN_ROOT'])/'exclusive-lock'; "
            "lock.mkdir(); time.sleep(0.05); lock.rmdir()"
        )
        manifest = valid_manifest()
        manifest["resource_limits"] = {"exclusive": 1}
        manifest["shards"] = [  # type: ignore[index]
            {
                "id": shard_id,
                "profiles": ["complete"],
                "resource": "exclusive",
                "commands": [["python3", "-c", lock_program]],
                "timeout_seconds": 5,
            }
            for shard_id in ("exclusive-a", "exclusive-b")
        ]
        coverage = valid_coverage()
        for requirement in coverage["requirements"]:  # type: ignore[union-attr]
            requirement["verifiers"] = ["exclusive-a"]

        with tempfile.TemporaryDirectory() as temporary:
            result = run_suite(
                manifest,
                coverage,
                profile="complete",
                repository=Path.cwd(),
                results_root=Path(temporary),
                jobs=2,
                enforce_budget=False,
            )

        self.assertTrue(result.succeeded)
        self.assertEqual(result.executed_shards, {"exclusive-a", "exclusive-b"})


if __name__ == "__main__":
    unittest.main()
