#!/usr/bin/env python3
"""Freeze a PILOT-01 task catalog and report evidence-backed evaluation results."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1
TASK_COUNT = 20
CATEGORIES = {"bugfix", "feature", "test"}
STATUSES = {"completed", "failed", "blocked", "not_tested"}
CI_RESULTS = {"passed", "failed", "blocked", "not_run"}
DEFECT_SEVERITIES = {"blocker", "major", "minor", "trivial"}
RELEASE_DECISIONS = {"continue", "pause", "stop"}
HEX_64 = re.compile(r"^[0-9a-fA-F]{64}$")
COMMIT_OID = re.compile(r"^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})$")
TASK_SET_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
APP_VERSION = re.compile(r"^\d+\.\d+\.\d+(?:-rc\d+)?$")
ENVIRONMENT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
COST = re.compile(r"^(?:0|[1-9]\d*)(?:\.\d{1,6})?$")


class ReportError(ValueError):
    """Raised when a catalog or result record violates the report contract."""


def _object(value: Any, label: str, allowed: set[str], required: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ReportError(f"{label} must be a JSON object")
    unknown = set(value) - allowed
    missing = required - set(value)
    if unknown:
        raise ReportError(f"{label} has unknown fields: {', '.join(sorted(unknown))}")
    if missing:
        raise ReportError(f"{label} is missing fields: {', '.join(sorted(missing))}")
    return value


def _canonical_digest(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _read_json(path: Path) -> Any:
    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            parse_float=Decimal,
            parse_constant=lambda value: (_ for _ in ()).throw(ReportError(f"invalid number: {value}")),
        )
    except OSError as error:
        raise ReportError(f"cannot read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ReportError(f"invalid JSON in {path}: {error}") from error
    except UnicodeError as error:
        raise ReportError(f"input is not valid UTF-8 JSON in {path}: {error}") from error


def freeze_catalog(source: Any) -> dict[str, Any]:
    catalog = _object(
        source,
        "catalog",
        {"schema_version", "task_set_id", "tasks"},
        {"schema_version", "task_set_id", "tasks"},
    )
    if type(catalog["schema_version"]) is not int or catalog["schema_version"] != SCHEMA_VERSION:
        raise ReportError(f"catalog schema_version must be {SCHEMA_VERSION}")
    task_set_id = catalog["task_set_id"]
    if not isinstance(task_set_id, str) or not TASK_SET_ID.fullmatch(task_set_id):
        raise ReportError("task_set_id must be 1-64 safe ASCII characters")
    tasks = catalog["tasks"]
    if not isinstance(tasks, list) or len(tasks) != TASK_COUNT:
        raise ReportError(f"catalog must contain exactly {TASK_COUNT} tasks")

    frozen_tasks: list[dict[str, str]] = []
    ids: set[str] = set()
    categories: set[str] = set()
    for index, raw_task in enumerate(tasks, start=1):
        task = _object(
            raw_task,
            f"catalog.tasks[{index}]",
            {"task_id", "category", "pilot_repo", "prompt_sha256"},
            {"task_id", "category", "pilot_repo", "prompt_sha256"},
        )
        task_id = task["task_id"]
        category = task["category"]
        pilot_repo = task["pilot_repo"]
        digest = task["prompt_sha256"]
        if not isinstance(task_id, str) or not TASK_SET_ID.fullmatch(task_id):
            raise ReportError(f"catalog.tasks[{index}].task_id is invalid")
        if task_id in ids:
            raise ReportError(f"duplicate task_id: {task_id}")
        if not isinstance(category, str) or category not in CATEGORIES:
            raise ReportError(f"task {task_id} category must be one of {', '.join(sorted(CATEGORIES))}")
        if not isinstance(pilot_repo, str) or not TASK_SET_ID.fullmatch(pilot_repo):
            raise ReportError(f"task {task_id} pilot_repo is invalid")
        if not isinstance(digest, str) or not HEX_64.fullmatch(digest):
            raise ReportError(f"task {task_id} prompt_sha256 must be a SHA-256 hex digest")
        ids.add(task_id)
        categories.add(category)
        frozen_tasks.append(
            {
                "task_id": task_id,
                "category": category,
                "pilot_repo": pilot_repo,
                "prompt_sha256": digest.lower(),
            }
        )

    if categories != CATEGORIES:
        raise ReportError("catalog must include bugfix, feature, and test tasks")
    if len({task["pilot_repo"] for task in frozen_tasks}) != 2:
        raise ReportError("catalog must cover exactly two pilot_repo aliases")
    frozen_tasks.sort(key=lambda task: task["task_id"])
    payload = {"schema_version": SCHEMA_VERSION, "task_set_id": task_set_id, "tasks": frozen_tasks}
    return {**payload, "catalog_sha256": _canonical_digest(payload)}


def load_frozen_catalog(source: Any) -> dict[str, Any]:
    frozen = _object(
        source,
        "frozen catalog",
        {"schema_version", "task_set_id", "tasks", "catalog_sha256"},
        {"schema_version", "task_set_id", "tasks", "catalog_sha256"},
    )
    expected = freeze_catalog({key: frozen[key] for key in ("schema_version", "task_set_id", "tasks")})
    if not isinstance(frozen["catalog_sha256"], str) or frozen["catalog_sha256"] != expected["catalog_sha256"]:
        raise ReportError("frozen catalog SHA-256 does not match its contents")
    return expected


def initialize_results(catalog_source: Any) -> dict[str, Any]:
    """Create a blank evidence-entry template; this is not pilot evidence."""
    catalog = load_frozen_catalog(catalog_source)
    return {
        "schema_version": SCHEMA_VERSION,
        "catalog_sha256": catalog["catalog_sha256"],
        "metadata": {
            "musehub_version": "",
            "git_commit": "",
            "agent_model": "",
            "environment": "",
            "started_at": "",
            "finished_at": "",
        },
        "tasks": [
            {
                "task_id": task["task_id"],
                "status": "not_tested",
                "pr_id": None,
                "commit_oid": None,
                "ci_result": "not_run",
                "human_accepted": False,
                "rework_minutes": None,
                "cost_usd": None,
                "cost_is_estimate": False,
            }
            for task in catalog["tasks"]
        ],
        "defects": [],
        "release_decision": {
            "decision": "",
            "reviewer": "",
            "decided_at": "",
            "rationale": "",
        },
    }


def _decimal(value: Any, label: str) -> Decimal:
    if isinstance(value, bool) or not isinstance(value, (int, Decimal)):
        raise ReportError(f"{label} must be a non-negative number or null")
    try:
        number = Decimal(value)
    except (InvalidOperation, TypeError, ValueError) as error:
        raise ReportError(f"{label} must be a non-negative number or null") from error
    if not number.is_finite() or number < 0 or number.as_tuple().exponent < -2:
        raise ReportError(f"{label} must be finite, non-negative, and use at most 2 decimal places")
    return number


def _timestamp(value: Any, label: str) -> datetime:
    if not isinstance(value, str):
        raise ReportError(f"{label} must be an ISO-8601 timestamp with a timezone")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ReportError(f"{label} must be an ISO-8601 timestamp with a timezone") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ReportError(f"{label} must include a timezone")
    return parsed


def _format_decimal(value: Decimal) -> str:
    text = format(value, "f")
    if "." not in text:
        return text + ".00"
    whole, fraction = text.split(".", maxsplit=1)
    fraction = fraction.rstrip("0")
    return f"{whole}.{fraction.ljust(2, '0')}"


def _format_cost(value: Decimal) -> str:
    text = format(value, "f")
    if "." not in text:
        return text + ".00"
    whole, fraction = text.split(".", maxsplit=1)
    fraction = fraction.rstrip("0")
    return f"{whole}.{fraction.ljust(2, '0')}"


def build_report(catalog_source: Any, result_source: Any) -> dict[str, Any]:
    catalog = load_frozen_catalog(catalog_source)
    results = _object(
        result_source,
        "results",
        {"schema_version", "catalog_sha256", "metadata", "tasks", "defects", "release_decision"},
        {"schema_version", "catalog_sha256", "metadata", "tasks", "defects", "release_decision"},
    )
    if type(results["schema_version"]) is not int or results["schema_version"] != SCHEMA_VERSION:
        raise ReportError(f"results schema_version must be {SCHEMA_VERSION}")
    if results["catalog_sha256"] != catalog["catalog_sha256"]:
        raise ReportError("results catalog_sha256 does not match the frozen catalog")

    metadata = _object(
        results["metadata"],
        "results.metadata",
        {"musehub_version", "git_commit", "agent_model", "environment", "started_at", "finished_at"},
        {"musehub_version", "git_commit", "agent_model", "environment", "started_at", "finished_at"},
    )
    if not isinstance(metadata["musehub_version"], str) or not APP_VERSION.fullmatch(metadata["musehub_version"]):
        raise ReportError("metadata.musehub_version must be a SemVer version with optional -rcN")
    if not isinstance(metadata["git_commit"], str) or not COMMIT_OID.fullmatch(metadata["git_commit"]):
        raise ReportError("metadata.git_commit must be a 40- or 64-character commit OID")
    if not isinstance(metadata["agent_model"], str) or not ENVIRONMENT.fullmatch(metadata["agent_model"]):
        raise ReportError("metadata.agent_model must be a safe model label")
    if not isinstance(metadata["environment"], str) or not ENVIRONMENT.fullmatch(metadata["environment"]):
        raise ReportError("metadata.environment must be a safe environment label")
    started = _timestamp(metadata["started_at"], "metadata.started_at")
    finished = _timestamp(metadata["finished_at"], "metadata.finished_at")
    if finished < started:
        raise ReportError("metadata.finished_at must not be earlier than started_at")

    raw_tasks = results["tasks"]
    if not isinstance(raw_tasks, list) or len(raw_tasks) != TASK_COUNT:
        raise ReportError(f"results must contain exactly {TASK_COUNT} task records")
    catalog_by_id = {task["task_id"]: task for task in catalog["tasks"]}
    normalized: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, raw_task in enumerate(raw_tasks, start=1):
        task = _object(
            raw_task,
            f"results.tasks[{index}]",
            {"task_id", "status", "pr_id", "commit_oid", "ci_result", "human_accepted", "rework_minutes", "cost_usd", "cost_is_estimate"},
            {"task_id", "status", "pr_id", "commit_oid", "ci_result", "human_accepted", "rework_minutes", "cost_usd", "cost_is_estimate"},
        )
        task_id = task["task_id"]
        if not isinstance(task_id, str) or task_id not in catalog_by_id:
            raise ReportError(f"results.tasks[{index}] references an unknown task_id")
        if task_id in seen:
            raise ReportError(f"duplicate result task_id: {task_id}")
        seen.add(task_id)
        status = task["status"]
        if not isinstance(status, str) or status not in STATUSES:
            raise ReportError(f"task {task_id} status must be one of {', '.join(sorted(STATUSES))}")
        pr_id = task["pr_id"]
        if pr_id is not None and (isinstance(pr_id, bool) or not isinstance(pr_id, int) or pr_id < 1):
            raise ReportError(f"task {task_id} pr_id must be a positive integer or null")
        commit_oid = task["commit_oid"]
        if commit_oid is not None and (not isinstance(commit_oid, str) or not COMMIT_OID.fullmatch(commit_oid)):
            raise ReportError(f"task {task_id} commit_oid must be a 40- or 64-character OID or null")
        ci_result = task["ci_result"]
        if not isinstance(ci_result, str) or ci_result not in CI_RESULTS:
            raise ReportError(f"task {task_id} ci_result must be one of {', '.join(sorted(CI_RESULTS))}")
        human_accepted = task["human_accepted"]
        if not isinstance(human_accepted, bool):
            raise ReportError(f"task {task_id} human_accepted must be boolean")
        rework = None if task["rework_minutes"] is None else _decimal(task["rework_minutes"], f"task {task_id} rework_minutes")
        raw_cost = task["cost_usd"]
        if raw_cost is None:
            cost = None
        elif isinstance(raw_cost, str) and COST.fullmatch(raw_cost):
            cost = Decimal(raw_cost)
        else:
            raise ReportError(f"task {task_id} cost_usd must be a decimal string or null")
        if not isinstance(task["cost_is_estimate"], bool):
            raise ReportError(f"task {task_id} cost_is_estimate must be boolean")
        if raw_cost is None and task["cost_is_estimate"]:
            raise ReportError(f"task {task_id} cannot mark a missing cost as estimated")

        effective_pr = bool(
            status == "completed"
            and human_accepted
            and pr_id is not None
            and commit_oid is not None
            and ci_result == "passed"
        )
        normalized.append(
            {
                "task_id": task_id,
                "category": catalog_by_id[task_id]["category"],
                "pilot_repo": catalog_by_id[task_id]["pilot_repo"],
                "status": status,
                "effective_pr": effective_pr,
                "rework_minutes": None if rework is None else _format_decimal(rework),
                "cost_usd": None if cost is None else _format_cost(cost),
                "cost_is_estimate": task["cost_is_estimate"],
            }
        )

    if seen != set(catalog_by_id):
        missing = sorted(set(catalog_by_id) - seen)
        raise ReportError(f"results are missing task records: {', '.join(missing)}")

    raw_defects = results["defects"]
    if not isinstance(raw_defects, list):
        raise ReportError("results.defects must be a list")
    defects: list[dict[str, str]] = []
    defect_ids: set[str] = set()
    for index, raw_defect in enumerate(raw_defects, start=1):
        defect = _object(
            raw_defect,
            f"results.defects[{index}]",
            {"defect_id", "task_id", "severity", "summary", "evidence"},
            {"defect_id", "task_id", "severity", "summary", "evidence"},
        )
        defect_id = defect["defect_id"]
        task_id = defect["task_id"]
        severity = defect["severity"]
        if not isinstance(defect_id, str) or not TASK_SET_ID.fullmatch(defect_id):
            raise ReportError(f"results.defects[{index}].defect_id is invalid")
        if defect_id in defect_ids:
            raise ReportError(f"duplicate defect_id: {defect_id}")
        if not isinstance(task_id, str) or task_id not in catalog_by_id:
            raise ReportError(f"results.defects[{index}] references an unknown task_id")
        if not isinstance(severity, str) or severity not in DEFECT_SEVERITIES:
            raise ReportError(f"defect {defect_id} severity must be one of {', '.join(sorted(DEFECT_SEVERITIES))}")
        summary = defect["summary"]
        evidence = defect["evidence"]
        if not isinstance(summary, str) or not summary.strip():
            raise ReportError(f"defect {defect_id} summary must be non-empty text")
        if not isinstance(evidence, str) or not evidence.strip():
            raise ReportError(f"defect {defect_id} evidence must be a non-empty reference")
        defect_ids.add(defect_id)
        defects.append(
            {
                "defect_id": defect_id,
                "task_id": task_id,
                "severity": severity,
                "summary": summary.strip(),
                "evidence": evidence.strip(),
            }
        )

    release = _object(
        results["release_decision"],
        "results.release_decision",
        {"decision", "reviewer", "decided_at", "rationale"},
        {"decision", "reviewer", "decided_at", "rationale"},
    )
    if not isinstance(release["decision"], str) or release["decision"] not in RELEASE_DECISIONS:
        raise ReportError(f"release_decision.decision must be one of {', '.join(sorted(RELEASE_DECISIONS))}")
    if not isinstance(release["reviewer"], str) or not ENVIRONMENT.fullmatch(release["reviewer"]):
        raise ReportError("release_decision.reviewer must be a safe reviewer label")
    decided_at = _timestamp(release["decided_at"], "release_decision.decided_at")
    if decided_at < finished:
        raise ReportError("release_decision.decided_at must not be earlier than metadata.finished_at")
    if not isinstance(release["rationale"], str) or not release["rationale"].strip():
        raise ReportError("release_decision.rationale must be non-empty text")
    release_decision = {
        "decision": release["decision"],
        "reviewer": release["reviewer"],
        "decided_at": release["decided_at"],
        "rationale": release["rationale"].strip(),
    }

    by_category: dict[str, dict[str, Any]] = {}
    for category in sorted(CATEGORIES):
        group = [task for task in normalized if task["category"] == category]
        by_category[category] = {
            "task_count": len(group),
            "completed_count": sum(task["status"] == "completed" for task in group),
            "effective_pr_count": sum(task["effective_pr"] for task in group),
        }
    by_repo: dict[str, dict[str, Any]] = {}
    for pilot_repo in sorted({task["pilot_repo"] for task in normalized}):
        group = [task for task in normalized if task["pilot_repo"] == pilot_repo]
        by_repo[pilot_repo] = {
            "task_count": len(group),
            "completed_count": sum(task["status"] == "completed" for task in group),
            "effective_pr_count": sum(task["effective_pr"] for task in group),
        }

    known_rework = [Decimal(task["rework_minutes"]) for task in normalized if task["rework_minutes"] is not None]
    known_cost = [Decimal(task["cost_usd"]) for task in normalized if task["cost_usd"] is not None]
    total_cost = sum(known_cost, Decimal(0))
    estimated_cost = sum(
        (Decimal(task["cost_usd"]) for task in normalized if task["cost_is_estimate"]),
        Decimal(0),
    )
    completed_count = sum(task["status"] == "completed" for task in normalized)
    effective_pr_count = sum(task["effective_pr"] for task in normalized)
    return {
        "schema_version": SCHEMA_VERSION,
        "task_set_id": catalog["task_set_id"],
        "catalog_sha256": catalog["catalog_sha256"],
        "metadata": metadata,
        "metrics": {
            "task_count": TASK_COUNT,
            "completed_count": completed_count,
            "completion_rate": _format_decimal(Decimal(completed_count) * 100 / TASK_COUNT) + "%",
            "effective_pr_count": effective_pr_count,
            "effective_pr_rate": _format_decimal(Decimal(effective_pr_count) * 100 / TASK_COUNT) + "%",
            "rework_recorded_count": len(known_rework),
            "rework_total_minutes": _format_decimal(sum(known_rework, Decimal(0))),
            "rework_average_minutes": None if not known_rework else _format_decimal(sum(known_rework, Decimal(0)) / len(known_rework)),
            "cost_recorded_count": len(known_cost),
            "cost_total_usd": _format_cost(total_cost),
            "cost_estimated_total_usd": _format_cost(estimated_cost),
            "by_category": by_category,
            "by_repo": by_repo,
        },
        "tasks": sorted(normalized, key=lambda task: task["task_id"]),
        "defects": sorted(defects, key=lambda defect: defect["defect_id"]),
        "release_decision": release_decision,
    }


def _markdown_cell(value: str) -> str:
    return (
        value.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("|", "&#124;")
        .replace("\r", " ")
        .replace("\n", " ")
    )


def render_markdown(report: dict[str, Any]) -> str:
    metrics = report["metrics"]
    metadata = report["metadata"]
    lines = [
        f"# PILOT-01 evaluation: {report['task_set_id']}",
        "",
        f"- Catalog SHA-256: `{report['catalog_sha256']}`",
        f"- MuseHub version: `{metadata['musehub_version']}`",
        f"- Agent model: `{metadata['agent_model']}`",
        f"- Commit: `{metadata['git_commit']}`",
        f"- Environment: `{metadata['environment']}`",
        f"- Run window: `{metadata['started_at']}` to `{metadata['finished_at']}`",
        "",
        "## Metrics",
        "",
        "| Measure | Result | Coverage |",
        "|---|---:|---:|",
        f"| Completed tasks | {metrics['completed_count']}/{TASK_COUNT} ({metrics['completion_rate']}) | {TASK_COUNT}/{TASK_COUNT} |",
        f"| Effective PRs | {metrics['effective_pr_count']}/{TASK_COUNT} ({metrics['effective_pr_rate']}) | {TASK_COUNT}/{TASK_COUNT} |",
        f"| Human rework | {metrics['rework_total_minutes']} min total; {metrics['rework_average_minutes'] or 'n/a'} min average | {metrics['rework_recorded_count']}/{TASK_COUNT} |",
        f"| Cost | ${metrics['cost_total_usd']} USD; ${metrics['cost_estimated_total_usd']} estimated | {metrics['cost_recorded_count']}/{TASK_COUNT} |",
        "",
        "## Category breakdown",
        "",
        "| Category | Tasks | Completed | Effective PRs |",
        "|---|---:|---:|---:|",
    ]
    for category, data in metrics["by_category"].items():
        lines.append(f"| {category} | {data['task_count']} | {data['completed_count']} | {data['effective_pr_count']} |")
    lines.extend(
        [
            "",
            "## Pilot repository breakdown",
            "",
            "| Repository alias | Tasks | Completed | Effective PRs |",
            "|---|---:|---:|---:|",
        ]
    )
    for pilot_repo, data in metrics["by_repo"].items():
        lines.append(f"| {pilot_repo} | {data['task_count']} | {data['completed_count']} | {data['effective_pr_count']} |")
    lines.extend(["", "## Defect inventory", ""])
    if report["defects"]:
        lines.extend(
            [
                "| Defect | Task | Severity | Summary | Evidence reference |",
                "|---|---|---|---|---|",
            ]
        )
        for defect in report["defects"]:
            lines.append(
                "| {defect_id} | {task_id} | {severity} | {summary} | {evidence} |".format(
                    defect_id=_markdown_cell(defect["defect_id"]),
                    task_id=_markdown_cell(defect["task_id"]),
                    severity=_markdown_cell(defect["severity"]),
                    summary=_markdown_cell(defect["summary"]),
                    evidence=_markdown_cell(defect["evidence"]),
                )
            )
    else:
        lines.append("No defects recorded.")
    decision = report["release_decision"]
    lines.extend(
        [
            "",
            "## Human release decision",
            "",
            f"- Decision: `{_markdown_cell(decision['decision'])}`",
            f"- Reviewer label: `{_markdown_cell(decision['reviewer'])}`",
            f"- Recorded at: `{_markdown_cell(decision['decided_at'])}`",
            f"- Rationale: {_markdown_cell(decision['rationale'])}",
            "",
            "An effective PR is counted only when the task is completed, a human marked the result accepted, a PR ID and commit OID are recorded, and CI passed.",
            "Missing rework or cost values are excluded from their totals and reflected in coverage; no target success rate is implied.",
            "Defects and the release decision are human-entered evidence; this report does not infer a release outcome.",
            "",
        ]
    )
    return "\n".join(lines)


def ensure_outputs_new(paths: list[Path]) -> None:
    existing = [path for path in paths if path.exists()]
    if existing:
        outputs = ", ".join(str(path) for path in existing)
        raise ReportError(f"refusing to overwrite existing output: {outputs}")


def write_new(path: Path, text: str) -> None:
    try:
        with path.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
    except FileExistsError as error:
        raise ReportError(f"refusing to overwrite existing output: {path}") from error
    except OSError as error:
        raise ReportError(f"cannot write {path}: {error}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    freeze = commands.add_parser("freeze", help="validate and freeze a 20-task catalog")
    freeze.add_argument("catalog", type=Path)
    freeze.add_argument("output", type=Path)
    template = commands.add_parser("template", help="create a blank, non-evidence results-entry template")
    template.add_argument("catalog_lock", type=Path)
    template.add_argument("output", type=Path)
    report = commands.add_parser("report", help="validate pilot results and generate JSON/Markdown")
    report.add_argument("catalog_lock", type=Path)
    report.add_argument("results", type=Path)
    report.add_argument("--json", dest="json_output", type=Path, required=True)
    report.add_argument("--markdown", dest="markdown_output", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "freeze":
            frozen = freeze_catalog(_read_json(args.catalog))
            write_new(args.output, json.dumps(frozen, ensure_ascii=False, indent=2) + "\n")
            print(f"pilot_catalog_frozen={frozen['catalog_sha256']}")
            return 0
        if args.command == "template":
            template_results = initialize_results(_read_json(args.catalog_lock))
            write_new(args.output, json.dumps(template_results, ensure_ascii=False, indent=2) + "\n")
            print(
                f"pilot_results_template_initialized tasks={TASK_COUNT} "
                f"catalog_sha256={template_results['catalog_sha256']} evidence=none"
            )
            return 0
        if args.json_output.resolve() == args.markdown_output.resolve():
            raise ReportError("JSON and Markdown outputs must be different files")
        ensure_outputs_new([args.json_output, args.markdown_output])
        frozen = load_frozen_catalog(_read_json(args.catalog_lock))
        generated = build_report(frozen, _read_json(args.results))
        json_text = json.dumps(generated, ensure_ascii=False, indent=2) + "\n"
        markdown_text = render_markdown(generated)
        write_new(args.json_output, json_text)
        write_new(args.markdown_output, markdown_text)
        print(f"pilot_report_generated tasks={TASK_COUNT} catalog_sha256={frozen['catalog_sha256']}")
        return 0
    except ReportError as error:
        print(f"pilot_report_error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
