#!/usr/bin/env python3
"""Measure a fixed-rate, read-only workload against MuseHub's Issue list API."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import os
import platform
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_API_ORIGIN = "http://127.0.0.1:8080"
DEFAULT_ORG = "demo"
DEFAULT_RPS = 100
DEFAULT_DURATION_SECONDS = 60
DEFAULT_WARMUP_REQUESTS = 200
DEFAULT_TIMEOUT_SECONDS = 10
DEFAULT_MAX_WORKERS = 200
DEFAULT_MIN_MEMBERS = 50
DEFAULT_MIN_REPOSITORIES = 100
ISSUE_PAGE_LIMIT = 50
MIN_SCHEDULER_CONCURRENCY = 10


class BenchmarkError(RuntimeError):
    """A safe-to-display benchmark setup error."""


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Keep session cookies and benchmark credentials on the configured origin."""

    def redirect_request(self, request: urllib.request.Request, response: Any, code: int, message: str, headers: Any, new_url: str) -> None:
        return None


API_OPENER = urllib.request.build_opener(NoRedirectHandler)


def percentile(values: list[float], percent: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, math.ceil(percent / 100 * len(ordered)) - 1)
    return ordered[index]


def request_json(
    url: str,
    *,
    method: str = "GET",
    body: dict[str, Any] | None = None,
    cookie: str | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
) -> tuple[int, Any, str | None]:
    headers = {"Accept": "application/json"}
    payload = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        payload = json.dumps(body).encode("utf-8")
    if cookie:
        headers["Cookie"] = cookie
    request = urllib.request.Request(url, data=payload, headers=headers, method=method)
    try:
        with API_OPENER.open(request, timeout=timeout) as response:
            raw = response.read()
            parsed = json.loads(raw) if raw else None
            return response.status, parsed, response.headers.get("Set-Cookie")
    except urllib.error.HTTPError as error:
        return error.code, None, error.headers.get("Set-Cookie")
    except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"request failed ({type(error).__name__})") from None


def api_url(origin: str, path: str) -> str:
    return f"{origin.rstrip('/')}{path}"


def authenticate(origin: str, handle: str, credential: str, timeout: float) -> str:
    status, _body, set_cookie = request_json(
        api_url(origin, "/api/v1/auth/login"),
        method="POST",
        body={"handle": handle, "password": credential},
        timeout=timeout,
    )
    if status != 200 or not set_cookie:
        raise BenchmarkError(f"login failed (HTTP {status})")
    cookie_pair = set_cookie.split(";", 1)[0]
    if "=" not in cookie_pair:
        raise BenchmarkError("login response did not contain a session cookie")
    return cookie_pair


def get_json(origin: str, path: str, cookie: str, timeout: float) -> Any:
    status, body, _ = request_json(api_url(origin, path), cookie=cookie, timeout=timeout)
    if status != 200:
        raise BenchmarkError(f"dataset preflight failed (HTTP {status})")
    return body


def perform_get(url: str, cookie: str, timeout: float) -> dict[str, Any]:
    started = time.perf_counter()
    try:
        status, _body, _ = request_json(url, cookie=cookie, timeout=timeout)
        return {"latency_ms": (time.perf_counter() - started) * 1000, "status": status}
    except BenchmarkError as error:
        return {
            "latency_ms": (time.perf_counter() - started) * 1000,
            "status": None,
            "error": str(error),
        }


def run_fixed_rate(
    url: str,
    cookie: str,
    *,
    rps: int,
    duration_seconds: int,
    max_workers: int,
    timeout: float,
) -> tuple[list[dict[str, Any]], float]:
    total_requests = rps * duration_seconds
    results: list[dict[str, Any]] = []
    started = time.perf_counter()
    next_due = started
    interval = 1 / rps
    inflight: set[concurrent.futures.Future[dict[str, Any]]] = set()
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        for _ in range(total_requests):
            now = time.perf_counter()
            if now < next_due:
                time.sleep(next_due - now)
            while len(inflight) >= max_workers:
                done, inflight = concurrent.futures.wait(
                    inflight, return_when=concurrent.futures.FIRST_COMPLETED
                )
                results.extend(future.result() for future in done)
            request_started = time.perf_counter()
            inflight.add(executor.submit(perform_get, url, cookie, timeout))
            next_due = max(next_due + interval, request_started + interval)
        if inflight:
            done, _ = concurrent.futures.wait(inflight)
            results.extend(future.result() for future in done)
    elapsed = time.perf_counter() - started
    return results, elapsed


def run_scheduler_probe(
    origin: str,
    org: str,
    repo: str,
    source_oid: str,
    cookie: str,
    *,
    samples: int,
    poll_interval_ms: int,
    queue_timeout: float,
    completion_timeout: float,
    request_timeout: float,
) -> dict[str, Any]:
    org_path = urllib.parse.quote(org, safe="")
    repo_path = urllib.parse.quote(repo, safe="")
    run_path = f"/api/v1/orgs/{org_path}/repos/{repo_path}/workflow-runs"
    measurements: list[dict[str, Any]] = []
    terminal_states = {"succeeded", "failed", "canceled"}

    def request_cancel(record: dict[str, Any]) -> None:
        request_json(
            api_url(origin, str(record["detail_path"]) + "/cancel"),
            method="POST",
            cookie=cookie,
            timeout=request_timeout,
        )
        record["cancellation_requested"] = True

    for sample_index in range(samples):
        try:
            status, created, _ = request_json(
                api_url(origin, run_path),
                method="POST",
                body={"source_oid": source_oid},
                cookie=cookie,
                timeout=request_timeout,
            )
        except BenchmarkError:
            for record in measurements:
                if record["state"] not in terminal_states and not record["cancellation_requested"]:
                    request_cancel(record)
            raise
        if status != 201 or not isinstance(created, dict) or not isinstance(created.get("id"), str):
            for record in measurements:
                if record["state"] not in terminal_states and not record["cancellation_requested"]:
                    request_cancel(record)
            raise BenchmarkError(f"scheduler probe enqueue failed (sample={sample_index + 1}, HTTP {status})")
        enqueued_at = time.perf_counter()
        run_id = created["id"]
        measurements.append(
            {
                "sample": sample_index + 1,
                "run_id": run_id,
                "detail_path": f"/api/v1/orgs/{org_path}/repos/{repo_path}/pipeline-runs/{run_id}",
                "enqueued_at": enqueued_at,
                "queue_deadline": enqueued_at + queue_timeout,
                "completion_deadline": enqueued_at + completion_timeout,
                "state": created.get("state"),
                "observed_start_latency_ms": None,
                "cancellation_requested": False,
                "completion_timed_out": False,
            }
        )

    peak_observed_concurrency = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=samples) as executor:
        while any(record["state"] not in terminal_states and not record["completion_timed_out"] for record in measurements):
            now = time.perf_counter()
            pending = [
                record for record in measurements
                if record["state"] not in terminal_states and not record["completion_timed_out"]
            ]
            futures = {
                executor.submit(
                    request_json,
                    api_url(origin, str(record["detail_path"])),
                    cookie=cookie,
                    timeout=request_timeout,
                ): record
                for record in pending
            }
            for future in concurrent.futures.as_completed(futures):
                record = futures[future]
                try:
                    status, detail, _ = future.result()
                except BenchmarkError as error:
                    for active in measurements:
                        if active["state"] not in terminal_states and not active["cancellation_requested"]:
                            request_cancel(active)
                    raise BenchmarkError(f"scheduler probe polling failed (sample={record['sample']}): {error}") from None
                if status != 200 or not isinstance(detail, dict):
                    for active in measurements:
                        if active["state"] not in terminal_states and not active["cancellation_requested"]:
                            request_cancel(active)
                    raise BenchmarkError(f"scheduler probe polling failed (sample={record['sample']}, HTTP {status})")
                record["state"] = detail.get("state")
                if record["state"] == "running" and record["observed_start_latency_ms"] is None:
                    record["observed_start_latency_ms"] = round((time.perf_counter() - record["enqueued_at"]) * 1000, 3)

            observed_running = sum(record["state"] == "running" for record in measurements)
            peak_observed_concurrency = max(peak_observed_concurrency, observed_running)
            now = time.perf_counter()
            for record in measurements:
                if record["state"] in terminal_states:
                    continue
                if now >= record["completion_deadline"]:
                    if not record["cancellation_requested"]:
                        request_cancel(record)
                    record["completion_timed_out"] = True
                elif now >= record["queue_deadline"] and record["observed_start_latency_ms"] is None and not record["cancellation_requested"]:
                    request_cancel(record)
            if any(record["state"] not in terminal_states and not record["completion_timed_out"] for record in measurements):
                time.sleep(poll_interval_ms / 1000)

    latencies = [
        float(item["observed_start_latency_ms"])
        for item in measurements
        if item["observed_start_latency_ms"] is not None
    ]
    all_runs_succeeded = all(item["state"] == "succeeded" for item in measurements)
    concurrency_target_met = peak_observed_concurrency >= samples
    scheduler_p95_under_5s = percentile(latencies, 95) is not None and percentile(latencies, 95) < 5000
    return {
        "repository": repo,
        "samples": samples,
        "requested_concurrency": samples,
        "peak_observed_concurrency": peak_observed_concurrency,
        "concurrency_target_met": concurrency_target_met,
        "poll_interval_ms": poll_interval_ms,
        "queue_timeout_seconds": queue_timeout,
        "completion_timeout_seconds": completion_timeout,
        "measurements": [
            {
                "sample": record["sample"],
                "run_id": record["run_id"],
                "observed_start_latency_ms": record["observed_start_latency_ms"],
                "final_state": record["state"],
                "start_observed": record["observed_start_latency_ms"] is not None,
                "cancellation_requested": record["cancellation_requested"],
                "completion_timed_out": record["completion_timed_out"],
            }
            for record in measurements
        ],
        "observed_start_latency_ms": {
            "p50": round(percentile(latencies, 50), 3) if latencies else None,
            "p95": round(percentile(latencies, 95), 3) if latencies else None,
            "max": round(max(latencies), 3) if latencies else None,
        },
        "all_starts_observed": len(latencies) == samples,
        "scheduler_p95_under_5s": scheduler_p95_under_5s,
        "all_runs_succeeded": all_runs_succeeded,
        "passed": len(latencies) == samples
        and scheduler_p95_under_5s
        and concurrency_target_met
        and all_runs_succeeded,
    }


def summarize(results: list[dict[str, Any]], elapsed: float) -> dict[str, Any]:
    latencies = [item["latency_ms"] for item in results]
    statuses = Counter(str(item["status"]) if item["status"] is not None else "network_error" for item in results)
    successes = sum(item["status"] == 200 for item in results)
    errors = len(results) - successes
    return {
        "requests": len(results),
        "successful_requests": successes,
        "failed_requests": errors,
        "error_rate": errors / len(results) if results else 1.0,
        "status_counts": dict(sorted(statuses.items())),
        "elapsed_seconds": round(elapsed, 3),
        "achieved_rps": round(len(results) / elapsed, 2) if elapsed else 0.0,
        "latency_ms": {
            "min": round(min(latencies), 3) if latencies else None,
            "mean": round(statistics.fmean(latencies), 3) if latencies else None,
            "p50": round(percentile(latencies, 50), 3) if latencies else None,
            "p95": round(percentile(latencies, 95), 3) if latencies else None,
            "p99": round(percentile(latencies, 99), 3) if latencies else None,
            "max": round(max(latencies), 3) if latencies else None,
        },
    }


def markdown_report(report: dict[str, Any]) -> str:
    metrics = report["measurement"]
    latency = metrics["latency_ms"]
    conditions = report["conditions"]
    dataset = report["dataset"]
    passed = report["target_evaluation"]["passed"]
    return "\n".join(
        [
            "# MuseHub API performance measurement",
            "",
            f"- Result: **{'PASS' if passed else 'FAIL'}**",
            f"- Started (UTC): `{report['started_at_utc']}`",
            f"- Target: `{report['target']}`",
            f"- Workload: `{conditions['target_rps']} RPS` for `{conditions['duration_seconds']} s`; `{conditions['warmup_requests']}` warmup requests",
            f"- Dataset observed through API: `{dataset['active_members']} active members`, `{dataset['repositories']} repositories`",
            f"- Endpoint: `GET {report['endpoint_path']}`",
            "",
            "## Results",
            "",
            "| Metric | Result |",
            "|---|---:|",
            f"| Requests | {metrics['requests']} |",
            f"| Successful | {metrics['successful_requests']} |",
            f"| Failed | {metrics['failed_requests']} |",
            f"| Error rate | {metrics['error_rate']:.4%} |",
            f"| Achieved throughput | {metrics['achieved_rps']} RPS |",
            f"| p50 | {latency['p50']} ms |",
            f"| p95 | {latency['p95']} ms |",
            f"| p99 | {latency['p99']} ms |",
            f"| Max | {latency['max']} ms |",
            "",
            *(
                [
                    "## Runner scheduler probe",
                    "",
                    f"- Result: **{'PASS' if report['scheduler_measurement']['passed'] else 'FAIL'}**",
                    f"- Scheduler p95 under 5 seconds: `{report['scheduler_measurement']['scheduler_p95_under_5s']}`; observed concurrency target met: `{report['scheduler_measurement']['concurrency_target_met']}`",
                    f"- Runs: `{report['scheduler_measurement']['samples']}` manual workflow jobs rapidly enqueued in repository `{report['scheduler_measurement']['repository']}`",
                    f"- Observed concurrent jobs: peak `{report['scheduler_measurement']['peak_observed_concurrency']}` / target `{report['scheduler_measurement']['requested_concurrency']}`",
                    f"- Observed start latency: p50 `{report['scheduler_measurement']['observed_start_latency_ms']['p50']}` ms, p95 `{report['scheduler_measurement']['observed_start_latency_ms']['p95']}` ms, max `{report['scheduler_measurement']['observed_start_latency_ms']['max']}` ms",
                    f"- Poll interval: `{report['scheduler_measurement']['poll_interval_ms']}` ms; first observed `running` is an upper-bound estimate rather than the exact worker start time.",
                    f"- All starts observed: `{report['scheduler_measurement']['all_starts_observed']}`; all runs succeeded: `{report['scheduler_measurement']['all_runs_succeeded']}`",
                    f"- Completion timeouts: `{sum(item['completion_timed_out'] for item in report['scheduler_measurement']['measurements'])}`; cancellation was requested where a run had not reached a terminal state.",
                    "",
                ]
                if report.get("scheduler_measurement") is not None
                else []
            ),
            "## Conditions and limits",

            "",
            f"- Benchmark client: `{report['client_host']['platform']}`, `{report['client_host']['cpu_count']}` logical CPUs; this describes the client host, not necessarily the control plane.",
            f"- Declared control-plane profile: `{conditions['control_plane_vcpu'] or 'not supplied'} vCPU`, `{conditions['control_plane_memory_gib'] or 'not supplied'} GiB`, SSD `{conditions['ssd_declared']}`. These values are operator declarations and are not remotely verified.",
            f"- Declared concurrent jobs: `{conditions['concurrent_jobs_declared']}`; the read-only API workload does not start jobs, while the optional scheduler probe reports observed concurrent runs separately.",
            "- Requests use an authenticated, read-only Issue list endpoint. Git, search, and large-diff paths are excluded.",
            "- This result is one environment's measurement, not a general performance guarantee.",
            "",
        ]
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", default=os.getenv("MUSEHUB_PERF_API_ORIGIN", DEFAULT_API_ORIGIN))
    parser.add_argument("--org", default=os.getenv("MUSEHUB_PERF_ORG", DEFAULT_ORG))
    parser.add_argument("--repo", default=os.getenv("MUSEHUB_PERF_REPO"), help="repo slug; defaults to the first visible repository")
    parser.add_argument("--handle", default=os.getenv("MUSEHUB_PERF_HANDLE", "admin"))
    parser.add_argument("--rps", type=int, default=DEFAULT_RPS)
    parser.add_argument("--duration-seconds", type=int, default=DEFAULT_DURATION_SECONDS)
    parser.add_argument("--warmup-requests", type=int, default=DEFAULT_WARMUP_REQUESTS)
    parser.add_argument("--max-workers", type=int, default=DEFAULT_MAX_WORKERS)
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--min-members", type=int, default=DEFAULT_MIN_MEMBERS)
    parser.add_argument("--min-repositories", type=int, default=DEFAULT_MIN_REPOSITORIES)
    parser.add_argument("--control-plane-vcpu", type=int)
    parser.add_argument("--control-plane-memory-gib", type=float)
    parser.add_argument("--ssd-declared", choices=("yes", "no", "unknown"), default="unknown")
    parser.add_argument("--concurrent-jobs-declared", type=int, default=10)
    parser.add_argument("--scheduler-repo", help="dedicated repo containing a safe manual workflow; runs create persistent pipeline records")
    parser.add_argument("--scheduler-source-oid", help="commit OID containing the manual workflow to execute")
    parser.add_argument("--scheduler-samples", type=int, default=MIN_SCHEDULER_CONCURRENCY, help="jobs enqueued in one batch; must be at least 10")
    parser.add_argument("--scheduler-poll-ms", type=int, default=100)
    parser.add_argument("--scheduler-queue-timeout-seconds", type=float, default=30)
    parser.add_argument("--scheduler-completion-timeout-seconds", type=float, default=300)
    parser.add_argument("--output-prefix", required=True, help="path prefix for .json and .md output files")
    args = parser.parse_args()
    if not args.origin.startswith(("http://", "https://")):
        parser.error("--origin must use http:// or https://")
    for name in ("rps", "duration_seconds", "warmup_requests", "max_workers", "min_members", "min_repositories", "concurrent_jobs_declared", "scheduler_samples"):
        if getattr(args, name) < 1:
            parser.error(f"--{name.replace('_', '-')} must be at least 1")
    if (args.scheduler_repo is None) != (args.scheduler_source_oid is None):
        parser.error("--scheduler-repo and --scheduler-source-oid must be provided together")
    if args.scheduler_repo and args.scheduler_samples < MIN_SCHEDULER_CONCURRENCY:
        parser.error(f"--scheduler-samples must be at least {MIN_SCHEDULER_CONCURRENCY} when scheduler probing is enabled")
    if args.scheduler_poll_ms < 10:
        parser.error("--scheduler-poll-ms must be at least 10")
    if args.scheduler_queue_timeout_seconds <= 0 or args.scheduler_completion_timeout_seconds <= 0:
        parser.error("scheduler timeouts must be positive")
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    if args.control_plane_vcpu is not None and args.control_plane_vcpu < 1:
        parser.error("--control-plane-vcpu must be positive")
    if args.control_plane_memory_gib is not None and args.control_plane_memory_gib <= 0:
        parser.error("--control-plane-memory-gib must be positive")
    return args


def main() -> int:
    args = parse_args()
    secret_value = os.getenv("MUSEHUB_PERF_PASSWORD")
    if not secret_value:
        raise BenchmarkError("set MUSEHUB_PERF_PASSWORD in the environment; the value is never written to reports")
    if args.scheduler_repo and os.getenv("MUSEHUB_PERF_ALLOW_JOB_EXECUTION") != "1":
        raise BenchmarkError("scheduler probing starts real workflow jobs; set MUSEHUB_PERF_ALLOW_JOB_EXECUTION=1 only for a dedicated isolated test repo")
    if args.scheduler_source_oid and (
        len(args.scheduler_source_oid) not in (40, 64)
        or any(char not in "0123456789abcdefABCDEF" for char in args.scheduler_source_oid)
    ):
        raise BenchmarkError("scheduler source OID must be a 40- or 64-character hexadecimal commit ID")
    parsed_origin = urllib.parse.urlsplit(args.origin)
    if parsed_origin.username or parsed_origin.password or parsed_origin.query or parsed_origin.fragment:
        raise BenchmarkError("--origin must not contain credentials, a query, or a fragment")
    origin = urllib.parse.urlunsplit((parsed_origin.scheme, parsed_origin.netloc, parsed_origin.path.rstrip("/"), "", ""))
    started_at = datetime.now(timezone.utc).isoformat()
    cookie = authenticate(origin, args.handle, secret_value, args.timeout_seconds)
    org_path = urllib.parse.quote(args.org, safe="")
    members = get_json(origin, f"/api/v1/orgs/{org_path}/members", cookie, args.timeout_seconds)
    repos = get_json(origin, f"/api/v1/orgs/{org_path}/repos", cookie, args.timeout_seconds)
    if not isinstance(members, list) or not isinstance(repos, list):
        raise BenchmarkError("dataset preflight returned an unexpected response")
    active_members = sum(
        item.get("membership_status") == "active" and item.get("principal_status") == "active"
        for item in members if isinstance(item, dict)
    )
    if active_members < args.min_members or len(repos) < args.min_repositories:
        raise BenchmarkError(
            f"dataset is below required scale (active_members={active_members}/{args.min_members}, repositories={len(repos)}/{args.min_repositories})"
        )
    selected_repo = next((item for item in repos if item.get("slug") == args.repo), None) if args.repo else repos[0]
    if selected_repo is None or not isinstance(selected_repo.get("slug"), str):
        raise BenchmarkError("requested repository is not visible to the authenticated account")
    scheduler_repo = next((item for item in repos if item.get("slug") == args.scheduler_repo), None) if args.scheduler_repo else None
    if args.scheduler_repo and scheduler_repo is None:
        raise BenchmarkError("scheduler repository is not visible to the authenticated account")
    repo_slug = urllib.parse.quote(selected_repo["slug"], safe="")
    endpoint_path = f"/api/v1/orgs/{org_path}/repos/{repo_slug}/issues?limit={ISSUE_PAGE_LIMIT}"
    endpoint_url = api_url(origin, endpoint_path)
    for _ in range(args.warmup_requests):
        status, body, _ = request_json(endpoint_url, cookie=cookie, timeout=args.timeout_seconds)
        if status != 200 or not isinstance(body, dict) or not isinstance(body.get("items"), list):
            raise BenchmarkError(f"warmup failed (HTTP {status})")
    results, elapsed = run_fixed_rate(
        endpoint_url,
        cookie,
        rps=args.rps,
        duration_seconds=args.duration_seconds,
        max_workers=args.max_workers,
        timeout=args.timeout_seconds,
    )
    measurement = summarize(results, elapsed)
    scheduler_measurement = None
    if scheduler_repo is not None:
        scheduler_measurement = run_scheduler_probe(
            origin,
            args.org,
            scheduler_repo["slug"],
            args.scheduler_source_oid,
            cookie,
            samples=args.scheduler_samples,
            poll_interval_ms=args.scheduler_poll_ms,
            queue_timeout=args.scheduler_queue_timeout_seconds,
            completion_timeout=args.scheduler_completion_timeout_seconds,
            request_timeout=args.timeout_seconds,
        )
    api_passed = (
        measurement["latency_ms"]["p95"] is not None
        and measurement["latency_ms"]["p95"] < 300
        and measurement["achieved_rps"] >= args.rps * 0.95
        and measurement["failed_requests"] == 0
    )
    passed = api_passed and (scheduler_measurement is None or scheduler_measurement["passed"])
    report = {
        "schema_version": 1,
        "started_at_utc": started_at,
        "target": "docs/01-product-requirements.md §7 API p95 < 300 ms",
        "endpoint_path": endpoint_path,
        "dataset": {
            "organization": args.org,
            "active_members": active_members,
            "repositories": len(repos),
            "selected_repository": selected_repo["slug"],
        },
        "conditions": {
            "target_rps": args.rps,
            "duration_seconds": args.duration_seconds,
            "warmup_requests": args.warmup_requests,
            "max_workers": args.max_workers,
            "timeout_seconds": args.timeout_seconds,
            "min_members_required": args.min_members,
            "min_repositories_required": args.min_repositories,
            "control_plane_vcpu": args.control_plane_vcpu,
            "control_plane_memory_gib": args.control_plane_memory_gib,
            "ssd_declared": args.ssd_declared,
            "concurrent_jobs_declared": args.concurrent_jobs_declared,
        },
        "client_host": {"platform": platform.platform(), "cpu_count": os.cpu_count()},
        "measurement": measurement,
        "scheduler_measurement": scheduler_measurement,
        "target_evaluation": {
            "requested_rps": args.rps,
            "minimum_dataset_scale_met": active_members >= args.min_members and len(repos) >= args.min_repositories,
            "api_p95_under_300ms": measurement["latency_ms"]["p95"] is not None and measurement["latency_ms"]["p95"] < 300,
            "achieved_rps_at_least_95_percent_of_target": measurement["achieved_rps"] >= args.rps * 0.95,
            "zero_failed_requests": measurement["failed_requests"] == 0,
            "scheduler_probe_requested": scheduler_measurement is not None,
            "scheduler_p95_under_5s": scheduler_measurement["scheduler_p95_under_5s"] if scheduler_measurement is not None else None,
            "scheduler_concurrency_target_met": scheduler_measurement["concurrency_target_met"] if scheduler_measurement is not None else None,
            "api_passed": api_passed,
            "passed": passed,
        },
        "limitations": [
            *(
                [
                    "Scheduler jobs are rapidly enqueued as one batch and polled concurrently; the observed peak depends on polling and workflow duration, so use a safe manual workflow that remains running long enough to observe overlap.",
                ]
                if scheduler_measurement is not None
                else ["Runner scheduling latency is not measured unless the scheduler probe is explicitly requested."]
            ),
            "Control-plane CPU, memory, SSD, network locality, and database warm-cache state are not remotely verified.",
            "No endpoint URL, credentials, cookies, or response bodies are written to the report.",
        ],
    }
    prefix = Path(args.output_prefix)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    json_path = prefix.with_suffix(".json")
    markdown_path = prefix.with_suffix(".md")
    if json_path.exists() or markdown_path.exists():
        raise BenchmarkError("report output already exists; choose a new --output-prefix")
    json_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    markdown_path.write_text(markdown_report(report), encoding="utf-8")
    print(f"benchmark={'pass' if report['target_evaluation']['passed'] else 'fail'}")
    print(f"api_measurement={'pass' if api_passed else 'fail'} requests={measurement['requests']} failed={measurement['failed_requests']} achieved_rps={measurement['achieved_rps']} p95_ms={measurement['latency_ms']['p95']}")
    if scheduler_measurement is not None:
        print(f"scheduler_measurement={'pass' if scheduler_measurement['passed'] else 'fail'} jobs={scheduler_measurement['samples']} peak_concurrency={scheduler_measurement['peak_observed_concurrency']} p95_ms={scheduler_measurement['observed_start_latency_ms']['p95']}")
    print(f"json_report={json_path} markdown_report={markdown_path}")
    return 0 if report["target_evaluation"]["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BenchmarkError as error:
        print(f"api_benchmark=error detail={error}", file=sys.stderr)
        raise SystemExit(2)
