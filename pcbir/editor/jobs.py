"""One cancellable placement process per editor session, without a time limit.

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
from threading import RLock
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
    def __init__(self, board, options, *, revision, source_revision):
        if len(board.placements) > 5000:
            raise ValueError("editor placement supports at most 5000 components")
        self.revision, self.source_revision = revision, source_revision
        self.started = monotonic()
        self.finished = None  # frozen when the job stops, so elapsed time stops counting
        self.status = "running"
        self.progress = {"phase": "starting", "candidate": 0, "candidates": max(2, options.candidate_count)}
        self.profile = None
        self.result = None
        self.error = None
        self.lock = RLock()
        context = mp.get_context("spawn")
        self.pipe, child = context.Pipe(duplex=False)
        self.process = context.Process(target=_worker, args=(board, options, child), daemon=True)
        self.process.start()
        child.close()

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
                    self.finished = self.finished or monotonic()
                    self.process.join(timeout=.1)
                    self.pipe.close()
            return {"status": self.status, "progress": self.progress, "error": self.error,
                    "elapsed_seconds": round((self.finished or monotonic()) - self.started, 3),
                    "profile": self.profile,
                    "input_revision": self.revision}

    def cancel(self, status="cancelled", error=None):
        with self.lock:
            if self.status != "running":
                return
            self.status, self.error = status, error
            self.finished = monotonic()
            if self.process.is_alive():
                self.process.terminate()
                self.process.join(timeout=1)
                if self.process.is_alive():
                    self.process.kill()
                    self.process.join(timeout=1)
            self.pipe.close()
