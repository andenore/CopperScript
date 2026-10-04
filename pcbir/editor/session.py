"""Revision-bound placement and explicitly reviewed source transactions."""
from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
from pathlib import Path

from ..clusters import move_placement_unit
from ..layout import plan_placement
from ..physical import BoardSide, PhysicalBoard, Point
from ..placement import PlacementPlannerOptions, placement_solution_is_legal, placement_rejection_reasons
from .scene import board_scene, RatsnestCache
from .source import SourceEditError, SourceSnapshot, fixed_placement_patch, mechanical_patch
from .transactions import SourceWorkspace
from .jobs import PlacementJob


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
                 options: PlacementPlannerOptions | None = None, *, workspace: SourceWorkspace | None = None,
                 overlay=None):
        if board.tracks or board.vias or board.zone_fills or board.materialized_macros:
            raise EditorError("editor placement requires an unrouted physical source")
        self.source = source.resolve()
        self.read_source = workspace.source_reader if workspace and workspace.source_reader else self.source.read_bytes
        self.source_raw = self.read_source()
        self.source_revision = sha256(self.source_raw).hexdigest()
        self.options = options or PlacementPlannerOptions(candidate_count=1)
        self.overlay = overlay
        from .overlay import electrical_digest
        identity = electrical_digest(workspace.identity) if workspace else None
        self.state = State(overlay.seed(board, self.source_revision, identity) if overlay else board)
        self.revision = 0
        self.pending: State | None = None
        self.undo_stack: list[State] = []
        self.redo_stack: list[State] = []
        self.workspace = workspace
        self.source_pending = None
        self.source_undo: list[tuple[bytes, State]] = []
        self.source_redo: list[tuple[bytes, State]] = []
        self.outputs_stale = False
        self.job = None
        self.job_consumed = False
        self.ratsnest_cache = RatsnestCache()
        self.scene_board = None
        self.scene_locks = None
        self.scene_template = None

    def _check(self, revision):
        if type(revision) is not int or revision != self.revision:
            raise StaleRevision("session changed; reload the current scene")
        if sha256(self.read_source()).hexdigest() != self.source_revision:
            raise StaleRevision("source changed externally; reload the source before applying previews")
        if self.workspace:
            self.workspace.check_inputs()

    def _options(self):
        return replace(self.options, fixed_references=self.state.locks)

    def _document(self, state):
        if self.scene_board is not state.board or self.scene_locks != state.locks:
            self.scene_template = board_scene(state.board, source_revision=self.source_revision,
                revision=self.revision, session_locks=state.locks, options=self.options, ratsnest_cache=self.ratsnest_cache)
            self.scene_board, self.scene_locks = state.board, state.locks
        # Job polling does not retransform every pad or rerun full legality.
        # Dynamic fields and component annotations remain per-response records.
        scene = {**self.scene_template, "revision": self.revision, "source_revision": self.source_revision,
                 "components": [dict(c) for c in self.scene_template["components"]],
                 "capabilities": dict(self.scene_template["capabilities"]),
                 "warnings": dict(self.scene_template["warnings"])}
        if self.overlay:
            from .overlay import electrical_digest
            identity = electrical_digest(self.workspace.identity) if self.workspace else None
            overlay_revision = sha256(self.read_source()).hexdigest()
            if self.source_pending and state.board is self.source_pending.board:
                overlay_revision = self.source_pending.after.revision
            if self.workspace:
                try:
                    self.workspace.check_inputs()
                except ValueError:
                    identity = "inputs_changed"
            scene["routed_overlay"] = self.overlay.scene(state.board, overlay_revision, identity)
            if not scene["routed_overlay"]["stale"]:
                from .scene import ratsnest
                evidence = self.overlay.data["evidence"]
                scene["ratsnest"] = ([] if evidence["verified"] and not evidence["remaining"] else
                                     ratsnest(self.overlay.copper_board(state.board)))
                scene["connectivity_basis"] = ("native saved-board DRC" if evidence["verified"] and not evidence["remaining"]
                                               else "explicit routed copper; planes do not grant credit")
                from .scene import net_costs
                scene["net_costs"] = net_costs(scene["ratsnest"])
        if self.workspace:
            from ..elaborate import elaborate
            scene["power_nets"] = sorted({s.net for s in elaborate(self.workspace.identity).supplies})
        scene["scene_profile"] = {"ratsnest_recomputed_nets": self.ratsnest_cache.recomputed_nets}
        scene["source_writable"] = self.workspace is not None
        scene["capabilities"].update(source_save=self.workspace is not None,
                                     mechanical_edit=self.workspace is not None)
        scene["outputs_stale"] = self.outputs_stale
        scene["can_source_undo"] = bool(self.source_undo)
        scene["can_source_redo"] = bool(self.source_redo)
        scene["source_review"] = ({"id": self.source_pending.id, "diff": self.source_pending.diff}
                                  if self.source_pending else None)
        if self.workspace:
            scene["notice"] = "Source edits require diff review and explicit Save. Placement changes remain temporary unless locked into source. No manufacturing signoff."
            from ..parser import parse
            from ..lexer import tokenize
            from ..syntax import MechanicalDecl, MechanicalItemDecl, ConstraintDecl
            from .source import declaration_spans
            snapshot = SourceSnapshot(self.source_raw, str(self.source))
            document = parse(snapshot.text, snapshot.filename)
            tokens = tokenize(snapshot.text, snapshot.filename)
            scene["mechanical_features"] = []
            for block in document.declarations:
                if not isinstance(block, MechanicalDecl):
                    continue
                for item in block.items:
                    if isinstance(item, MechanicalItemDecl):
                        spans = declaration_spans(tokens, item)
                        scene["mechanical_features"].append({"kind": item.kind, "name": item.name,
                            "shape": item.shape, "line": item.location.line, "editable": True,
                            "parameters": {k: snapshot.text[v[0].start:v[-1].end]
                                           for k, v in spans.properties}})
            for component in scene["components"]:
                own = [d for d in document.declarations if isinstance(d, ConstraintDecl)
                       and d.kind == "fixed_placement" and component["reference"] in d.targets]
                component["source_lock_owned"] = len(own) == 1 and own[0].targets == (component["reference"],)
        for component in scene["components"]:
            rule = next((r for r in state.board.placement_rules if r.reference == component["reference"]), None)
            component["source_side_locked"] = bool(rule and rule.side is not None)
        return scene

    def scene(self):
        self._poll_job()
        scene = self._document(self.state)
        scene.update({"can_undo": bool(self.undo_stack), "can_redo": bool(self.redo_stack),
                      "source_stale": sha256(self.read_source()).hexdigest() != self.source_revision})
        scene["placement_job"] = self.job.poll() if self.job else None
        scene["pending_preview"] = self._document(self.pending) if self.pending else None
        return scene

    def _poll_job(self):
        if self.job is None:
            return
        info = self.job.poll()
        if info["status"] != "completed" or self.job_consumed:
            return
        self.job_consumed = True
        try:
            self._check(self.job.revision)
            if self.job.source_revision != self.source_revision:
                raise StaleRevision("source changed during placement")
        except (ValueError, OSError):
            self.job.status, self.job.error = "stale", "input changed; placement result discarded"
            return
        self.pending = State(self.job.result, self.state.locks)
        self.revision += 1

    def close(self):
        if self.job:
            self.job.cancel()

    def _commit(self, state):
        if state.board != self.state.board:
            self.outputs_stale = True
        self.undo_stack.append(self.state)
        self.undo_stack = self.undo_stack[-64:]
        self.redo_stack.clear()
        self.state = state
        self.pending = None
        self.source_pending = None
        self.revision += 1

    def operation(self, request: dict) -> dict:
        action = request.get("action")
        fields = {"auto_place": (), "move": ("reference", "x_nm", "y_nm", "rotation", "side"),
                  "lock": ("reference", "locked"), "apply": (), "discard": (), "undo": (), "redo": (),
                  "prepare_lock": ("reference", "x_nm", "y_nm", "rotation", "side", "locks"),
                  "prepare_mechanical": ("kind", "name", "shape", "parameters", "remove"),
                  "prepare_mechanical_batch": ("features",),
                  "save_source": ("review_id",), "discard_source": (),
                  "undo_source": (), "redo_source": (), "reload_source": (),
                  "start_auto_place": ("budget_seconds",), "cancel_auto_place": ()}
        if not isinstance(action, str) or action not in fields:
            raise EditorError("unknown editor operation")
        if set(request) != {"action", "revision", *fields[action]}:
            raise EditorError("unexpected or missing operation fields")
        if action == "cancel_auto_place":
            if type(request["revision"]) is not int or request["revision"] != self.revision:
                raise StaleRevision("session changed; reload the scene")
            self.close()
            return {"scene": self.scene(), "accepted": True}
        if action == "reload_source":
            if request["revision"] != self.revision or type(request["revision"]) is not int:
                raise StaleRevision("session changed; reload the current scene")
            if self.workspace is None:
                raise EditorError("source workspace is read-only")
            # Explicit reload discards local previews/history; never auto-merges.
            raw = self.read_source()
            from ..compiler import compile_design_source
            design = compile_design_source(SourceSnapshot(raw).text, str(self.source), locked=True, offline=True)
            from ..erc import check, has_errors
            if has_errors(check(design.electrical)):
                raise EditorError("external source fails ERC")
            board = self.workspace.build(design)
            from .transactions import electrical_identity
            self.workspace.identity = electrical_identity(design.electrical)
            self.workspace.inputs = self.workspace._inputs()
            self.source_revision = sha256(raw).hexdigest()
            self.source_raw = raw
            self.state = State(board)
            self.pending = self.source_pending = None
            self.undo_stack.clear(); self.redo_stack.clear()
            self.source_undo.clear(); self.source_redo.clear()
            self.outputs_stale = True
            self.close()
            self.revision += 1
            return {"scene": self.scene(), "accepted": True}
        self._check(request["revision"])
        if self.job and self.job.poll()["status"] == "running":
            self.job.cancel("stale", "session edited; old placement cancelled")
        if self.source_pending and action not in {"save_source", "discard_source"}:
            raise EditorError("save or discard the source review before another operation")
        if action in {"prepare_lock", "prepare_mechanical", "prepare_mechanical_batch", "save_source", "discard_source", "undo_source", "redo_source"}:
            return self._source_operation(action, request)
        if action == "start_auto_place":
            budget = request["budget_seconds"]
            if type(budget) not in (int, float) or not 1 <= budget <= 300:
                raise EditorError("placement budget must be 1–300 seconds")
            self.pending = None
            self.revision += 1
            self.job_consumed = False
            self.job = PlacementJob(self.state.board, self._options(), revision=self.revision,
                                   source_revision=self.source_revision, timeout_seconds=budget)
            return {"scene": self.scene(), "accepted": True}
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
                raise EditorError("pose violates physical constraints: " + "; ".join(
                    placement_rejection_reasons(self.state.board, moved, self._options())))
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

    def _source_operation(self, action, request):
        if self.workspace is None:
            raise EditorError("source workspace is read-only")
        if self.pending:
            raise EditorError("apply or discard the placement preview before editing source")
        snapshot = SourceSnapshot(self.read_source(), str(self.source))
        if action == "prepare_lock":
            reference = request["reference"]
            refs = frozenset(p.reference for p in self.state.board.placements)
            if reference not in refs:
                raise EditorError("unknown component reference")
            import json
            owner = json.loads(self.state.board.metadata.get("mechanical_attachment_owners","{}")).get(reference)
            if owner:
                raise EditorError(f"pose is owned by mechanical attachment {owner!r}; edit that attachment instead")
            from ..elaborate import elaborate
            from ..model import ConstraintKind
            # A flattened import may own a pose even when no root declaration
            # exists. Do not manufacture a competing board-level owner.
            from ..compiler import compile_design_source
            current = compile_design_source(snapshot.text, snapshot.filename, locked=True, offline=True)
            imported = [c for c in elaborate(current.electrical).constraints
                        if c.kind is ConstraintKind.FIXED_PLACEMENT and reference in c.targets
                        and any(not origin.startswith(str(self.source) + ":") for origin in c.origins)]
            if imported:
                raise EditorError("fixed pose is owned by an imported module; edit its public mechanical binding instead")
            locks = request["locks"]
            if (not isinstance(locks, list) or any(not isinstance(k, str) for k in locks)
                    or len(set(locks)) != len(locks) or set(locks) - {"position", "rotation", "side"}):
                raise EditorError("locks must be unique position/rotation/side names")
            values = {}
            clear = []
            if "position" in locks:
                values.update(x_nm=request["x_nm"], y_nm=request["y_nm"])
            else:
                clear += ["x", "y"]
            if "rotation" in locks:
                values["rotation"] = request["rotation"]
            else:
                clear.append("rotation")
            if "side" in locks:
                values["side"] = request["side"]
            else:
                clear.append("side")
            patch = fixed_placement_patch(snapshot, reference, **values, clear=tuple(clear),
                                          resolved_references=refs)
            self.source_pending = self.workspace.review(snapshot, patch.apply(snapshot),
                                                       self.state.board, self.options)
        elif action in {"prepare_mechanical","prepare_mechanical_batch"}:
            features=request['features'] if action=='prepare_mechanical_batch' else [{k:request[k] for k in ('kind','name','shape','parameters','remove')}]
            if not isinstance(features,list) or not 1<=len(features)<=513:raise EditorError('mechanical batch requires 1–513 feature edits')
            # Existing imported features cannot be shadowed by a local declaration.
            import json
            provenance = json.loads(self.state.board.metadata.get("mechanical_provenance", "{}"))
            current=snapshot;seen=set()
            for feature in features:
                if (not isinstance(feature,dict) or set(feature)!={'kind','name','shape','parameters','remove'}
                    or not isinstance(feature['parameters'],dict) or len(feature['parameters'])>32
                    or not all(isinstance(k,str) and isinstance(v,str) for k,v in feature['parameters'].items())
                    or not all(isinstance(feature[k],str) for k in ('kind','name','shape')) or type(feature['remove']) is not bool):
                    raise EditorError('mechanical feature requires a bounded literal property map')
                identity=(feature['kind'],feature['name'])
                if identity in seen:raise EditorError('mechanical batch cannot edit a feature twice')
                seen.add(identity)
                if any(f['kind']==feature['kind'] and f['name']==feature['name'] and f.get('profile') for f in provenance.get('features',())):
                    raise EditorError('mechanical feature is owned by an imported profile')
                current=SourceSnapshot(mechanical_patch(current,**feature).apply(current),snapshot.filename)
            self.source_pending = self.workspace.review(snapshot, current.raw,
                                                       self.state.board, self.options)
        elif action == "save_source":
            review = self.source_pending
            if review is None or request["review_id"] != review.id:
                raise StaleRevision("review changed; inspect the current source diff")
            # Repeat validation immediately before the final filesystem CAS.
            _, validated = self.workspace.validate(review.after.raw, review.board, self.options)
            if validated != review.board:
                raise EditorError("physical inputs changed since review; discard and prepare the edit again")
            self.workspace.save(review)
            self.source_undo.append((review.before.raw, self.state))
            self.source_undo = self.source_undo[-64:]
            self.source_redo.clear()
            self._saved(review.after.raw, review.board)
            return {"scene": self.scene(), "accepted": True}
        elif action == "discard_source":
            self.source_pending = None
        elif action in {"undo_source", "redo_source"}:
            take, put = (self.source_undo, self.source_redo) if action == "undo_source" else (self.source_redo, self.source_undo)
            if not take:
                raise EditorError("no source history available")
            raw, state = take[-1]
            review = self.workspace.review(snapshot, raw, state.board, self.options)
            self.workspace.save(review)
            take.pop()
            put.append((snapshot.raw, self.state))
            self._saved(raw, review.board)
            return {"scene": self.scene(), "accepted": True}
        self.revision += 1
        response = {"scene": self.scene(), "accepted": True}
        if self.source_pending:
            response["source_preview"] = self._document(State(self.source_pending.board))
        return response

    def _saved(self, raw, board):
        self.source_raw = raw
        self.source_revision = sha256(raw).hexdigest()
        self.state = State(replace(board, metadata={**board.metadata,
            "fabrication_ready": "false", "editor_outputs_stale": "true"}))
        self.pending = self.source_pending = None
        self.undo_stack.clear(); self.redo_stack.clear()
        self.outputs_stale = True
        self.revision += 1
