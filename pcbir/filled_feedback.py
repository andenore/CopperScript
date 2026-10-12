"""Native filled-copper feedback, with bounded guide-free bridge transactions.

Source-zone intent is immutable. Only exact mapped, general distribution-net
opens are eligible; unknown native objects and specialized routes fail closed.
The shared dependency router may displace completed ordinary area copper, never
source, macro, critical, escape or previously accepted plane/bridge ownership.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import asdict, dataclass, replace
from itertools import chain, product
import json
from pathlib import Path

from .detailed import DetailedRouterOptions
from .drc import physical_board_digest, run_physical_drc
from .escape_feedback import (EscapeFeedbackOptions, _merge_local_detail,
    _reroute_local_dependencies, _with_preserved_area, _unaffected_copper)
from .flow import RoutingPipelineResult
from .manufacturing import CommandRunner
from .physical import CopperLayer, PhysicalBoard, Point, RouteKind, nm_from_mm
from .plane_verify import (PlaneVerification, native_object_map, verify_filled_planes,
                          remove_native_dangling_copper)
from .geometry import point_in_polygon
from .filled_connectivity import filled_component_roots, native_item_identity
from .progress import ProgressCallback, emit
from .routing_clearance import RoutingBlocker, RoutingClearanceIndex
from .surface_path import surface_path_between
from .zone_geometry import distribution_zone_nets


@dataclass(frozen=True, slots=True)
class FilledCopperFeedbackOptions:
    maximum_trials: int = 8
    maximum_search_states: int = 4000
    step_nm: int = nm_from_mm("0.25")
    maximum_detour_nm: int = nm_from_mm(2)
    maximum_blockers: int = 4
    maximum_dependency_expansions: int = 2
    maximum_fragment_anchors: int = 64

    def __post_init__(self):
        if (any(type(value) is not int for value in (
                self.maximum_trials, self.maximum_search_states, self.step_nm,
                self.maximum_detour_nm, self.maximum_blockers, self.maximum_dependency_expansions,
                self.maximum_fragment_anchors))
                or self.maximum_trials < 0 or self.maximum_search_states <= 0
                or self.step_nm <= 0 or self.maximum_detour_nm < 0
                or not 1 <= self.maximum_blockers <= 8
                or not 0 <= self.maximum_dependency_expansions <= 8
                or not 1 <= self.maximum_fragment_anchors <= 256):
            raise ValueError("filled-copper feedback bounds are invalid")


@dataclass(frozen=True, slots=True)
class FilledCopperTrial:
    index: int
    input_digest: str
    net: str
    native_item_uuids: tuple[str, str]
    layer: str
    outcome: str
    blockers: tuple[RoutingBlocker, ...] = ()
    displaced_nets: tuple[str, ...] = ()
    dependency_expansions: int = 0
    expanded_states: int = 0
    dependency_searches: int = 0
    candidate_digest: str | None = None
    verification: PlaneVerification | None = None
    cleanup_removed_tracks: int = 0
    cleanup_removed_vias: int = 0


@dataclass(frozen=True, slots=True)
class FilledCopperFeedbackResult:
    board: PhysicalBoard
    pipeline: RoutingPipelineResult
    verification: PlaneVerification | None
    stop_reason: str
    trials: tuple[FilledCopperTrial, ...] = ()
    diagnostic: str | None = None

    def to_json(self):
        return json.dumps({"schema": "copperscript-filled-feedback/v0.1",
            "stop_reason": self.stop_reason, "diagnostic": self.diagnostic,
            "board_digest": physical_board_digest(self.board),
            "verification": json.loads(self.verification.to_json()) if self.verification else None,
            "trials": [{**asdict(item), "verification": json.loads(item.verification.to_json())
                        if item.verification else None} for item in self.trials]},
            indent=2, sort_keys=True) + "\n"


def _hard_findings(board):
    return tuple(item for item in run_physical_drc(board).findings
                 if item.severity.value == "error"
                 and item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})


def _open_nets(evidence):
    entries = [finding for finding in evidence.native_findings if finding.category == "unconnected"]
    if len(entries) != evidence.unconnected_count or any(
            finding.type != "unconnected_items" or not finding.items
            or any(item.net is None or item.kind == "unknown" for item in finding.items)
            for finding in entries):
        return None
    return frozenset(item.net for finding in entries for item in finding.items)


def _sample_anchors(groups, limit):
    """Bounded round-robin over components, then their accessible layers."""
    balanced = []
    for group in groups.values():
        layers, seen = {}, set()
        for component, owner, position in group:
            for layer in owner.layers:
                key = owner.uuid, position, layer
                if key not in seen:
                    seen.add(key)
                    layers.setdefault(layer, []).append((component, replace(owner, layers=(layer,)), position))
        balanced.append([values[index] for index in range(limit) for values in layers.values()
                         if index < len(values)][:limit])
    return [group[index] for index in range(limit) for group in balanced if index < len(group)][:limit]


def _finding_proposals(board, evidence, anchor_limit):
    eligible = distribution_zone_nets(board)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    objects = tuple(native_object_map(board).values())
    components = filled_component_roots(board, evidence, eligible)
    def root(item):
        return components.get(native_item_identity(item))
    def anchors(item):
        if item.kind != "zone":
            # A native nearest-item report can name a front track and a back
            # track although either connected copper component already owns a
            # plated transition. Follow actual bare copper, never a same-net
            # label, component-internal edge or an authored zone outline.
            connected = root(item)
            found = [(connected, item, point) for point in item.positions_nm]
            if connected is not None:
                additions = [(connected, owner, position) for owner in objects
                    if owner.uuid != item.uuid and owner.net == item.net and root(owner) == connected
                    for position in owner.positions_nm]
                additions.sort(key=lambda anchor: (
                    min(sum((a-b)**2 for a,b in zip(anchor[2], point)) for point in item.positions_nm),
                    anchor[1].uuid, anchor[2]))
                found.extend(additions)
            return _sample_anchors({connected: found}, anchor_limit)
        groups = {}
        # Native DRC can identify two fragments with the SAME zone UUID and
        # nominal origin. Membership is measured in the freshly filled polygon,
        # not the authored zone outline or description string.
        for fragment in evidence.filled_regions:
            if fragment.zone_uuid != item.uuid:
                continue
            polygon = tuple(Point(*point) for point in fragment.vertices_nm)
            identity = f"fill:{fragment.fragment_digest}"
            component = components.get(identity, identity)
            found = []
            for owner in objects:
                if (owner.net != item.net or owner.kind not in {"pad", "segment", "via"}
                        or fragment.layer not in owner.layers):
                    continue
                for position in owner.positions_nm:
                    if point_in_polygon(Point(*position), polygon):
                        found.append((component,
                            replace(owner, layers=(fragment.layer,)), position))
                        if len(found) >= anchor_limit:
                            break
                if len(found) >= anchor_limit:
                    break
            if found:
                groups.setdefault(component, []).extend(found)
        # A report may name a back-layer zone while the missing component only
        # has front lands. Follow plated contacts and fresh fill to accessible
        # surfaces; never invent a via or use a zone's nominal origin.
        for component, found in groups.items():
            found.extend((component, owner, position) for owner in objects
                if owner.net == item.net and root(owner) == component
                for position in owner.positions_nm)
        # A busy first fragment must not consume the entire anchor allowance
        # before any other fragment is represented. Keep the overall cap while
        # sampling deterministically round-robin across eligible fragments.
        return _sample_anchors(groups, anchor_limit)
    for finding in evidence.native_findings:
        if finding.category != "unconnected" or len(finding.items) != 2:
            continue
        first, second = finding.items
        if (first.net != second.net or first.net not in eligible
                or first.kind not in {"pad", "segment", "via", "zone"}
                or second.kind not in {"pad", "segment", "via", "zone"}):
            continue
        rule = rules.get(first.net)
        if rule and rule.kind is not RouteKind.GENERAL:
            continue
        allowed = set(layer.value for layer in (rule.allowed_layers if rule and rule.allowed_layers
                                               else board.stackup.copper_layers))
        pairs = sorted(((a, b) for a, b in product(anchors(first), anchors(second))
                        if a[2] != b[2] and not (a[0] is not None and a[0] == b[0])),
                       key=lambda pair: (sum((a-b)**2 for a,b in zip(pair[0][2], pair[1][2])),
                                         pair[0][1].uuid, pair[1][1].uuid, pair[0][2], pair[1][2]))
        for (_, a, start), (_, b, end) in pairs:
            for layer in sorted(set(a.layers) & set(b.layers) & allowed):
                yield first.net, (a.uuid, b.uuid), CopperLayer(layer), Point(*start), Point(*end)


def _proposals(board, evidence, anchor_limit):
    """Share trials across open nets, without materializing every anchor pair."""
    if not evidence.matches(board):
        return
    grouped = {}
    for finding in evidence.native_findings:
        if finding.category == "unconnected" and len(finding.items) == 2:
            net = finding.items[0].net
            grouped.setdefault(net, []).append(_finding_proposals(
                board, replace(evidence, native_findings=(finding,)), anchor_limit))
    queues = [iter(chain.from_iterable(groups)) for groups in grouped.values()]
    seen = set()
    while queues:
        active = []
        for queue in queues:
            for proposal in queue:
                net, _, layer, start, end = proposal
                key = _proposal_key(net, layer, start, end)
                if key not in seen:
                    seen.add(key)
                    yield proposal
                    active.append(queue)
                    break
        queues = active


def _proposal_key(net, layer, start, end):
    return net, layer, tuple(sorted(((start.x_nm, start.y_nm), (end.x_nm, end.y_nm))))


def _rejection_context(board, evidence, net, dependency_nets):
    """Ignore only unrelated copper on OTHER layers when reusing a rejection.

    A bridge search and dependency reroute can explore beyond a local box.
    Keep all same-layer copper, every via, and entire involved-net geometry;
    changes there invalidate the cache. Source intent/poses/options are invariant
    within this transaction. No cross-run cache or board-specific selectors.
    """
    involved = {net, *dependency_nets}
    layers = {p.layer for p in board.tracks if p.net in involved}
    layers.update(layer for zone in board.zones if zone.net == net for layer in zone.layers)
    if dependency_nets:
        # A displaced ordinary owner may reroute on a previously unused layer.
        # Do not cache against only the layers of its incumbent route.
        layers.update(board.stackup.copper_layers)
    return (frozenset(Counter(t for t in board.tracks if t.net in involved or t.layer in layers).items()),
            frozenset(Counter(board.vias).items()),
            frozenset(fragment.fragment_digest for fragment in evidence.filled_regions if fragment.net in involved))


def improve_filled_copper(
    initial: RoutingPipelineResult, board: PhysicalBoard | None = None, *,
    kicad_cli: Path, options: FilledCopperFeedbackOptions | None = None,
    detailed_options: DetailedRouterOptions | None = None,
    artifact_directory: Path | None = None, runner: CommandRunner | None = None,
    on_progress: ProgressCallback | None = None,
) -> FilledCopperFeedbackResult:
    """Refill first, repair only verified opens, then independently refill trials.

    All failed candidate geometry is discarded. A strict reduction in native
    opens, no newly open net, no islands/physical failures and no new native
    non-library violations is mandatory. Partial progress is not signoff.
    """
    options = options or FilledCopperFeedbackOptions()
    detailed_options = detailed_options or DetailedRouterOptions()
    board = board or initial.board
    if board.placements != initial.board.placements or replace(
            board, tracks=initial.board.tracks, vias=initial.board.vias,
            metadata=initial.board.metadata) != initial.board:
        raise ValueError("filled feedback requires matching routed source intent and poses")
    if artifact_directory is not None:
        artifact_directory.mkdir(parents=True, exist_ok=False)
    verification_count = 0
    def verify(candidate):
        nonlocal verification_count
        verification_count += 1
        directory = (artifact_directory / f"verification-{verification_count:03d}"
                     if artifact_directory is not None else None)
        return verify_filled_planes(candidate, kicad_cli=kicad_cli, runner=runner,
                                    artifact_directory=directory)
    def tool_failure(exc):
        reason = ("missing_tool" if isinstance(exc, FileNotFoundError) else
                  "unsupported_tool" if "qualified major" in str(exc) else "invalid_native_result")
        return FilledCopperFeedbackResult(board, initial, None, reason, diagnostic=str(exc))
    try:
        evidence = verify(board)
    except (OSError, RuntimeError) as exc:
        return tool_failure(exc)
    if evidence.passed:
        return FilledCopperFeedbackResult(board, initial, evidence, "verified")
    if initial.package_access is not None and not initial.package_access.ready:
        return FilledCopperFeedbackResult(board, initial, evidence, "package_access_blocked")
    if _hard_findings(board):
        return FilledCopperFeedbackResult(board, initial, evidence, "physical_violations")
    if evidence.island_count:
        return FilledCopperFeedbackResult(board, initial, evidence, "source_zone_design",
            diagnostic="Native isolated copper requires source-zone review; routing intent is unchanged.")
    if not evidence.unconnected_count:
        return FilledCopperFeedbackResult(board, initial, evidence, "native_violations")
    known_opens = _open_nets(evidence)
    if known_opens is None:
        return FilledCopperFeedbackResult(board, initial, evidence, "unsupported_native_objects")
    if options.maximum_trials == 0:
        return FilledCopperFeedbackResult(board, initial, evidence, "disabled")

    # Only completed ordinary AREA ownership is mutable. Occurrence subtraction
    # prevents duplicate segment geometry from erasing another owner's copper.
    zone_nets = distribution_zone_nets(board)
    specialized = {rule.net for rule in board.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    if not known_opens & (zone_nets - specialized):
        return FilledCopperFeedbackResult(board, initial, evidence, "no_eligible_zone_opens",
            diagnostic="Known native opens remain outside general zone-bridge scope; inspect ordinary/critical routing failures.")
    eligible = frozenset(item.net for item in initial.detailed.nets
                         if item.connected and item.net not in zone_nets | specialized)
    prefix = (initial.fanout.board if initial.fanout else
              initial.plane_stitch.board if initial.plane_stitch else initial.critical.board)
    protected_tracks, protected_vias = Counter(prefix.tracks), Counter(prefix.vias)
    def partition(objects, protected):
        fixed, mutable = [], []
        remaining = protected.copy()
        for item in objects:
            if remaining[item] or item.net not in eligible:
                fixed.append(item)
                remaining[item] -= bool(remaining[item])
            else:
                mutable.append(item)
        return tuple(fixed), tuple(mutable)
    records, seen = [], {physical_board_digest(board)}
    rejected = {}
    pipeline = initial
    reason, diagnostic = "stagnation", None
    while evidence.unconnected_count and len(records) < options.maximum_trials:
        improved, proposed = False, False
        for net, uuids, layer, start, end in _proposals(board, evidence, options.maximum_fragment_anchors):
            proposed = True
            key = _proposal_key(net, layer, start, end)
            previous = rejected.get(key)
            if previous and previous[1] == _rejection_context(board, evidence, net, previous[0]):
                continue
            if len(records) >= options.maximum_trials:
                reason = "trial_budget_exhausted"
                break
            fixed_tracks, mutable_tracks = partition(board.tracks, protected_tracks)
            fixed_vias, mutable_vias = partition(board.vias, protected_vias)
            fixed = replace(board, tracks=fixed_tracks, vias=fixed_vias)
            clearance = RoutingClearanceIndex(fixed)
            full = RoutingClearanceIndex(fixed)
            for item in mutable_tracks:
                full.add_track(item)
            for item in mutable_vias:
                full.add_via(item)
            work = {"states": 0, "dependency_searches": 0}
            failure_blockers = []
            def observe(phase, event, data):
                if phase == "surface_bridge_search" and event == "finished":
                    work["states"] += data["expanded_states"]
                    failure_blockers.extend(RoutingBlocker(**item) for item in data.get("blocker_records", ()))
                if phase == "zone_subset_search" and event == "started":
                    work["dependency_searches"] += 1
                if on_progress:
                    on_progress(phase, event, data)
            width = next((rule.width_nm for rule in board.net_routing_rules
                          if rule.net == net and rule.width_nm is not None), board.rules.default_track_width_nm)
            # Prefer the empty blocker set. Spend at most a quarter of the
            # shared per-trial state budget against ALL copper before proposing
            # a path with mutable area owners removed. Never commit a relaxed
            # path without restoring/rerouting its exact blockers below.
            bridge = None
            if mutable_tracks or mutable_vias:
                bridge = surface_path_between(board, full, net, start, end, width, layer,
                    step_nm=options.step_nm, maximum_detour_nm=options.maximum_detour_nm,
                    state_budget=max(1, options.maximum_search_states//4), on_progress=observe)
            remaining_states = options.maximum_search_states-work["states"]
            if bridge is None and remaining_states > 0:
                bridge = surface_path_between(fixed, clearance, net, start, end, width, layer,
                    step_nm=options.step_nm, maximum_detour_nm=options.maximum_detour_nm,
                    state_budget=remaining_states, on_progress=observe)
            blockers, displaced, expansions, checked, digest = tuple(failure_blockers), frozenset(), 0, None, None
            outcome = "no_legal_surface_bridge"
            candidate = None
            cleaned_tracks = cleaned_vias = 0
            if bridge:
                blockers = tuple(sorted({item for track in bridge
                                         for item in full.track_blocker_records(track)},
                                        key=lambda item: item.identity))
                displaced = frozenset(item.net for item in blockers if item.net is not None)
                if any(item.immutable for item in blockers):
                    outcome = "immutable_blocker"
                elif not displaced <= eligible or len(displaced) > options.maximum_blockers:
                    outcome = "ineligible_or_oversized_blocker_set"
                else:
                    reserved = replace(fixed, tracks=(*fixed.tracks, *bridge))
                    repaired = _reroute_local_dependencies(pipeline, reserved, mutable_tracks, mutable_vias,
                        displaced, eligible, detailed_options, EscapeFeedbackOptions(
                            maximum_local_blockers=options.maximum_blockers,
                            maximum_dependency_expansions=options.maximum_dependency_expansions),
                        on_progress=observe)
                    outcome = "dependency_reroute_failed"
                    if repaired is not None:
                        candidate, reroute, displaced, expansions = repaired
                        expected = _with_preserved_area(reserved, mutable_tracks, mutable_vias, displaced)
                        digest = physical_board_digest(candidate)
                        if (_unaffected_copper(candidate, displaced) != _unaffected_copper(expected, displaced)
                                or Counter(reserved.tracks)-Counter(candidate.tracks)
                                or Counter(reserved.vias)-Counter(candidate.vias)):
                            outcome = "ownership_changed"
                        elif digest in seen:
                            outcome = "repeated_state"
                        elif _hard_findings(candidate):
                            outcome = "physical_violations"
                        else:
                            seen.add(digest)
                            try:
                                checked = verify(candidate)
                                before_cleanup = candidate
                                candidate, checked = remove_native_dangling_copper(candidate, checked,
                                    Counter(reserved.tracks), Counter(reserved.vias),
                                    eligible_nets=displaced, kicad_cli=kicad_cli, verifier=verify)
                                cleaned_tracks = len(before_cleanup.tracks) - len(candidate.tracks)
                                cleaned_vias = len(before_cleanup.vias) - len(candidate.vias)
                                digest = physical_board_digest(candidate)
                                seen.add(digest)
                            except (OSError, RuntimeError) as exc:
                                failure = tool_failure(exc)
                                records.append(FilledCopperTrial(len(records)+1, physical_board_digest(board),
                                    net, uuids, layer.value, "native_tool_failure", blockers,
                                    tuple(sorted(displaced)), expansions, work["states"],
                                    work["dependency_searches"], digest))
                                return replace(failure, board=board, pipeline=pipeline, verification=evidence,
                                               trials=tuple(records))
                            old_nets, new_nets = _open_nets(evidence), _open_nets(checked)
                            hard_native = [finding for finding in checked.native_findings
                                if finding.category == "violation" and finding.type not in
                                {"lib_footprint_issues", "lib_footprint_mismatch"}]
                            outcome = "native_not_improved"
                            if (checked.matches(candidate) and not hard_native and not checked.island_count
                                    and new_nets is not None and new_nets <= old_nets
                                    and checked.unconnected_count < evidence.unconnected_count):
                                outcome = "accepted"
            record = FilledCopperTrial(len(records)+1, physical_board_digest(board), net, uuids,
                layer.value, outcome, blockers, tuple(sorted(displaced)), expansions,
                work["states"], work["dependency_searches"], digest, checked, cleaned_tracks, cleaned_vias)
            records.append(record)
            # Failed native trials have unchanged input geometry. Do not repeat
            # them merely because another rail accepted an unrelated bridge.
            if outcome == "native_not_improved":
                rejected[key] = (displaced, _rejection_context(board, evidence, net, displaced))
            emit(on_progress, "filled_copper_trial", "finished", index=record.index,
                 net=net, outcome=outcome, displaced_nets=list(record.displaced_nets),
                 cleanup_removed_tracks=cleaned_tracks, cleanup_removed_vias=cleaned_vias)
            if outcome == "accepted":
                merged = _merge_local_detail(pipeline, candidate, reroute, displaced)
                # Closure must not alter geometry after this native certificate.
                if merged.board.tracks != candidate.tracks or merged.board.vias != candidate.vias:
                    records[-1] = replace(record, outcome="closure_changed_verified_geometry")
                    continue
                pipeline = merged
                board, evidence, improved = candidate, checked, True
                protected_tracks.update(bridge)
                if not evidence.unconnected_count:
                    reason = "verified" if evidence.passed else "native_violations"
                break
        if not improved:
            reason = ("trial_budget_exhausted" if len(records) >= options.maximum_trials else
                      "stagnation" if proposed else
                      "no_eligible_surface_bridge" if _open_nets(evidence) & (zone_nets - specialized)
                      else "no_eligible_zone_opens")
            if not proposed:
                diagnostic = ("Known native zone opens have no supported same-surface terminal/fragment pair; routing intent is unchanged."
                    if reason == "no_eligible_surface_bridge" else
                    "Known native opens remain outside general zone-bridge scope; inspect ordinary/critical routing failures.")
            break
    if evidence.unconnected_count and len(records) >= options.maximum_trials:
        reason = "trial_budget_exhausted"
    return FilledCopperFeedbackResult(board, pipeline, evidence, reason, tuple(records), diagnostic)
