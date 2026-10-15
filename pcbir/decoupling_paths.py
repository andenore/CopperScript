"""Bounded explicit supply/return joins for opposite-surface decouplers.

Only original straight-track and ordinary plated-through-via edges prove paths.
Zones, pad groups and component internals are not inferred. A return witness is
a physical join to a source-declared net terminal, not electrical qualification.
"""
from __future__ import annotations

from collections import defaultdict, deque
from dataclasses import dataclass, replace
from decimal import Decimal
from itertools import islice
from math import hypot

from .physical import (BoardSide, CopperLayer, DecouplingLink, PadKind,
                       PadReference, PhysicalBoard, Point, RouteKind,
                       TrackSegment, Via, ViaKind)
from .drc import DrcSeverity, physical_board_digest, run_physical_drc
from .fanout import FanoutOptions, _candidates, _legal_choices, _two_leg_candidates
from .hard_macros import macro_owned_pads
from .pad_via_arrays import require_via_in_pad_arrays
from .pin_escape import checked_access_paths, _verified_land_path
from .pin_access import local_access_path
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .routing_vias import physical_via_span
from .surface_path import _track_inside_board, via_inside_board


@dataclass(frozen=True, slots=True)
class DecouplingPathNode:
    position: Point
    layer: CopperLayer


@dataclass(frozen=True, slots=True)
class DecouplingPathStep:
    edge: TrackSegment | Via
    start: DecouplingPathNode
    end: DecouplingPathNode


@dataclass(frozen=True, slots=True)
class DecouplingCopperPath:
    start: PadReference
    end: PadReference
    net: str
    steps: tuple[DecouplingPathStep, ...]

    @property
    def tracks(self) -> tuple[TrackSegment, ...]:
        return tuple(dict.fromkeys(s.edge for s in self.steps if isinstance(s.edge, TrackSegment)))

    @property
    def vias(self) -> tuple[Via, ...]:
        return tuple(dict.fromkeys(s.edge for s in self.steps if isinstance(s.edge, Via)))


@dataclass(frozen=True, slots=True)
class DecouplingCrossLayerWitness:
    link: DecouplingLink
    supply_path: DecouplingCopperPath
    return_target: PadReference
    return_path: DecouplingCopperPath
    context_digest: str
    board_digest: str
    qualification: str = "Physical declared-return-net join only; component internals and electrical return qualification unproved."


@dataclass(frozen=True, slots=True)
class DecouplingPathResult:
    board: PhysicalBoard
    created_tracks: tuple[TrackSegment, ...] = ()
    created_vias: tuple[Via, ...] = ()
    witness: DecouplingCrossLayerWitness | None = None
    reason: str = ""
    supply_path: DecouplingCopperPath | None = None
    return_path: DecouplingCopperPath | None = None

    @property
    def supply_verified(self) -> bool:
        return self.supply_path is not None

    @property
    def return_verified(self) -> bool:
        return self.return_path is not None


def _context(board):
    if board.materialized_macros:
        from .hard_macros import macro_source, resolved_macro_geometry
        from .pad_via_arrays import via_in_pad_array_vias
        tracks, vias = [], []
        for macro in board.hard_macros:
            if macro.cluster in board.materialized_macros:
                owned_tracks, owned_vias, _, _ = resolved_macro_geometry(board, macro)
                tracks.extend(owned_tracks)
                vias.extend(owned_vias)
        # macro_source accepts exactly the immutable prefix, not a routed board.
        # Keep that prefix valid while recovering its unchanged source contract.
        prefix = replace(board, tracks=tuple(tracks),
                         vias=(*vias, *via_in_pad_array_vias(board)), zone_fills=())
        source = macro_source(prefix)
    else:
        source = board
    return physical_board_digest(replace(source, tracks=(), vias=(), zone_fills=()))


def _land(board, reference):
    pose = next((p for p in board.placements if p.reference == reference.component), None)
    if pose is None:
        return None
    pads = tuple(p for p in board.footprints[pose.footprint].pads if p.number == reference.pad)
    if len(pads) != 1 or pads[0].kind is not PadKind.SMD:
        return None
    layer = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    return pose, pads[0], DecouplingPathNode(transformed_local_point(pose, pads[0].position), layer)


def _return_targets(board, link):
    net = next((n for n in board.nets if n.name == link.return_net), None)
    return tuple(sorted(p for p in net.pads if p.component == link.target.component)) if net else ()


def _source_reason(board, link):
    if link not in board.decoupling_links:
        return "undeclared_association"
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        return "unmaterialized_macro_owner"
    owned = macro_owned_pads(board)
    if link.capacitor in owned or link.target in owned or link.return_pad in owned:
        return "immutable_macro_association"
    try:
        require_via_in_pad_arrays(board, "cross-layer decoupling")
    except ValueError:
        return "missing_required_array"
    rule = next((r for r in board.net_routing_rules if r.net == link.net), None)
    if rule and rule.kind is not RouteKind.GENERAL:
        return "specialized_supply_owner"
    returned = next((r for r in board.net_routing_rules if r.net == link.return_net), None)
    if returned and returned.kind is not RouteKind.GENERAL:
        return "specialized_return_owner"
    first, second = _land(board, link.target), _land(board, link.capacitor)
    if first is None or second is None or _land(board, link.return_pad) is None:
        return "unsupported_or_ambiguous_land"
    if first[2].layer is second[2].layer:
        return "opposite_surfaces_required_use_legacy_surface_helper"
    return ""


def _rule(board, net):
    return next((r for r in board.net_routing_rules if r.net == net), None)


def _budget_reason(board, net):
    rule = _rule(board, net)
    if rule is None:
        return ""
    if rule.max_vias is not None and sum(v.net == net for v in board.vias) > rule.max_vias:
        return "whole_net_via_budget_exceeded"
    if rule.max_length_nm is not None:
        length = sum(round(hypot(t.end.x_nm-t.start.x_nm, t.end.y_nm-t.start.y_nm))
                     for t in board.tracks if t.net == net)
        if length > rule.max_length_nm:
            return "whole_net_length_budget_exceeded"
    return ""


def _normal_via(board, index, via):
    outer = (board.stackup.copper_layers[0], board.stackup.copper_layers[-1])
    if {via.from_layer, via.to_layer} != set(outer) or via.finish != "standard":
        return False
    technologies = {t.id: t for t in board.stackup.via_technologies}
    if technologies:
        technology = technologies.get(via.technology)
        if technology is None or technology.kind is not ViaKind.THROUGH:
            return False
        if (via.drill_nm < technology.minimum_drill_nm
                or (via.size_nm-via.drill_nm)//2 < technology.minimum_annular_ring_nm
                or Decimal(board.stackup.thickness_nm)/Decimal(via.drill_nm) > technology.maximum_aspect_ratio):
            return False
    elif via.technology is not None:
        return False
    return (via_inside_board(board, via.position, via.size_nm)
            and index.can_via(via.net, via.position, via.size_nm, via.from_layer,
                              via.to_layer, drill_nm=via.drill_nm))


def _on(point, track):
    a,b = track.start,track.end
    return ((point.x_nm-a.x_nm)*(b.y_nm-a.y_nm) == (point.y_nm-a.y_nm)*(b.x_nm-a.x_nm)
            and min(a.x_nm,b.x_nm) <= point.x_nm <= max(a.x_nm,b.x_nm)
            and min(a.y_nm,b.y_nm) <= point.y_nm <= max(a.y_nm,b.y_nm))


def _track_ok(board, index, track, net):
    rule = _rule(board, net)
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    return (track.net == net and track.layer in routing_layers(board, net, rule)
            and track.width_nm >= max(board.rules.minimum_track_width_nm,
                                      index.breakout.required_width_nm(track, width))
            and _track_inside_board(board, track.start, track.end, track.width_nm)
            and index.can_track(net, track.start, track.end, track.width_nm, track.layer))


def _find_path(board, start, ends, net, index, maximum_states):
    origin = _land(board, start)
    terminals = {land[2]: ref for ref in ends if (land := _land(board, ref)) is not None}
    if origin is None or not terminals or _budget_reason(board, net):
        return None
    allowed = routing_layers(board, net, _rule(board, net))
    first = origin[2]
    if first.layer not in allowed:
        return None
    terminals = {node: ref for node,ref in terminals.items() if node.layer in allowed}
    tracks = tuple(t for t in board.tracks if _track_ok(board, index, t, net))
    vias = tuple(v for v in board.vias if v.net == net and _normal_via(board, index, v))
    points = {first.position, *(n.position for n in terminals),
              *(p for t in tracks for p in (t.start,t.end)), *(v.position for v in vias)}
    graph = defaultdict(list)
    for track in tracks:
        on = sorted((p for p in points if _on(p, track)),
                    key=lambda p: (p.x_nm-track.start.x_nm)**2+(p.y_nm-track.start.y_nm)**2)
        for a,b in zip(on,on[1:]):
            x,y = DecouplingPathNode(a,track.layer),DecouplingPathNode(b,track.layer)
            graph[x].append((y,track));graph[y].append((x,track))
    for via in vias:
        nodes = [DecouplingPathNode(via.position,layer) for layer in allowed]
        for a in nodes:
            graph[a].extend((b,via) for b in nodes if b != a)
    queue = deque((first,));parents = {first: None}
    while queue and len(parents) <= maximum_states:
        node = queue.popleft()
        if node in terminals:
            steps=[];cursor=node
            while parents[cursor] is not None:
                previous,edge=parents[cursor]
                steps.append(DecouplingPathStep(edge,previous,cursor));cursor=previous
            return DecouplingCopperPath(start,terminals[node],net,tuple(reversed(steps)))
        for neighbor,edge in graph[node]:
            if neighbor not in parents:
                parents[neighbor]=(node,edge);queue.append(neighbor)
    return None


def verify_crosslayer_decoupling(board, link, *, return_target=None,
                                maximum_path_states=10_000, index=None):
    """Read-only exact supply AND return witnesses from current original edges.

    An optional freshly constructed clearance index must reference this exact
    immutable board object. Callers must not mutate the shared index.
    """
    if maximum_path_states <= 0:
        raise ValueError("path state bound must be positive")
    if index is not None and index.board is not board:
        return DecouplingPathResult(board, reason="stale_clearance_index")
    reason = _source_reason(board, link)
    if reason:
        return DecouplingPathResult(board, reason=reason)
    targets = _return_targets(board, link)
    if return_target is not None:
        if return_target not in targets:
            return DecouplingPathResult(board, reason="return_target_not_declared_on_target_component")
        targets = (return_target,)
    index = RoutingClearanceIndex(board) if index is None else index
    supply = _find_path(board, link.target, (link.capacitor,), link.net, index, maximum_path_states)
    returned = _find_path(board, link.return_pad, targets, link.return_net, index, maximum_path_states)
    if supply is None:
        return DecouplingPathResult(board,return_path=returned,
                                   reason=_budget_reason(board,link.net) or "supply_path_absent")
    if returned is None:
        return DecouplingPathResult(board, supply_path=supply,
            reason=_budget_reason(board,link.return_net) or ("return_path_absent" if targets else "no_declared_target_return_pad"))
    witness = DecouplingCrossLayerWitness(link,supply,returned.end,returned,
                                         _context(board),physical_board_digest(board))
    return DecouplingPathResult(board,witness=witness,supply_path=supply,return_path=returned)


def validate_crosslayer_witness(board, witness, *, index=None):
    """Fresh validation of the exact recorded path, allowing appended copper.

    A supplied fresh index must belong to this exact board and remain unmutated.
    """
    if index is not None and index.board is not board:
        return "stale_clearance_index"
    if (not isinstance(witness, DecouplingCrossLayerWitness)
            or not isinstance(witness.link, DecouplingLink)
            or not isinstance(witness.return_target, PadReference)
            or any(not isinstance(path, DecouplingCopperPath)
                   or not isinstance(path.start, PadReference)
                   or not isinstance(path.end, PadReference)
                   or not isinstance(path.net, str)
                   or not isinstance(path.steps, tuple)
                   or any(not isinstance(step, DecouplingPathStep)
                          or any(not isinstance(node, DecouplingPathNode)
                                 or not isinstance(node.position, Point)
                                 or not isinstance(node.layer, CopperLayer)
                                 for node in (step.start, step.end))
                          for step in path.steps)
                   for path in (witness.supply_path, witness.return_path))):
        return "untyped_admitted_bypass_witness"
    if witness.context_digest != _context(board) or _source_reason(board,witness.link):
        return "stale_witness_context"
    link=witness.link
    if (witness.supply_path.start != link.target or witness.supply_path.end != link.capacitor
            or witness.supply_path.net != link.net or witness.return_path.start != link.return_pad
            or witness.return_path.end != witness.return_target or witness.return_path.net != link.return_net
            or witness.return_target not in _return_targets(board,link)):
        return "witness_terminal_identity_mismatch"
    index=RoutingClearanceIndex(board) if index is None else index
    for path in (witness.supply_path,witness.return_path):
        if _budget_reason(board,path.net):
            return "witness_budget_exceeded"
        first,last=_land(board,path.start),_land(board,path.end)
        if first is None or last is None or not path.steps:
            return "witness_land_absent"
        cursor=first[2]
        visited={cursor}
        for step in path.steps:
            if step.start != cursor:
                return "witness_path_discontinuous"
            if step.end in visited:
                return "witness_path_cycle"
            edge=step.edge
            if isinstance(edge,TrackSegment):
                if (not any(edge is item for item in board.tracks) or not _track_ok(board,index,edge,path.net)
                        or step.start.layer is not edge.layer or step.end.layer is not edge.layer
                        or not _on(step.start.position,edge) or not _on(step.end.position,edge)):
                    return "witness_track_invalid"
            elif isinstance(edge,Via):
                if (not any(edge is item for item in board.vias) or edge.net != path.net
                        or not _normal_via(board,index,edge) or step.start.position != edge.position
                        or step.end.position != edge.position or step.start.layer is step.end.layer
                        or step.start.layer not in routing_layers(board,path.net,_rule(board,path.net))
                        or step.end.layer not in routing_layers(board,path.net,_rule(board,path.net))):
                    return "witness_via_invalid"
            else:
                return "witness_unknown_edge"
            cursor=step.end
            visited.add(cursor)
        if cursor != last[2]:
            return "witness_terminal_moved"
    return ""


def refresh_crosslayer_witnesses(board, witnesses, *, index=None, replacing_links=frozenset()):
    """Fresh same-path validation; never waive the original source context.

    Return immutable original-edge witnesses bound to the current copper
    digest, or no witnesses and an explicit reason. This never searches or
    creates copper. Empty legacy ledgers require no index or digest work.
    """
    if not isinstance(witnesses, tuple) or any(not isinstance(w, DecouplingCrossLayerWitness) for w in witnesses):
        return (), "untyped_admitted_bypass_witness"
    if not witnesses:
        return (), None
    index = RoutingClearanceIndex(board) if index is None else index
    retained=[]
    for witness in witnesses:
        reason = validate_crosslayer_witness(board, witness, index=index)
        if reason:
            if witness.link in replacing_links:
                continue  # This exact association is rebuilt by the caller.
            return (), reason
        retained.append(witness)
    digest = physical_board_digest(board)
    return tuple(replace(witness, board_digest=digest) for witness in retained), None


@dataclass(slots=True)
class _ConnectionWork:
    # One allowance per outer transaction, shared by every lazy connection
    # generator, supply, return and return target. Never a new source maze.
    local_paths: int = 0


def _existing_launches(board, a, b, net, width, index, options, site_limit):
    """Observe exact original-edge source paths; never infer a package join."""
    radius = options.maximum_radius_nm
    sites = sorted((v for v in board.vias if v.net == net
                    and (v.position.x_nm-a[2].position.x_nm)**2
                    + (v.position.y_nm-a[2].position.y_nm)**2 <= radius**2),
                   key=lambda v: ((v.position.x_nm-b[2].position.x_nm)**2
                                  + (v.position.y_nm-b[2].position.y_nm)**2,
                                  v.position.x_nm, v.position.y_nm))
    for via in islice(sites, site_limit):
        if not _normal_via(board, index, via):
            continue
        # verified_fanout_path deliberately expands internal-pad groups; this
        # explicit supply/return contract requires the exact source terminal.
        path = _verified_land_path(board, a[1], a[0], net, via.position, index,
                                   target_layer=b[2].layer, required_width_nm=width)
        if path is not None and all(_track_ok(board, index, t, net) for t in path):
            yield path, via


def _connections(board, first, second, net, options, launch_limit, site_limit, *, work=None):
    from .track_containment import track_is_contained
    work = work if work is not None else _ConnectionWork()
    a,b=_land(board,first),_land(board,second)
    if a is None or b is None or a[2].layer is b[2].layer:
        return
    rule=_rule(board,net);allowed=routing_layers(board,net,rule)
    if (a[2].layer not in allowed or b[2].layer not in allowed
            or rule and rule.max_vias == 0):
        return
    span=physical_via_span(board,a[2].layer,b[2].layer)
    if span is None or {span[0],span[1]} != {board.stackup.copper_layers[0],board.stackup.copper_layers[-1]}:
        return
    index=RoutingClearanceIndex(board)
    width=rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm

    def launches():
        admitted=0
        for path,via in _existing_launches(board, a, b, net, width, index, options, site_limit):
            admitted+=1
            yield path,via,True
            if admitted>=launch_limit:
                return
        # Existing launches spend the SAME finite slots before ordinary
        # candidates; no extra launch or complete-proposal allowance is added.
        remaining=launch_limit-admitted
        settings=(options,replace(options,step_nm=options.refinement_step_nm))
        fresh=[]
        for config in settings:
            endpoints=islice(_candidates(a[2].position,a[0].position,config,
                                        diagonal_normal=a[0].rotation_degrees%90==45),site_limit)
            fresh.extend(_legal_choices(board,index,net,a[2].position,a[0],a[1],endpoints))
            if len(fresh)>=remaining:
                break
            if config.two_leg_escapes:
                fresh.extend(_legal_choices(board,index,net,a[2].position,a[0],a[1],
                    islice(_two_leg_candidates(a[2].position,a[0].position,config),site_limit),multiple_orders=True))
            if len(fresh)>=remaining:
                break
        for path,via in tuple(dict.fromkeys(fresh))[:remaining]:
            existing=next((v for v in board.vias if v.net==net and v.position==path[-1].end
                           and _normal_via(board,index,v)),None)
            selected=via or existing
            if selected is not None and _normal_via(board,index,selected):
                yield path,selected,False

    for path,selected,reused in launches():
        tracks=tuple(dict.fromkeys(t for t in path if not track_is_contained(t, board.tracks)))
        added_vias=(selected,) if selected not in board.vias else ()
        try:
            launched=replace(board,tracks=(*board.tracks,*tracks),vias=(*board.vias,*added_vias))
        except ValueError:
            continue
        fresh=RoutingClearanceIndex(launched)
        tails=tuple(checked_access_paths(launched,fresh,net,selected.position,b[2].position,width,b[2].layer))
        if not tails and reused and options.maze_escapes and work.local_paths < 4:
            work.local_paths+=1
            tail=local_access_path(launched,fresh,net,selected.position,b[2].position,b[2].layer,width,
                                   step_nm=100_000,detour_nm=1_000_000,maximum_states=4000)
            tails=() if tail is None else (tail,)
        for tail in tails:
            additions=tuple(dict.fromkeys(t for t in tail if not track_is_contained(t, launched.tracks)))
            try:
                candidate=replace(launched,tracks=(*launched.tracks,*additions))
            except ValueError:
                continue
            if not _budget_reason(candidate,net):
                yield candidate


def propose_crosslayer_decoupling(board, link, *, fanout_options=None,
                                 maximum_launch_candidates=8, maximum_return_candidates=8,
                                 maximum_site_candidates=64, maximum_proposals=64,
                                 maximum_path_states=10_000, protected_witnesses=(),
                                 return_target=None):
    """Atomically propose normal-through supply and explicit-pad return routes.

    Candidate counts/radius are computational bounds, never electrical limits.
    Existing same-net vias examined per connection are capped by the site bound;
    verified reuse spends the existing launch slots before new launches. One
    shared transaction permits at most four local 100 um/1 mm/4000-state tails,
    only for verified existing launches after cheap tails fail and when the
    fanout maze option is enabled. Source paths are recognized, never searched.
    Rejection returns the original board and no adopted generated copper.
    """
    if min(maximum_launch_candidates,maximum_return_candidates,maximum_site_candidates,
           maximum_proposals,maximum_path_states)<=0:
        raise ValueError("decoupling proposal bounds must be positive")
    index=RoutingClearanceIndex(board)
    existing=verify_crosslayer_decoupling(board,link,return_target=return_target,
                                         maximum_path_states=maximum_path_states,index=index)
    if _source_reason(board,link):
        return existing
    if return_target is not None and return_target not in _return_targets(board,link):
        return existing
    protected=list(protected_witnesses)
    for other in board.decoupling_links:
        if other != link:
            measured=verify_crosslayer_decoupling(board,other,maximum_path_states=maximum_path_states,index=index)
            if measured.witness is not None:
                protected.append(measured.witness)
    if any(validate_crosslayer_witness(board,w,index=index) for w in protected):
        return DecouplingPathResult(board,reason="protected_witness_invalid")
    if existing.witness is not None:
        return existing
    targets=_return_targets(board,link)
    if return_target is not None:
        targets=(return_target,)
    if not targets:
        return DecouplingPathResult(board,reason="no_declared_target_return_pad",supply_path=existing.supply_path)
    options=fanout_options or FanoutOptions()
    if existing.return_path is not None:
        # The fresh complete half is already an observed path, not another
        # target search. Distance truncation must not discard its explicit end.
        # Initial verification also constrains this end to return_target.
        targets=(existing.return_path.end,)
    else:
        cap=_land(board,link.return_pad)[2].position
        targets=tuple(sorted((p for p in targets if _land(board,p)),key=lambda p:(
            (_land(board,p)[2].position.x_nm-cap.x_nm)**2+(_land(board,p)[2].position.y_nm-cap.y_nm)**2,p)))[:maximum_return_candidates]
    baseline={f.fingerprint for f in run_physical_drc(board).findings
              if f.severity is DrcSeverity.ERROR and f.code not in {"DRC-OPEN-NET","DRC-ROUTE-INCOMPLETE"}}
    work=_ConnectionWork()
    supplies=(board,) if existing.supply_path is not None else _connections(
        board,link.target,link.capacitor,link.net,options,maximum_launch_candidates,maximum_site_candidates,work=work)
    attempted=0
    uncovered=False
    for supplied in supplies:
        for target in targets:
            returns=((supplied,) if existing.return_path is not None and existing.return_path.end==target
                     else _connections(supplied,target,link.return_pad,link.return_net,
                                       options,maximum_launch_candidates,maximum_site_candidates,work=work))
            for candidate in returns:
                attempted+=1
                fresh=RoutingClearanceIndex(candidate)
                measured=verify_crosslayer_decoupling(candidate,link,return_target=target,
                                                      maximum_path_states=maximum_path_states,index=fresh)
                if measured.witness is not None and not any(validate_crosslayer_witness(candidate,w,index=fresh) for w in protected):
                    created_tracks=candidate.tracks[len(board.tracks):]
                    created_vias=candidate.vias[len(board.vias):]
                    covered_tracks={id(step.edge) for path in (measured.supply_path,measured.return_path)
                                    for step in path.steps if isinstance(step.edge,TrackSegment)}
                    covered_vias={id(step.edge) for path in (measured.supply_path,measured.return_path)
                                  for step in path.steps if isinstance(step.edge,Via)}
                    coverage=(all(id(t) in covered_tracks for t in created_tracks)
                              and all(id(v) in covered_vias for v in created_vias))
                    uncovered |= not coverage
                    exact=(coverage
                           and all(_track_ok(candidate,fresh,t,t.net) for t in created_tracks)
                           and all(_normal_via(candidate,fresh,v) for v in created_vias))
                    if exact:
                        errors={f.fingerprint for f in run_physical_drc(candidate).findings
                                if f.severity is DrcSeverity.ERROR and f.code not in {"DRC-OPEN-NET","DRC-ROUTE-INCOMPLETE"}}
                        if not errors-baseline:
                            return replace(measured,created_tracks=created_tracks,created_vias=created_vias)
                if attempted>=maximum_proposals:
                    return DecouplingPathResult(board,reason=("unwitnessed_generated_bypass_copper" if uncovered else "bounded_complete_proposals_exhausted"),
                                                supply_path=existing.supply_path,return_path=existing.return_path)
    return DecouplingPathResult(board,reason=("unwitnessed_generated_bypass_copper" if uncovered else "bounded_supply_return_candidates_exhausted"),
                                supply_path=existing.supply_path,return_path=existing.return_path)
