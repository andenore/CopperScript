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
import time
from pathlib import Path
import stat
import tempfile
from uuid import uuid4
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
    from ..macro_bindings import macro_binding_identity
    modules = {name: replace(module, constraints=tuple(replace(c, origins=())
               for c in module.constraints), physical_macros=tuple(map(macro_binding_identity, module.physical_macros)))
               for name, module in board.module_definitions.items()}
    return replace(board, constraints=constraints, module_definitions=modules,
                   physical_macros=tuple(map(macro_binding_identity, board.physical_macros)))


def scoped_identity(board, constraint_scope=None):
    identity = electrical_identity(board)
    if constraint_scope is None:
        return identity
    from .features import CONSTRAINTS
    scopes = (constraint_scope,) if isinstance(constraint_scope[0], str) else constraint_scope
    if not scopes or any(kind not in CONSTRAINTS for kind, targets in scopes):
        raise SourceEditError('unsupported constraint edit permission')
    return replace(identity, constraints=tuple(c for c in identity.constraints
        if not any(c.kind.value == kind and c.targets == tuple(targets) for kind, targets in scopes)))


def seed_placements(board: PhysicalBoard, seeds: PhysicalBoard, only=None) -> PhysicalBoard:
    """Retain movable session seeds without making them persistent locks.

    ``only`` limits seeding to these references (the others keep ``board``'s poses).
    """
    existing = {p.reference: p for p in seeds.placements if only is None or p.reference in only}
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
    from ..clusters import apply_fixed_cluster_poses
    return apply_fixed_cluster_poses(replace(board, placements=tuple(poses)))


# A save holds the lock while it re-checks inputs and writes the file: seconds at
# most. A lock older than this was left by a crashed save.
STALE_LOCK_SECONDS = 120


def _lock_age(path: Path) -> float | None:
    try:
        return time.time() - path.stat().st_mtime
    except FileNotFoundError:
        return None


def _break_stale_lock(lock: Path) -> None:
    """Remove a crashed save's lock without ever removing a live one.

    The rename is atomic, so only one editor claims a stale lock. If the claimed
    file turns out to be fresh (another editor replaced it in between), it is put
    back unless a newer lock already exists.
    """
    age = _lock_age(lock)
    if age is None or age <= STALE_LOCK_SECONDS:
        return
    claimed = lock.with_name(f"{lock.name}.stale-{uuid4().hex}")
    try:
        os.rename(lock, claimed)
    except FileNotFoundError:
        return
    if (_lock_age(claimed) or 0) <= STALE_LOCK_SECONDS:
        try:
            os.link(claimed, lock)  # fails if a newer lock exists; never overwrites one
        except FileExistsError:
            pass
        except OSError:  # no hard links (some network shares): restore only if still absent
            if not lock.exists():
                try:
                    os.rename(claimed, lock)
                    return
                except OSError:
                    pass
    claimed.unlink(missing_ok=True)


@dataclass(frozen=True)
class ReviewedSource:
    before: SourceSnapshot
    after: SourceSnapshot
    design: Design
    board: PhysicalBoard
    diff: str
    # One (kind, targets), or an atomic tuple of such scopes.
    constraint_scope: tuple | None = None
    summary: str = ""

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
        self.references = design.mechanical.references if design.mechanical else ()
        self.inputs = self._inputs()

    def _inputs(self):
        return tuple((str(p), sha256(p.read_bytes()).hexdigest() if p.is_file() else None)
                     for p in self.input_paths)

    def check_inputs(self):
        from ..mechanical_references import verify_reference_assets
        verify_reference_assets(self.references)
        if self._inputs() != self.inputs:
            raise SourceEditError("compiler/footprint inputs changed externally; reload the source workspace")

    def validate(self, raw: bytes, seeds: PhysicalBoard, options: PlacementPlannerOptions,
                 *, preserve_electrical=True, constraint_scope=None) -> tuple[Design, PhysicalBoard]:
        self.check_inputs()
        snapshot = SourceSnapshot(raw, str(self.source))
        design = compile_design_source(snapshot.text, snapshot.filename, locked=True, offline=True)
        if preserve_electrical and scoped_identity(design.electrical, constraint_scope) != scoped_identity(self.identity, constraint_scope):
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
        from ..mechanical_references import verify_reference_assets
        verify_reference_assets(board.mechanical_references)
        return design, board

    def review(self, snapshot: SourceSnapshot, raw: bytes, seeds: PhysicalBoard,
               options: PlacementPlannerOptions, *, constraint_scope=None) -> ReviewedSource:
        if snapshot.raw == raw:
            raise SourceEditError("edit has no source changes")
        design, board = self.validate(raw, seeds, options, constraint_scope=constraint_scope)
        after = SourceSnapshot(raw, snapshot.filename)
        diff = "".join(unified_diff(snapshot.text.splitlines(keepends=True),
                      after.text.splitlines(keepends=True), fromfile="current.copper", tofile="proposed.copper"))
        return ReviewedSource(snapshot, after, design, board, diff, constraint_scope)

    def save(self, review: ReviewedSource):
        """Commit reviewed bytes only, never arbitrary client-supplied source."""
        if self.source_reader is not None:
            raise SourceEditError("document host must commit through native document edits, not filesystem saves")
        self.check_inputs()
        from ..mechanical_references import verify_reference_assets
        verify_reference_assets(review.board.mechanical_references)
        if self.source.is_symlink():
            raise SourceEditError("source was replaced with a symbolic link")
        lock = self.source.with_name(self.source.name + ".editor-lock")
        temporary = None
        owns_lock = False
        _break_stale_lock(lock)
        token = f"pid {os.getpid()} token {uuid4().hex} at {time.time():.0f}\n"
        try:
            with lock.open("x") as marker:
                owns_lock = True
                marker.write(token)
                marker.flush()
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
                verify_reference_assets(review.board.mechanical_references)
                if self.source.read_bytes() != review.before.raw:
                    raise SourceEditError("source changed during save; no edit committed")
                os.replace(temporary, self.source)
                temporary = None
        except FileExistsError as exc:
            raise SourceEditError(f"another editor is saving this source right now ({lock.name}); retry in a "
                                  f"moment. A lock older than {STALE_LOCK_SECONDS} s is removed automatically") from exc
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
            # Remove only our own lock: never one another editor created after a stale break.
            if owns_lock:
                try:
                    if lock.read_text(encoding="utf-8") == token:
                        lock.unlink(missing_ok=True)
                except FileNotFoundError:
                    pass
