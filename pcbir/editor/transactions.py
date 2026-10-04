"""Source-authoritative review/save transactions shared by editor hosts.

No path comes from an HTTP request. Compilation is in memory and dependency
resolution is offline. Saving uses a same-directory temporary file, fsync, an
exclusive cooperative lock, a final byte-revision check and atomic replacement.
External non-cooperating writers must use reload/conflict resolution; the editor
never merges their changes silently.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from difflib import unified_diff
from hashlib import sha256
import os
from pathlib import Path
import stat
import tempfile
from typing import Callable

from ..compiler import compile_design_source
from ..design import Design
from ..erc import check, has_errors
from ..model import ConstraintKind
from ..physical import PhysicalBoard
from ..placement import PlacementPlannerOptions, placement_solution_is_legal, placement_rejection_reasons
from .source import SourceEditError, SourceSnapshot


def electrical_identity(board):
    """Ignore only editable pose intent and diagnostic source offsets.

    All parts, hierarchy, configuration, nets, supplies, interfaces, dependencies
    and other constraints must remain exactly equal.
    """
    constraints = tuple(replace(c, origins=()) for c in board.constraints
                        if c.kind is not ConstraintKind.FIXED_PLACEMENT)
    modules = {name: replace(module, constraints=tuple(replace(c, origins=())
               for c in module.constraints)) for name, module in board.module_definitions.items()}
    return replace(board, constraints=constraints, module_definitions=modules)


def seed_placements(board: PhysicalBoard, seeds: PhysicalBoard) -> PhysicalBoard:
    """Retain movable session seeds without making them persistent locks."""
    existing = {p.reference: p for p in seeds.placements}
    rules = {r.reference: r for r in board.placement_rules}
    poses = []
    for p in board.placements:
        previous = existing.get(p.reference)
        if previous is not None and previous.footprint == p.footprint:
            p = replace(p, position=previous.position, rotation_degrees=previous.rotation_degrees,
                        side=previous.side)
        rule = rules.get(p.reference)
        if rule:
            p = replace(p, position=rule.fixed_position or p.position,
                        rotation_degrees=rule.fixed_rotation_degrees if rule.fixed_rotation_degrees is not None else p.rotation_degrees,
                        side=rule.side or p.side)
        poses.append(p)
    return replace(board, placements=tuple(poses))


@dataclass(frozen=True)
class ReviewedSource:
    before: SourceSnapshot
    after: SourceSnapshot
    design: Design
    board: PhysicalBoard
    diff: str

    @property
    def id(self):
        return sha256((self.before.revision + self.after.revision).encode()).hexdigest()


class SourceWorkspace:
    def __init__(self, source: Path, design: Design, build: Callable[[Design], PhysicalBoard],
                 *, input_paths: tuple[Path, ...] = (), source_reader=None):
        self.source = source.absolute()
        if self.source.is_symlink():
            raise SourceEditError("source saving does not follow a symbolic link")
        self.identity = electrical_identity(design.electrical)
        self.source_reader = source_reader
        self.build = build
        self.input_paths = tuple(p.resolve() for p in input_paths)
        self.inputs = self._inputs()

    def _inputs(self):
        return tuple((str(p), sha256(p.read_bytes()).hexdigest() if p.is_file() else None)
                     for p in self.input_paths)

    def check_inputs(self):
        if self._inputs() != self.inputs:
            raise SourceEditError("compiler/footprint inputs changed externally; reload the source workspace")

    def validate(self, raw: bytes, seeds: PhysicalBoard, options: PlacementPlannerOptions,
                 *, preserve_electrical=True) -> tuple[Design, PhysicalBoard]:
        self.check_inputs()
        snapshot = SourceSnapshot(raw, str(self.source))
        design = compile_design_source(snapshot.text, snapshot.filename, locked=True, offline=True)
        if preserve_electrical and electrical_identity(design.electrical) != self.identity:
            raise SourceEditError("edit changes electrical intent or dependency bytes")
        diagnostics = check(design.electrical)
        if has_errors(diagnostics):
            raise SourceEditError("prospective source fails ERC: " + "; ".join(str(d) for d in diagnostics))
        board = seed_placements(self.build(design), seeds)
        if board.tracks or board.vias or board.zone_fills or board.materialized_macros:
            raise SourceEditError("prospective rebuild retained routed copper")
        if not placement_solution_is_legal(board, {p.reference: p for p in board.placements}, options):
            raise SourceEditError("prospective geometry/pose violates physical constraints: " + "; ".join(
                placement_rejection_reasons(board, {p.reference: p for p in board.placements}, options)))
        from ..drc import placement_copper_findings
        from ..hard_macros import materialize_hard_macros
        findings = placement_copper_findings(materialize_hard_macros(board))
        if findings:
            raise SourceEditError("prospective copper/drill geometry: " + "; ".join(f.message for f in findings))
        self.check_inputs()
        return design, board

    def review(self, snapshot: SourceSnapshot, raw: bytes, seeds: PhysicalBoard,
               options: PlacementPlannerOptions) -> ReviewedSource:
        if snapshot.raw == raw:
            raise SourceEditError("edit has no source changes")
        design, board = self.validate(raw, seeds, options)
        after = SourceSnapshot(raw, snapshot.filename)
        diff = "".join(unified_diff(snapshot.text.splitlines(keepends=True),
                      after.text.splitlines(keepends=True), fromfile="current.copper", tofile="proposed.copper"))
        return ReviewedSource(snapshot, after, design, board, diff)

    def save(self, review: ReviewedSource):
        """Commit reviewed bytes only, never arbitrary client-supplied source."""
        if self.source_reader is not None:
            raise SourceEditError("document host must commit through native document edits, not filesystem saves")
        self.check_inputs()
        if self.source.is_symlink():
            raise SourceEditError("source was replaced with a symbolic link")
        lock = self.source.with_name(self.source.name + ".editor-lock")
        temporary = None
        owns_lock = False
        try:
            with lock.open("x"):
                owns_lock = True
                if self.source.read_bytes() != review.before.raw:
                    raise SourceEditError("source changed since review; reload before saving")
                mode = stat.S_IMODE(self.source.stat().st_mode)
                fd, name = tempfile.mkstemp(prefix=".copper-edit-", dir=self.source.parent)
                temporary = Path(name)
                with os.fdopen(fd, "wb") as stream:
                    stream.write(review.after.raw)
                    stream.flush()
                    os.fsync(stream.fileno())
                os.chmod(temporary, mode)
                self.check_inputs()
                if self.source.read_bytes() != review.before.raw:
                    raise SourceEditError("source changed during save; no edit committed")
                os.replace(temporary, self.source)
                temporary = None
        except FileExistsError as exc:
            raise SourceEditError("another editor owns the source save lock") from exc
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
            # Do not remove somebody else's lock after an exclusive-create failure.
            if owns_lock:
                lock.unlink(missing_ok=True)
