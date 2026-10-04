"""One bounded, cancellable placement process per editor session.

Only trusted in-memory IR crosses the private process pipe. No pickle endpoint
or user-selected module/path is exposed. Termination stops CPU work, unlike
cancelling a Python Future whose thread continues to run.
"""
from __future__ import annotations

import cProfile
import io
import multiprocessing as mp
from multiprocessing.reduction import ForkingPickler
import pstats
from threading import Event, RLock, Thread
from time import monotonic
from types import MappingProxyType

from ..layout import plan_placement


def _mapping(values):
    return MappingProxyType(values)


def _reduce_mapping(value):
    return _mapping, (dict(value),)


ForkingPickler.register(type(MappingProxyType({})), _reduce_mapping)


def _worker(board, options, pipe):
    profile = cProfile.Profile()
    try:
        def progress(phase, candidate, candidates):
            pipe.send(("progress", {"phase": phase, "candidate": candidate, "candidates": candidates}))
        profile.enable()
        result = plan_placement(board, options, progress=progress)
        profile.disable()
        out = io.StringIO()
        stats = pstats.Stats(profile, stream=out).strip_dirs().sort_stats("cumulative")
        stats.print_stats(20)
        pipe.send(("result", result.board, {"total_calls": stats.total_calls,
            "total_seconds": stats.total_tt, "top_cumulative": out.getvalue()}))
    except Exception as exc:
        pipe.send(("error", f"{type(exc).__name__}: {exc}"))
    finally:
        pipe.close()


class PlacementJob:
    def __init__(self, board, options, *, revision, source_revision, timeout_seconds=120):
        if len(board.placements) > 5000 or not 0 < timeout_seconds <= 300:
            raise ValueError("editor placement budget is at most 5000 components and 300 seconds")
        self.revision, self.source_revision = revision, source_revision
        self.started = monotonic()
        self.timeout_seconds = timeout_seconds
        self.status = "running"
        self.progress = {"phase": "starting", "candidate": 0, "candidates": max(2, options.candidate_count)}
        self.profile = None
        self.result = None
        self.error = None
        self.lock = RLock()
        self.done = Event()
        context = mp.get_context("spawn")
        self.pipe, child = context.Pipe(duplex=False)
        self.process = context.Process(target=_worker, args=(board, options, child), daemon=True)
        self.process.start()
        child.close()
        self.watchdog = Thread(target=self._watch, daemon=True)
        self.watchdog.start()

    def _watch(self):
        if not self.done.wait(self.timeout_seconds):
            self.cancel("timed_out", "placement exceeded its wall-time budget")

    def poll(self):
        with self.lock:
            if self.status == "running":
                while self.pipe.poll():
                    try:
                        message = self.pipe.recv()
                    except EOFError:
                        break
                    if message[0] == "progress":
                        self.progress = message[1]
                    elif message[0] == "result":
                        self.result, self.profile = message[1:]
                        self.status = "completed"
                        break
                    else:
                        self.status, self.error = "failed", message[1]
                        break
                if self.status == "running" and not self.process.is_alive():
                    self.status, self.error = "failed", "placement process exited without a result"
                if self.status != "running":
                    self.done.set()
                    self.process.join(timeout=.1)
                    self.pipe.close()
            return {"status": self.status, "progress": self.progress, "error": self.error,
                    "elapsed_seconds": round(monotonic() - self.started, 3),
                    "budget_seconds": self.timeout_seconds, "profile": self.profile,
                    "input_revision": self.revision}

    def cancel(self, status="cancelled", error=None):
        with self.lock:
            if self.status != "running":
                return
            self.status, self.error = status, error
            if self.process.is_alive():
                self.process.terminate()
                self.process.join(timeout=1)
                if self.process.is_alive():
                    self.process.kill()
                    self.process.join(timeout=1)
            self.done.set()
            self.pipe.close()
