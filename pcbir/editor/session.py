"""Revision-bound, immutable in-memory placement transactions. No source writes."""
from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
from pathlib import Path

from ..clusters import move_placement_unit
from ..layout import plan_placement
from ..physical import BoardSide, PhysicalBoard, Point
from ..placement import PlacementPlannerOptions, placement_solution_is_legal
from .scene import board_scene


class EditorError(ValueError):
    pass


class StaleRevision(EditorError):
    pass


@dataclass(frozen=True)
class State:
    board: PhysicalBoard
    locks: frozenset[str] = frozenset()


def _integer(value, name):
    if type(value) is not int or abs(value) > 10**12:
        raise EditorError(f"{name} requires an integer within +/- 1 km in nanometres")
    return value


class EditorSession:
    def __init__(self, board: PhysicalBoard, source: Path,
                 options: PlacementPlannerOptions | None = None):
        if board.tracks or board.vias or board.zone_fills or board.materialized_macros:
            raise EditorError("editor placement requires an unrouted physical source")
        self.source = source.resolve()
        self.source_revision = sha256(self.source.read_bytes()).hexdigest()
        self.options = options or PlacementPlannerOptions(candidate_count=1)
        self.state = State(board)
        self.revision = 0
        self.pending: State | None = None
        self.undo_stack: list[State] = []
        self.redo_stack: list[State] = []

    def _check(self, revision):
        if type(revision) is not int or revision != self.revision:
            raise StaleRevision("session changed; reload the current scene")
        if sha256(self.source.read_bytes()).hexdigest() != self.source_revision:
            raise StaleRevision("source changed externally; restart the editor before applying previews")

    def _options(self):
        return replace(self.options, fixed_references=self.state.locks)

    def _document(self, state):
        return board_scene(state.board, source_revision=self.source_revision,
            revision=self.revision, session_locks=state.locks, options=self.options)

    def scene(self):
        scene = self._document(self.state)
        scene.update({"can_undo": bool(self.undo_stack), "can_redo": bool(self.redo_stack),
                      "source_stale": sha256(self.source.read_bytes()).hexdigest() != self.source_revision})
        return scene

    def _commit(self, state):
        self.undo_stack.append(self.state)
        self.undo_stack = self.undo_stack[-64:]
        self.redo_stack.clear()
        self.state = state
        self.pending = None
        self.revision += 1

    def operation(self, request: dict) -> dict:
        action = request.get("action")
        fields = {"auto_place": (), "move": ("reference", "x_nm", "y_nm", "rotation", "side"),
                  "lock": ("reference", "locked"), "apply": (), "discard": (), "undo": (), "redo": ()}
        if not isinstance(action, str) or action not in fields:
            raise EditorError("unknown editor operation")
        if set(request) != {"action", "revision", *fields[action]}:
            raise EditorError("unexpected or missing operation fields")
        self._check(request["revision"])
        if action == "auto_place":
            # A failed attempt invalidates an old preview, but never accepted state.
            self.pending = None
            planned = plan_placement(self.state.board, self._options()).board
            self.pending = State(planned, self.state.locks)
            self.revision += 1  # A preview is revision-bound too: another tab cannot replace it silently.
            return {"preview": self._document(self.pending), "accepted": False}
        if action == "move":
            self.pending = None
            reference = request["reference"]
            poses = {p.reference: p for p in self.state.board.placements}
            if not isinstance(reference, str) or reference not in poses:
                raise EditorError("unknown component reference")
            if reference in self.state.locks:
                raise EditorError("unlock the temporary pose before moving it")
            pose = poses[reference]
            try:
                rotation = Decimal(str(request["rotation"]))
                if not rotation.is_finite() or abs(rotation) > 3600:
                    raise ValueError("rotation must be finite degrees within +/- 3600")
                proposed = replace(pose, position=Point(_integer(request["x_nm"], "x_nm"),
                    _integer(request["y_nm"], "y_nm")), rotation_degrees=rotation,
                    side=BoardSide(request["side"]))
            except (ValueError, InvalidOperation) as exc:
                raise EditorError(str(exc)) from exc
            moved = move_placement_unit(self.state.board, poses, reference, proposed)
            if not placement_solution_is_legal(self.state.board, moved, self._options()):
                raise EditorError("pose violates a fixed pose, allowed angle/side, courtyard, material or relative rule")
            trial = replace(self.state.board, placements=tuple(moved[p.reference]
                for p in self.state.board.placements), metadata={**self.state.board.metadata,
                    "fabrication_ready": "false", "editor_preview": "true"})
            self.pending = State(trial, self.state.locks)
            self.revision += 1
            return {"preview": self._document(self.pending), "accepted": False}
        if action == "lock":
            reference = request["reference"]
            if type(request["locked"]) is not bool or not isinstance(reference, str):
                raise EditorError("lock requires a component reference and boolean locked")
            if reference not in {p.reference for p in self.state.board.placements}:
                raise EditorError("unknown component reference")
            # Lock whole rigid unit so the planner cannot move its other members.
            unit = {reference}
            for cluster in self.state.board.rigid_clusters:
                if any(m.reference == reference for m in cluster.members):
                    unit.update(m.reference for m in cluster.members)
            locks = self.state.locks | unit if request["locked"] else self.state.locks - unit
            self._commit(State(self.state.board, frozenset(locks)))
        elif action == "apply":
            if self.pending is None:
                raise EditorError("no pending preview")
            if not placement_solution_is_legal(self.state.board,
                    {p.reference: p for p in self.pending.board.placements}, self._options()):
                raise EditorError("pending placement no longer satisfies hard rules")
            self._commit(self.pending)
        elif action == "discard":
            self.pending = None
            self.revision += 1
        elif action in {"undo", "redo"}:
            take, put = (self.undo_stack, self.redo_stack) if action == "undo" else (self.redo_stack, self.undo_stack)
            if not take:
                raise EditorError(f"nothing to {action}")
            put.append(self.state)
            self.state = take.pop()
            self.pending = None
            self.revision += 1
        return {"scene": self.scene(), "accepted": True}
