"""Unit tests for PILOT report validation and aggregation.

All records in this module are synthetic fixtures for testing the tool only;
they are not pilot execution evidence or evaluation results.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import pilot_report


def make_catalog() -> dict:
    tasks = []
    categories = ("bugfix", "feature", "test")
    for index in range(pilot_report.TASK_COUNT):
        tasks.append(
            {
                "task_id": f"task-{index + 1:02d}",
                "category": categories[index % len(categories)],
                "pilot_repo": "repo-a" if index < 10 else "repo-b",
                "prompt_sha256": f"{index + 1:064x}",
            }
        )
    return {"schema_version": 1, "task_set_id": "unit-test-fixture", "tasks": tasks}


def make_results(frozen: dict) -> dict:
    rows = []
    for index, task in enumerate(frozen["tasks"]):
        is_effective_pr = index == 0
        rows.append(
            {
                "task_id": task["task_id"],
                "status": "completed" if is_effective_pr else "not_tested",
                "pr_id": 17 if is_effective_pr else None,
                "commit_oid": "a" * 40 if is_effective_pr else None,
                "ci_result": "passed" if is_effective_pr else "not_run",
                "human_accepted": is_effective_pr,
                "rework_minutes": 5 if is_effective_pr else None,
                "cost_usd": "1.25" if is_effective_pr else None,
                "cost_is_estimate": False,
            }
        )
    return {
        "schema_version": 1,
        "catalog_sha256": frozen["catalog_sha256"],
        "metadata": {
            "musehub_version": "0.18.0-rc20",
            "git_commit": "b" * 40,
            "agent_model": "unit-test-model",
            "environment": "unit-test-only",
            "started_at": "2026-01-01T10:00:00Z",
            "finished_at": "2026-01-01T11:00:00+00:00",
        },
        "tasks": rows,
        "defects": [],
        "release_decision": {
            "decision": "continue",
            "reviewer": "pilot-reviewer",
            "decided_at": "2026-01-01T12:00:00Z",
            "rationale": "Continue to the next evaluation stage based on the recorded evidence.",
        },
    }


class FreezeCatalogTests(unittest.TestCase):
    def test_sorts_tasks_and_hash_is_independent_of_input_order(self) -> None:
        catalog = make_catalog()
        reversed_catalog = {**catalog, "tasks": list(reversed(catalog["tasks"]))}

        first = pilot_report.freeze_catalog(catalog)
        second = pilot_report.freeze_catalog(reversed_catalog)

        self.assertEqual(first["tasks"], second["tasks"])
        self.assertEqual(first["catalog_sha256"], second["catalog_sha256"])
        self.assertEqual(first["tasks"][0]["task_id"], "task-01")

    def test_rejects_duplicate_task_ids(self) -> None:
        catalog = make_catalog()
        catalog["tasks"][1]["task_id"] = catalog["tasks"][0]["task_id"]

        with self.assertRaisesRegex(pilot_report.ReportError, "duplicate task_id"):
            pilot_report.freeze_catalog(catalog)

    def test_requires_three_categories_and_exactly_two_repositories(self) -> None:
        catalog = make_catalog()
        catalog["tasks"] = [
            {**task, "category": "bugfix"} for task in catalog["tasks"]
        ]
        with self.assertRaisesRegex(pilot_report.ReportError, "bugfix, feature, and test"):
            pilot_report.freeze_catalog(catalog)

        catalog = make_catalog()
        catalog["tasks"] = [
            {**task, "pilot_repo": "repo-a"} for task in catalog["tasks"]
        ]
        with self.assertRaisesRegex(pilot_report.ReportError, "exactly two"):
            pilot_report.freeze_catalog(catalog)


class BuildReportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.frozen = pilot_report.freeze_catalog(make_catalog())
        self.results = make_results(self.frozen)

    def test_aggregates_only_evidence_qualified_effective_prs(self) -> None:
        report = pilot_report.build_report(self.frozen, self.results)
        metrics = report["metrics"]

        self.assertEqual(metrics["task_count"], 20)
        self.assertEqual(metrics["completed_count"], 1)
        self.assertEqual(metrics["completion_rate"], "5.00%")
        self.assertEqual(metrics["effective_pr_count"], 1)
        self.assertEqual(metrics["effective_pr_rate"], "5.00%")
        self.assertEqual(metrics["rework_recorded_count"], 1)
        self.assertEqual(metrics["rework_total_minutes"], "5.00")
        self.assertEqual(metrics["cost_recorded_count"], 1)
        self.assertEqual(metrics["cost_total_usd"], "1.25")
        self.assertEqual(metrics["by_repo"]["repo-a"]["task_count"], 10)
        self.assertEqual(metrics["by_repo"]["repo-b"]["task_count"], 10)
        markdown = pilot_report.render_markdown(report)
        self.assertIn("Missing rework or cost values are excluded", markdown)
        self.assertIn("unit-test-only", markdown)
        self.assertIn("## Human release decision", markdown)
        self.assertIn("No defects recorded.", markdown)
        self.assertEqual(report["release_decision"]["decision"], "continue")

    def test_includes_human_defects_and_release_decision(self) -> None:
        results = {
            **self.results,
            "defects": [
                {
                    "defect_id": "D-001",
                    "task_id": "task-01",
                    "severity": "major",
                    "summary": "Acceptance check missed an edge case | reproducible",
                    "evidence": "run-01/artifacts/defect-001",
                }
            ],
            "release_decision": {
                **self.results["release_decision"],
                "decision": "pause",
                "rationale": "Review the reproducible edge case before continuing.",
            },
        }

        report = pilot_report.build_report(self.frozen, results)
        markdown = pilot_report.render_markdown(report)

        self.assertEqual(report["defects"][0]["defect_id"], "D-001")
        self.assertIn("Acceptance check missed an edge case &#124; reproducible", markdown)
        self.assertIn("Decision: `pause`", markdown)
        self.assertIn("human-entered evidence", markdown)

    def test_rejects_invalid_defect_reference_and_incomplete_decision(self) -> None:
        invalid_defect = {
            **self.results,
            "defects": [
                {
                    "defect_id": "D-001",
                    "task_id": "unknown-task",
                    "severity": "major",
                    "summary": "A real issue",
                    "evidence": "run-01/artifacts/defect-001",
                }
            ],
        }
        with self.assertRaisesRegex(pilot_report.ReportError, "unknown task_id"):
            pilot_report.build_report(self.frozen, invalid_defect)

        incomplete_decision = {
            **self.results,
            "release_decision": {**self.results["release_decision"], "rationale": "  "},
        }
        with self.assertRaisesRegex(pilot_report.ReportError, "rationale must be non-empty"):
            pilot_report.build_report(self.frozen, incomplete_decision)

    def test_rejects_stale_catalog_hash_and_missing_results(self) -> None:
        stale = {**self.results, "catalog_sha256": "0" * 64}
        with self.assertRaisesRegex(pilot_report.ReportError, "does not match"):
            pilot_report.build_report(self.frozen, stale)

        missing = {**self.results, "tasks": self.results["tasks"][:-1]}
        with self.assertRaisesRegex(pilot_report.ReportError, "exactly 20"):
            pilot_report.build_report(self.frozen, missing)

    def test_refuses_result_that_does_not_meet_effective_pr_requirements(self) -> None:
        row = {**self.results["tasks"][0], "ci_result": "failed"}
        results = {**self.results, "tasks": [row, *self.results["tasks"][1:]]}

        report = pilot_report.build_report(self.frozen, results)

        self.assertEqual(report["metrics"]["effective_pr_count"], 0)

    def test_rejects_timezone_free_or_reversed_run_window(self) -> None:
        metadata = {**self.results["metadata"], "started_at": "2026-01-01T10:00:00"}
        results = {**self.results, "metadata": metadata}
        with self.assertRaisesRegex(pilot_report.ReportError, "include a timezone"):
            pilot_report.build_report(self.frozen, results)

        metadata = {
            **self.results["metadata"],
            "started_at": "2026-01-01T12:00:00Z",
            "finished_at": "2026-01-01T11:00:00Z",
        }
        results = {**self.results, "metadata": metadata}
        with self.assertRaisesRegex(pilot_report.ReportError, "earlier than started_at"):
            pilot_report.build_report(self.frozen, results)


class OutputTests(unittest.TestCase):
    def test_write_new_does_not_overwrite_existing_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.json"
            output.write_text("keep this", encoding="utf-8")

            with self.assertRaisesRegex(pilot_report.ReportError, "refusing to overwrite"):
                pilot_report.write_new(output, "replacement")

            self.assertEqual(output.read_text(encoding="utf-8"), "keep this")

    def test_output_preflight_prevents_partial_report_when_second_file_exists(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            json_output = Path(directory) / "report.json"
            markdown_output = Path(directory) / "report.md"
            markdown_output.write_text("keep this", encoding="utf-8")

            with self.assertRaisesRegex(pilot_report.ReportError, "refusing to overwrite"):
                pilot_report.ensure_outputs_new([json_output, markdown_output])

            self.assertFalse(json_output.exists())
            self.assertEqual(markdown_output.read_text(encoding="utf-8"), "keep this")


if __name__ == "__main__":
    unittest.main()
