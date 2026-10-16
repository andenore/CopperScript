"""Source terminal preparation for bounded exact differential searches.

Coarse access is a routing heuristic, not a certificate. These physical terminal
records have no global grid node and never make a failed global guide connected.
Short same-package branches are returned as disposable candidate copper; only
the complete pair's ordinary atomic geometry/connectivity gates may adopt them.
"""
from dataclasses import dataclass, field, fields, replace
from itertools import permutations
from time import perf_counter
from .physical import BoardSide, CopperLayer, PadKind, PadReference, Point, TrackSegment, Via
from .placement import transformed_pad_position
from .routing import GlobalNetRoute
from .routing_clearance import RoutingBlocker, RoutingClearanceIndex
from .pin_escape import checked_access_path
from .pin_access import local_access_path

def terminal_copper_layers(board, reference):
    """Actual unique contact layers; a guide's tentative via grants no access.

    Plated lands contact every physical copper layer. Ambiguous multi-land
    numbers, missing references, apertures and unplated holes fail closed.
    Track policy is intersected separately by the caller.
    """
    pose = next((p for p in board.placements if p.reference == reference.component), None)
    if pose is None:
        return frozenset()
    pads = [p for p in board.footprints[pose.footprint].pads if p.number == reference.pad]
    if len(pads) != 1:
        return frozenset()
    pad = pads[0]
    if pad.kind is PadKind.THROUGH_HOLE:
        return frozenset(board.stackup.copper_layers)
    if pad.kind is PadKind.SMD:
        return frozenset((CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK,))
    return frozenset()


@dataclass(frozen=True, slots=True)
class PairTerminalPreparation:
    """Immutable local-work evidence; elapsed time is excluded from equality.

    Maze counts cover only the coupled search. Reporting never certifies a
    connected guide, creates ownership, or contributes to routing fingerprints.
    """
    status: str = "not_started"
    reason: str = ""
    elapsed_seconds: float = field(default=0.0, compare=False)
    paired_topology_supported: bool = False
    port_pairs: int = 0
    searches: int = 0
    expanded_states: int = 0
    candidates: int = 0
    tuning_rejections: int = 0
    profile_rejections: int = 0
    connectivity_rejections: int = 0
    geometry_rejections: int = 0
    track_count: int = 0
    via_count: int = 0
    blocking_nets: tuple[str, ...] = ()
    blocking_objects: tuple[RoutingBlocker, ...] = ()
    rejection_examples: tuple[str, ...] = ()

@dataclass(slots=True)
class PairTerminalPreparationStats:
    """Local proposal telemetry; never a connected guide or copper owner.

    ``expanded_states`` counts the coupled maze only. Port formation, local
    surface access and matching also cost work, measured by ``elapsed_seconds``.
    """
    status: str = "not_started"
    reason: str = ""
    elapsed_seconds: float = 0.0
    paired_topology_supported: bool = False
    port_pairs: int = 0
    searches: int = 0
    expanded_states: int = 0
    candidates: int = 0
    tuning_rejections: int = 0
    profile_rejections: int = 0
    connectivity_rejections: int = 0
    geometry_rejections: int = 0
    track_count: int = 0
    via_count: int = 0
    blocking_nets: tuple[str, ...] = ()
    blocking_objects: tuple[RoutingBlocker, ...] = ()
    rejection_examples: tuple[str, ...] = ()

    def snapshot(self) -> PairTerminalPreparation:
        return PairTerminalPreparation(**{item.name: getattr(self, item.name)
                                          for item in fields(PairTerminalPreparation)})

    def record_blockers(self, index, tracks, vias):
        found = {item.identity: item for item in self.blocking_objects}
        for track in tracks:
            for item in index.track_blocker_records(track):
                found.setdefault(item.identity, item)
                if len(found) >= 32:
                    break
            if len(found) >= 32:
                break
        if len(found) < 32:
            for via in vias:
                for item in index.via_blocker_records(via):
                    found.setdefault(item.identity, item)
                    if len(found) >= 32:
                        break
                if len(found) >= 32:
                    break
        self.blocking_objects = tuple(found.values())

    def reject(self, reason, nets=()):
        self.rejection_examples = tuple(dict.fromkeys((*self.rejection_examples, reason)))[:3]
        self.blocking_nets = tuple(sorted(set(self.blocking_nets) | set(nets)))[:32]


@dataclass(frozen=True, slots=True)
class PairTerminal:
    pad: PadReference
    pad_position: Point
    access_position: Point
    layer: CopperLayer
    via: Via | None = None
    region_only: bool = True
    tracks: tuple[TrackSegment, ...] = ()

def terminal_partners(first, second):
    """Pair two ends geometrically while preserving every original pad identity.

    Shared packages take precedence, then closest physical endpoint pairing.
    This chooses a spine orientation, never changes the electrical net partition.
    """
    if len(first.accesses)!=2 or len(second.accesses)!=2:
        return None
    if any(len({a.pad for a in g.accesses})!=2 for g in (first, second)):
        return None
    choices=[]
    for order in permutations(second.accesses):
        score=(sum(a.pad.component!=b.pad.component for a,b in zip(first.accesses,order)),
            sum((a.pad_position.x_nm-b.pad_position.x_nm)**2+(a.pad_position.y_nm-b.pad_position.y_nm)**2 for a,b in zip(first.accesses,order)),
            tuple(str(b.pad) for b in order))
        choices.append((score,order))
    _,order=min(choices,key=lambda item:item[0])
    result = {a.pad:b for a,b in zip(first.accesses,order)}
    # Component aliases remain available to package-level nested-exit plans.
    # Repeated contacts use their exact pad identity instead of overwriting an end.
    for a,b in zip(first.accesses,order):
        if sum(x.pad.component == a.pad.component for x in first.accesses) == 1:
            result[a.pad.component] = b
    return result

def source_terminal_guide(board, name, existing):
    net=next((n for n in board.nets if n.name==name),None)
    if net is None: return existing
    poses={p.reference:p for p in board.placements}
    accesses=[]
    for ref in net.pads:
        pose=poses.get(ref.component)
        if pose is None: return existing
        lands=[p for p in board.footprints[pose.footprint].pads if p.number==ref.pad]
        if len(lands)!=1: return existing  # same-number multi-land inventory is not guessed
        pos=transformed_pad_position(board,pose,ref.pad)
        layer=CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
        accesses.append(PairTerminal(ref,pos,pos,layer))
    if existing is not None and existing.connected and {a.pad for a in existing.accesses}==set(net.pads):
        return existing
    return GlobalNetRoute(name,False,tuple(accesses),(),(),0,
        ('source terminals for exact search; no connected coarse guide',))

def _surface_pair_terminals(board, first, second, routes):
    from .hard_macros import macro_owned_pads
    owned = macro_owned_pads(board)
    if any(p in owned for n in board.nets if n.name in (first.net, second.net) for p in n.pads):
        return routes, (), ()  # keep prescribed macro ports and their existing access contract
    guides=[source_terminal_guide(board,r.net,routes.get(r.net)) for r in (first,second)]
    if any(g is None for g in guides): return None,(),()
    all_branches=[]
    normalized=[]
    from .pair_search import _PairAccessIndex
    index=_PairAccessIndex(board)
    for rule,guide in zip((first,second),guides):
        groups={}
        for a in guide.accesses: groups.setdefault(a.pad.component,[]).append(a)
        if len(groups)!=2 or sum(len(v)-1 for v in groups.values())>8:
            return None,(),()  # fixed bound on disposable local branches
        anchors=[]
        branches=[]
        for component,terminals in sorted(groups.items()):
            anchor=min(terminals,key=lambda a:str(a.pad))
            anchors.append(anchor)
            for other in sorted(terminals,key=lambda a:str(a.pad)):
                if other is anchor: continue
                # A local contact tie is allowed only inside a source-declared
                # breakout radius. No remote branch is inferred or silently omitted.
                limit=rule.breakout_length_nm
                if (limit is None or anchor.layer is not other.layer or
                    (anchor.pad_position.x_nm-other.pad_position.x_nm)**2+
                    (anchor.pad_position.y_nm-other.pad_position.y_nm)**2>limit**2):
                    return None,(),()
                width=rule.width_nm or board.rules.default_track_width_nm
                path=checked_access_path(board,index,rule.net,anchor.pad_position,other.pad_position,width,anchor.layer)
                if path is None:
                    path=local_access_path(board,index,rule.net,anchor.pad_position,other.pad_position,anchor.layer,width,
                        step_nm=125000,detour_nm=limit,maximum_states=600)
                if path is None: return None,(),()
                for track in path: index.add_track(track,locked=True)
                branches.extend(path)
        normalized.append(replace(guide,accesses=tuple(anchors),connected=guide.connected and len(guide.accesses)==2))
        all_branches.append(tuple(branches))
    if terminal_partners(*normalized) is None: return None,(),()
    return {**routes,first.net:normalized[0],second.net:normalized[1]},*all_branches


def prepare_pair_terminals(board, first, second, routes, *, report=None):
    """Return disposable branches; optional telemetry never changes acceptance."""
    report = report if report is not None else PairTerminalPreparationStats()
    started = perf_counter()
    try:
        prepared, a, b = _surface_pair_terminals(board, first, second, routes)
        if prepared is not None:
            report.status = "surface_contacts" if a or b else "source_terminals"
            report.track_count = len(a) + len(b)
            return prepared, a, b, (), ()
        proposed = _paired_local_contacts(board, first, second, report=report)
        if proposed is None:
            report.status = "blocked" if report.paired_topology_supported else "unsupported"
            report.reason = ("bounded local contact crossover has no accepted candidate"
                             if report.paired_topology_supported else
                             "local contact crossover topology or source contract unsupported")
            return routes, (), (), (), ()
        normalized, a, b, pairs, returns = proposed
        report.status = "matched_contacts"
        report.track_count = len(a) + len(b)
        report.via_count = 2 * len(pairs) + len(returns)
        return {**routes, **normalized}, a, b, pairs, returns
    finally:
        report.elapsed_seconds = perf_counter() - started


def _paired_local_contacts(board, first, second, *, report=None):
    report = report if report is not None else PairTerminalPreparationStats()
    # One local duplicated-contact group per member, two contacts each, with
    # one remaining main endpoint. Broader trees stay explicitly unsupported.
    from .hard_macros import macro_owned_pads
    owned = macro_owned_pads(board)
    guides = [source_terminal_guide(board, r.net, None) for r in (first, second)]
    if any(g is None or any(a.pad in owned for a in g.accesses) for g in guides):
        return None
    groups = []
    for rule, guide in zip((first, second), guides):
        grouped = {}
        for a in guide.accesses:
            grouped.setdefault(a.pad.component, []).append(a)
        duplicate = [items for items in grouped.values() if len(items) == 2]
        if len(grouped) != 2 or len(guide.accesses) != 3 or len(duplicate) != 1:
            return None
        contacts = tuple(sorted(duplicate[0], key=lambda a:str(a.pad)))
        if (rule.breakout_length_nm is None or contacts[0].layer is not contacts[1].layer
            or (contacts[0].pad_position.x_nm-contacts[1].pad_position.x_nm)**2
               + (contacts[0].pad_position.y_nm-contacts[1].pad_position.y_nm)**2
               > rule.breakout_length_nm**2):
            return None
        groups.append((grouped, contacts))
    if groups[0][1][0].pad.component != groups[1][1][0].pad.component:
        return None
    report.paired_topology_supported = True
    local = [replace(g, connected=False, accesses=items[1], segments=(), vias=())
             for g, items in zip(guides, groups)]
    from .pair_search import paired_candidates, PairSearchStats
    from .pair_vias import paired_via_candidates
    from .drc import explicit_copper_connectivity, run_physical_drc, PhysicalDrcPolicy
    from .physical import PhysicalNet
    stats = PairSearchStats()
    for search in (paired_via_candidates, paired_candidates):
        for raw_candidate in search(board, first, second, *local, pitch_nm=250000,
                maximum_searches=8, maximum_states=4000, maximum_total_states=16000, stats=stats):
            report.port_pairs, report.searches = stats.port_pairs, stats.searches
            report.expanded_states, report.candidates = stats.expanded_states, stats.candidates
            for candidate in _matched_local_candidates(board, first, second, raw_candidate, report=report):
                # Tune the complete local pair against the unchanged source skew
                # limit before checking its actual geometry. No provisional skew
                # exemption: rejected tuning retains no branch copper.
                from .critical import _route_pair
                local_routes = {r.net:g for r,g in zip((first,second),local)}
                metrics, branch_tracks, vias = _route_pair(board, first, second, local_routes,
                    exact_tracks=(candidate.first,candidate.second),
                    exact_via_pairs=candidate.via_pairs, exact_return_vias=candidate.return_vias)
                if not metrics.connected:
                    report.profile_rejections += 1
                    for reason in metrics.diagnostics:
                        report.reject(reason)
                    continue
                branch_a = tuple(t for t in branch_tracks if t.net == first.net)
                branch_b = tuple(t for t in branch_tracks if t.net == second.net)
                trial = replace(board, tracks=(*board.tracks, *branch_tracks),
                                vias=(*board.vias, *vias))
                graph = explicit_copper_connectivity(trial)
                if not all(graph.net_connected(PhysicalNet(r.net, tuple(a.pad for a in g.accesses)))
                           for r,g in zip((first,second),local)):
                    report.connectivity_rejections += 1
                    report.reject("local contact copper does not connect every source contact")
                    continue
                findings = tuple(f for f in run_physical_drc(trial,
                    policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
                    if f.severity.value == 'error' and f.code != 'DRC-OPEN-NET')
                if findings:
                    report.geometry_rejections += 1
                    report.record_blockers(RoutingClearanceIndex(board), branch_tracks, vias)
                    for finding in findings:
                        report.reject(f"{finding.code}: {finding.message}",
                                      set(finding.nets) - {first.net, second.net})
                    continue
                normalized = [replace(g, connected=False, accesses=tuple(
                    min(items,key=lambda a:str(a.pad)) for _,items in sorted(grouped.items())))
                    for g,(grouped,_) in zip(guides,groups)]
                return ({first.net:normalized[0], second.net:normalized[1]},
                        branch_a, branch_b, candidate.via_pairs, candidate.return_vias)
    report.port_pairs, report.searches = stats.port_pairs, stats.searches
    report.expanded_states, report.candidates = stats.expanded_states, stats.candidates
    return None


def _matched_local_candidates(board, first, second, candidate, *, report=None):
    """At most two exact-clearance tuning proposals; matching limits unchanged."""
    from .critical import _track_length
    from .critical_tuning import UnitTuner
    limits = [r.max_skew_nm for r in (first,second) if r.max_skew_nm is not None]
    tracks = (candidate.first,candidate.second)
    lengths = tuple(_track_length(items) for items in tracks)
    delta = abs(lengths[0]-lengths[1])
    if not limits or delta <= min(limits):
        yield candidate
        return
    shorter = 0 if lengths[0] < lengths[1] else 1
    rule = (first,second)[shorter]
    if rule.tuning_amplitude_limit_nm is None:
        if report is not None:
            report.tuning_rejections += 1
            report.reject("local contact skew exceeds the source limit without a tuning amplitude contract")
        return
    vias = (*[v for pair in candidate.via_pairs for v in pair],*candidate.return_vias)
    obstacles = replace(board, tracks=(*board.tracks,*tracks[1-shorter]),
                        vias=(*board.vias,*vias))
    index = RoutingClearanceIndex(obstacles)
    tuner = UnitTuner(board,index,tracks[shorter],(rule.net,),0,
        rule.tuning_amplitude_limit_nm,max(board.rules.minimum_clearance_nm,rule.clearance_nm or 0),
        style=rule.tuning_style,leg_gap_nm=rule.tuning_spacing_nm,
        vias=tuple(v for v in vias if v.net == rule.net))
    for amount in dict.fromkeys((delta,delta-min(limits)+min(20000,min(limits)))):
        amount += amount % 2
        plan = tuner.plan(amount)
        if plan.tracks is None:
            if report is not None:
                report.tuning_rejections += 1
                report.reject(plan.reason)
            continue
        proposed = index.breakout.split_tracks(plan.tracks,rule.width_nm or board.rules.default_track_width_nm)
        yield replace(candidate,**{('first','second')[shorter]:proposed})
