"""Optional operational telemetry; never part of routing or signoff evidence."""
from __future__ import annotations

import json
import time
from typing import Callable, Mapping

ProgressCallback = Callable[[str, str, Mapping[str, object]], None]


def emit(callback: ProgressCallback | None, phase: str, event: str, **details: object) -> None:
    if callback is not None:
        callback(phase, event, details)


def critical_progress(callback: ProgressCallback | None):
    if callback is None:
        return None
    def observe(event, nets, result):
        emit(callback, "critical_group", event, nets=list(nets),
             **({"connected": result.connected, "strategy": result.strategy,
                 "search_states": result.search_states} if result is not None else {}))
    return observe


def console_progress(*, clock: Callable[[], float] = time.perf_counter) -> ProgressCallback:
    started = clock()
    def observe(phase, event, details):
        # Flush immediately so the full-run wrapper can persist each event.
        print("PROGRESS " + json.dumps({"phase": phase, "event": event,
              "elapsed_seconds": round(clock() - started, 3), "details": dict(details)},
              sort_keys=True), flush=True)
    return observe
