"""Deterministic unit tests for the fixed-rate pacing in ``benchmark-api.py``.

The module under test has a hyphenated filename, so it is loaded by path with
``importlib``.  No test performs network I/O or a real ``time.sleep``: every
test drives the scheduler with a fake clock whose ``sleep`` advances virtual
time by the requested duration plus a fixed overshoot.  That makes the
accumulation behaviour of the submission schedule reproducible.
"""

from __future__ import annotations

import importlib.util
import inspect
import unittest
from pathlib import Path

_MODULE_PATH = Path(__file__).with_name("benchmark-api.py")
_SPEC = importlib.util.spec_from_file_location("benchmark_api", _MODULE_PATH)
if _SPEC is None or _SPEC.loader is None:  # pragma: no cover - import wiring guard
    raise ImportError(f"cannot load benchmark module from {_MODULE_PATH}")
benchmark_api = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(benchmark_api)

URL = "http://127.0.0.1:8080/api/v1/issues?limit=50"
COOKIE = "session=unit-test"
RPS = 100
INTERVAL = 1 / RPS
OVERSHOOT = 0.002


def synthetic_perform(url: str, cookie: str, timeout: float) -> dict[str, object]:
    """Stand-in for ``perform_get``: instant, synthetic, never touches the clock."""
    return {"latency_ms": 1.0, "status": 200}


class FakeClock:
    """Virtual monotonic clock; ``sleep`` advances by duration plus overshoot."""

    def __init__(
        self,
        *,
        overshoot: float = 0.0,
        stall: float = 0.0,
        stall_on_call: int | None = None,
    ) -> None:
        self.now = 0.0
        self.overshoot = overshoot
        self.stall = stall
        self.stall_on_call = stall_on_call
        self.sleeps: list[float] = []

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        if seconds <= 0:
            raise AssertionError(f"non-positive sleep requested: {seconds!r}")
        self.sleeps.append(seconds)
        self.now += seconds + self.overshoot
        if self.stall_on_call is not None and len(self.sleeps) == self.stall_on_call:
            self.now += self.stall

    def drive(
        self,
        *,
        rps: int = RPS,
        duration_seconds: int = 1,
        max_workers: int = 4,
        perform=synthetic_perform,
    ) -> tuple[list[dict[str, object]], float]:
        return benchmark_api.run_fixed_rate(
            URL,
            COOKIE,
            rps=rps,
            duration_seconds=duration_seconds,
            max_workers=max_workers,
            timeout=1.0,
            clock=self,
            sleeper=self.sleep,
            perform=perform,
        )


class FixedRateScheduleTests(unittest.TestCase):
    def test_sleep_overshoot_does_not_accumulate(self) -> None:
        clock = FakeClock(overshoot=OVERSHOOT)

        results, elapsed = clock.drive()

        self.assertEqual(len(results), RPS)
        scheduled = (RPS - 1) * INTERVAL
        # Absolute deadlines: the whole run may be late by at most one overshoot,
        # instead of one overshoot per request as the old relative chaining did.
        self.assertLessEqual(elapsed, scheduled + OVERSHOOT + 1e-9)
        self.assertLess(elapsed, 1.02)
        # The old `next_due = max(next_due + interval, ...)` chain drifted by
        # roughly RPS * OVERSHOOT (~0.2 s) for this workload.
        self.assertLess(elapsed, scheduled + RPS * OVERSHOOT / 10)

    def test_overshoot_is_absorbed_by_the_next_slot(self) -> None:
        clock = FakeClock(overshoot=OVERSHOOT)

        _results, elapsed = clock.drive()

        sleeps = clock.sleeps
        self.assertEqual(len(sleeps), RPS - 1)
        self.assertLessEqual(max(sleeps), INTERVAL + 1e-12)
        # The first slot starts immediately; every later wait shrinks by exactly
        # the overshoot already spent, so the schedule never lengthens.
        self.assertAlmostEqual(sleeps[0], INTERVAL, delta=1e-9)
        for wait in sleeps[1:]:
            self.assertAlmostEqual(wait, INTERVAL - OVERSHOOT, delta=1e-9)
        self.assertLessEqual(elapsed - (RPS - 1) * INTERVAL, OVERSHOOT + 1e-9)

    def test_single_large_stall_does_not_shift_the_schedule(self) -> None:
        clock = FakeClock(overshoot=OVERSHOOT, stall=0.05, stall_on_call=10)

        results, elapsed = clock.drive()

        self.assertEqual(len(results), RPS)
        scheduled = (RPS - 1) * INTERVAL
        # The 50 ms stall is absorbed by skipping later waits, not by shifting
        # every remaining deadline by 50 ms.
        self.assertLessEqual(elapsed, scheduled + OVERSHOOT + 1e-9)
        self.assertLess(elapsed, 1.0)
        self.assertLess(len(clock.sleeps), RPS - 1)

    def test_on_time_run_issues_no_zero_or_negative_sleep(self) -> None:
        clock = FakeClock(overshoot=0.0)

        results, elapsed = clock.drive()

        self.assertEqual(len(results), RPS)
        # FakeClock.sleep raises on a non-positive request, so reaching this
        # point already proves no sleep was clamped to zero or a negative value.
        self.assertTrue(all(wait > 0 for wait in clock.sleeps))
        self.assertEqual(len(clock.sleeps), RPS - 1)
        self.assertAlmostEqual(elapsed, (RPS - 1) * INTERVAL, delta=1e-9)

    def test_late_iterations_never_extend_the_final_deadline(self) -> None:
        # Every slot overshoots; the last submission must still be pinned to the
        # absolute deadline of the final slot.
        clock = FakeClock(overshoot=OVERSHOOT)

        _results, elapsed = clock.drive(rps=10, duration_seconds=1)

        self.assertLessEqual(elapsed, 9 * 0.1 + OVERSHOOT + 1e-9)
        self.assertAlmostEqual(elapsed, 9 * 0.1 + OVERSHOOT, delta=1e-9)


class PaceUntilTests(unittest.TestCase):
    def test_sleeps_only_when_the_deadline_is_in_the_future(self) -> None:
        clock = FakeClock()
        clock.now = 10.0

        benchmark_api.pace_until(10.5, clock=clock, sleeper=clock.sleep)
        self.assertEqual(clock.sleeps, [0.5])
        self.assertEqual(clock.now, 10.5)

        # Exactly at the deadline and strictly past it: no sleep at all.
        benchmark_api.pace_until(clock.now, clock=clock, sleeper=clock.sleep)
        benchmark_api.pace_until(clock.now - 5.0, clock=clock, sleeper=clock.sleep)
        self.assertEqual(clock.sleeps, [0.5])
        self.assertEqual(clock.now, 10.5)

    def test_defaults_never_sleep_for_an_elapsed_deadline(self) -> None:
        # Uses the production clock/sleeper defaults; a past deadline must return
        # immediately rather than sleeping.
        benchmark_api.pace_until(0.0)
        benchmark_api.pace_until(-1.0)


class RunFixedRateWiringTests(unittest.TestCase):
    def test_returns_exactly_rps_times_duration_results_with_injected_clock(self) -> None:
        calls: list[tuple[str, str, float]] = []

        def perform(url: str, cookie: str, timeout: float) -> dict[str, object]:
            calls.append((url, cookie, timeout))
            return {"latency_ms": 2.0, "status": 200}

        clock = FakeClock(overshoot=OVERSHOOT)

        results, elapsed = clock.drive(
            rps=20, duration_seconds=2, max_workers=4, perform=perform
        )

        self.assertEqual(len(results), 20 * 2)
        self.assertEqual(len(calls), 20 * 2)
        self.assertTrue(all(call == (URL, COOKIE, 1.0) for call in calls))
        self.assertTrue(
            all(result == {"latency_ms": 2.0, "status": 200} for result in results)
        )
        # Elapsed comes from the injected clock, so it is virtual time only.
        self.assertAlmostEqual(elapsed, clock.now, delta=1e-12)
        self.assertAlmostEqual(elapsed, 39 * (1 / 20) + OVERSHOOT, delta=1e-9)

    def test_new_parameters_are_trailing_keyword_only_defaults(self) -> None:
        parameters = inspect.signature(benchmark_api.run_fixed_rate).parameters
        expected = {
            "clock": benchmark_api.time.perf_counter,
            "sleeper": benchmark_api.time.sleep,
            "perform": benchmark_api.perform_get,
        }
        for name, default in expected.items():
            parameter = parameters[name]
            self.assertEqual(parameter.kind, inspect.Parameter.KEYWORD_ONLY)
            self.assertIs(parameter.default, default)
        # Existing required keyword arguments and the return annotation survive.
        for name in ("url", "cookie", "rps", "duration_seconds", "max_workers", "timeout"):
            self.assertIn(name, parameters)
        for name in ("rps", "duration_seconds", "max_workers", "timeout"):
            self.assertEqual(parameters[name].kind, inspect.Parameter.KEYWORD_ONLY)
            self.assertIs(parameters[name].default, inspect.Parameter.empty)


if __name__ == "__main__":
    unittest.main()
