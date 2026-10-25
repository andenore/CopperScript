"""Bounded distribution trees, physical area proposals and joint allocation.

All proposals retain the complete board. A plan owns only appended occurrences;
it does not move contacts, remove source copper, replace zones or defer nets.
Native fill and electrical extraction are separate, snapshot-bound observations.
"""
from dataclasses import dataclass, field, replace
from hashlib import sha256
from math import ceil, cos, pi, isqrt

from .physical import (CopperLayer, CopperZone, IslandPolicy, PadReference,
                       Point, PolygonRing, PolygonWithHoles, RouteKind, ZoneConnection)
from .drc import _explicit_copper_contacts, explicit_copper_connectivity, physical_board_digest, run_physical_drc
from .geometry import RoundedConvexShape
from .power_areas import (PowerAreaCandidate, allocate_power_areas, reduce_power_conflict,
                         power_repair_contexts)
from .routing_clearance import RoutingClearanceIndex
from .surface_path import surface_path_between
from .progress import emit


@dataclass(frozen=True)
class ExternalPowerBoundary:
    net: str
    terminal: PadReference
    role: str
    provenance: str

    def __post_init__(self):
        if not self.net or self.role not in {'source', 'return'} or not self.provenance.strip():
            raise ValueError('external power boundary requires source/return role and provenance')


@dataclass(frozen=True)
class PowerAreaOptions:
    nets: tuple[str, ...] = ()  # Empty means all declared power domains.
    allowed_layers: tuple[CopperLayer, ...] = ()
    maximum_domains: int = 8
    candidates_per_domain: int = 4
    maximum_layer_alternatives: int = 2
    maximum_terminals: int = 64
    maximum_anchors: int = 8
    maximum_new_contacts: int = 8
    search_states: int = 4000
    allocation_states: int = 4096
    maximum_conflict_trials: int = 0
    maximum_branch_layers: int = 0
    branch_search_states: int = 4000
    # Reuse generated assignments after fresh native rejection. Exact dangling
    # transitions may also receive additive branches within the original repair
    # and allocator allowances; every changed assignment needs fresh native fill.
    maximum_native_trials: int = 1
    # Opt-in private contact negotiation before area growth. Zero retains the
    # established sequential preparation; one extra native preservation check
    # is required if the bounded joint controller proposes a replacement.
    joint_contact_trials: int = 0
    # Opt-in failed-area feedback. Each slot permits one exact dependency
    # regeneration plus one fresh area pool; allocator states and regrowth
    # trials remain cumulative across those changed contact scenes.
    area_contact_trials: int = 0
    reserve_ordinary_before_contacts: bool = False
    package_patterns_before_contacts: bool = False
    package_pattern_offset_nm: int = 0
    package_pattern_offsets: tuple = ()
    # Runtime reservations derived from verified ordinary launches. They enter
    # exact clearance indexes, including drill/copper checks, but are never
    # adopted as power copper or authored source facts.
    reserved_tracks: tuple = ()
    reserved_vias: tuple = ()
    step_nm: int = 250000
    coarse_step_nm: int = 500000
    detour_nm: int = 2000000
    # Protected corridors are authored geometry, not guessed USB straight lines.
    protected_corridors: tuple = ()
    electrical_models: tuple = ()
    external_boundaries: tuple[ExternalPowerBoundary, ...] = ()
    partitionable_zone_ids: tuple[str, ...] = ()
    # Opt-in source domains whose bypass returns use freshly verified planes.
    # Empty retains the established explicit local-return routing policy.
    plane_return_nets: tuple[str, ...] = ()
    # Opt-in joint contact arrangements per distribution layer. These share
    # the existing area-candidate cap; one preserves the legacy stitcher.
    maximum_contact_alternatives: int = 1
    contact_candidates_per_terminal: int = 8
    contact_site_queries: int = 2048
    contact_assignment_states: int = 256
    contact_pair_queries: int = 65536

    def __post_init__(self):
        if type(self.reserve_ordinary_before_contacts) is not bool:
            raise ValueError('early ordinary reservation policy must be boolean')
        if type(self.package_patterns_before_contacts) is not bool:
            raise ValueError('package pattern preparation policy must be boolean')
        if type(self.package_pattern_offset_nm) is not int or self.package_pattern_offset_nm<0:
            raise ValueError('package pattern offset must be nonnegative integer nanometres')
        offsets=tuple(tuple(v) for v in self.package_pattern_offsets)
        if any(len(v)!=2 or not isinstance(v[0],str) or not v[0] or type(v[1]) is not int or v[1]<=0 for v in offsets):
            raise ValueError('package offsets require component names and positive integer nanometres')
        if len(dict(offsets))!=len(offsets):raise ValueError('duplicate package offset component')
        object.__setattr__(self,'package_pattern_offsets',offsets)
        if type(self.joint_contact_trials) is not int or self.joint_contact_trials < 0:
            raise ValueError('joint contact trial bound must be a nonnegative integer')
        if type(self.area_contact_trials) is not int or self.area_contact_trials < 0:
            raise ValueError('area contact trial bound must be a nonnegative integer')
        from .physical import TrackSegment, Via
        for name, kind in (('reserved_tracks', TrackSegment), ('reserved_vias', Via)):
            object.__setattr__(self, name, tuple(getattr(self, name)))
            if any(not isinstance(v, kind) for v in getattr(self, name)):
                raise ValueError('reserved copper requires actual verified track/via objects')
        if type(self.maximum_conflict_trials) is not int or self.maximum_conflict_trials < 0:
            raise ValueError('power conflict trial bound must be a nonnegative integer')
        if type(self.maximum_branch_layers) is not int or self.maximum_branch_layers < 0:
            raise ValueError('power branch layer bound must be a nonnegative integer')
        for name in ('maximum_domains', 'candidates_per_domain', 'maximum_layer_alternatives',
                     'maximum_terminals', 'maximum_anchors', 'maximum_new_contacts', 'search_states',
                     'allocation_states', 'branch_search_states', 'maximum_native_trials', 'step_nm', 'coarse_step_nm', 'detour_nm',
                     'maximum_contact_alternatives', 'contact_candidates_per_terminal', 'contact_site_queries',
                     'contact_assignment_states', 'contact_pair_queries'):
            if type(getattr(self, name)) is not int or getattr(self, name) <= 0:
                raise ValueError('power area work bounds must be positive integers')
        for name in ('nets', 'allowed_layers', 'protected_corridors', 'electrical_models', 'external_boundaries', 'partitionable_zone_ids', 'plane_return_nets'):
            object.__setattr__(self, name, tuple(getattr(self, name)))
        if len(set(self.nets)) != len(self.nets) or len(set(self.allowed_layers)) != len(self.allowed_layers):
            raise ValueError('duplicate power area net or layer')
        if len({m.net for m in self.electrical_models}) != len(self.electrical_models):
            raise ValueError('duplicate electrical domain model')
        if (any(not isinstance(n, str) or not n for n in self.plane_return_nets)
                or len(set(self.plane_return_nets)) != len(self.plane_return_nets)
                or self.nets and not set(self.plane_return_nets) <= set(self.nets)):
            raise ValueError('plane return domains must be unique selected power nets')


@dataclass(frozen=True)
class DomainObligation:
    net: str
    sources: tuple[PadReference, ...]
    feeds: tuple
    terminals: tuple[PadReference, ...]
    local_links: tuple
    status: str
    external_boundaries: tuple = ()
    source_ports: tuple[str, ...] = ()


@dataclass(frozen=True)
class PhysicalAreaCandidate:
    allocation: PowerAreaCandidate
    tracks: tuple
    zones: tuple
    layer: CopperLayer
    unresolved: tuple[str, ...] = ()
    vias: tuple = ()
    tree_width_nm: int | None = None
    tree_reverse: bool = False
    tree_detour: int = 0
    contact_variant: int = 0
    contact_identity: str = ''


@dataclass(frozen=True)
class PowerSearchFailure:
    net: str
    layer: CopperLayer
    start: Point
    end: Point
    width_nm: int
    expanded_states: int
    sampled_blockers: tuple[str, ...]
    held_candidate_ids: tuple[str, ...] = ()


@dataclass(frozen=True)
class PowerContactObservation:
    net: str
    layer: CopperLayer
    terminals: tuple[PadReference, ...]
    status: str
    accessible_layers: tuple[CopperLayer, ...] = ()
    bridge_layer: CopperLayer | None = None
    expanded_states: int | None = None
    sampled_blockers: tuple[str, ...] = ()

    def report(self):
        return dict(net=self.net, layer=self.layer.value,
            terminals=[str(p) for p in self.terminals], status=self.status,
            accessible_layers=[l.value for l in self.accessible_layers],
            bridge_layer=self.bridge_layer.value if self.bridge_layer else None,
            expanded_states=self.expanded_states, sampled_blockers=self.sampled_blockers)


@dataclass(frozen=True)
class PowerAreaPlan:
    snapshot: str
    contract_digest: str
    status: str
    obligations: tuple[DomainObligation, ...]
    candidates: tuple[PhysicalAreaCandidate, ...]
    selected_ids: tuple[str, ...]
    diagnostics: tuple[tuple[str, str], ...]
    allocation_states: int = 0
    exhausted: bool = False
    work_limits: tuple[tuple[str, int], ...] = ()
    partitioned_zone_ids: tuple[str, ...] = ()
    source_board: object = field(default=None, repr=False, compare=False)
    search_failures: tuple[PowerSearchFailure, ...] = ()
    conflict_trials: tuple = ()
    conflict_contexts: tuple = ()
    contact_observations: tuple[PowerContactObservation, ...] = ()
    contact_trials: tuple = ()
    conflict_core_observations: tuple = ()
    native_transition_trials: tuple = ()

    @property
    def selected(self):
        return tuple(c for c in self.candidates if c.allocation.id in self.selected_ids)

    @property
    def score(self):
        selected = {c.allocation.domain for c in self.selected}
        return (sum(len(o.terminals) for o in self.obligations if o.net not in selected),
                sum(len(c.unresolved) for c in self.selected),
                sum(c.allocation.area_nm2 for c in self.selected))

    def report(self):
        return dict(snapshot=self.snapshot, contract_digest=self.contract_digest, status=self.status,
            selected_ids=self.selected_ids, allocation_states=self.allocation_states, exhausted=self.exhausted,
            work_limits=dict(self.work_limits),
            partitioned_zone_ids=self.partitioned_zone_ids,
            conflict_trials=self.conflict_trials,
            conflict_contexts=self.conflict_contexts,
            conflict_core_observations=self.conflict_core_observations,
            native_transition_trials=self.native_transition_trials,
            contact_trials=self.contact_trials,
            contact_observations=[o.report() for o in self.contact_observations],
            search_failures=[dict(net=f.net, layer=f.layer.value, start_nm=(f.start.x_nm, f.start.y_nm),
                end_nm=(f.end.x_nm, f.end.y_nm), width_nm=f.width_nm, expanded_states=f.expanded_states,
                sampled_blockers=f.sampled_blockers, held_candidate_ids=f.held_candidate_ids) for f in self.search_failures],
            diagnostics=self.diagnostics, domains=[dict(net=o.net, status=o.status,
                source_ports=o.source_ports,
                sources=[str(p) for p in o.sources], feeds=[dict(terminal=str(f.terminal),
                upstream_net=f.upstream_net, upstream_terminal=str(f.upstream_terminal),
                provenance=f.provenance) for f in o.feeds],
                external_boundaries=[dict(terminal=str(b.terminal), role=b.role, provenance=b.provenance) for b in o.external_boundaries],
                terminals=[str(p) for p in o.terminals], local_associations=len(o.local_links)) for o in self.obligations],
            candidates=[dict(id=c.allocation.id, net=c.allocation.domain, layer=c.layer.value,
                layers=sorted({t.layer.value for t in c.tracks} | {l.value for z in c.zones for l in z.layers}),
                area_nm2=c.allocation.area_nm2, tracks=len(c.tracks), vias=len(c.vias), zones=len(c.zones),
                tree_width_nm=c.tree_width_nm, tree_reverse=c.tree_reverse, tree_detour=c.tree_detour,
                contact_variant=c.contact_variant, contact_identity=c.contact_identity,
                unresolved=c.unresolved,
                conflicts_with=sorted({other for token in c.allocation.resources if token.startswith('conflict:')
                    for other in token.split(':')[1:] if other != c.allocation.id})) for c in self.candidates])


def power_obligations(board, options):
    domains = {d.net: d for d in board.power_domains}
    nets = {n.name: n for n in board.nets}
    requested = options.nets or tuple(sorted(domains))
    for boundary in options.external_boundaries:
        if boundary.net not in nets or boundary.terminal not in nets[boundary.net].pads:
            raise ValueError('external power boundary must identify a declared net terminal')
    if any(n not in nets for n in requested):
        raise ValueError('power area selection contains unknown net')
    obligations = []
    for name in sorted(requested):
        domain = domains.get(name)
        external = tuple(b for b in options.external_boundaries if b.net == name)
        sources = tuple(sorted(set(domain.sources if domain else ()) | {b.terminal for b in external}))
        feeds = tuple(f for f in board.power_feeds if f.net == name)
        obligations.append(DomainObligation(name, sources, feeds, tuple(sorted(nets[name].pads)),
            tuple(link for link in board.decoupling_links if link.net == name),
            'boundary_known' if sources or feeds else 'source_boundary_unresolved', external,
            domain.source_ports if domain else ()))
    return tuple(obligations)


def _shape_geometry(shape):
    from shapely.geometry import MultiPoint
    geometry = MultiPoint([(p.x_nm, p.y_nm) for p in shape.spine]).convex_hull
    return geometry.buffer(shape.radius_nm, quad_segs=8) if shape.radius_nm else geometry


def _conflict_geometry(shape):
    """Circumscribed envelope: conflict certificates never omit round copper."""
    from shapely.geometry import MultiPoint
    geometry = MultiPoint([(p.x_nm, p.y_nm) for p in shape.spine]).convex_hull
    return geometry.buffer(shape.radius_nm/cos(pi/32)+1, quad_segs=8) if shape.radius_nm else geometry


def _zone_geometry(zone):
    from shapely.geometry import Polygon
    geometry = Polygon([(p.x_nm, p.y_nm) for p in zone.outline.outer.vertices],
                       [[(p.x_nm, p.y_nm) for p in h.vertices] for h in zone.outline.holes])
    if not geometry.is_valid:
        raise ValueError(f'invalid power planning polygon: {zone.id}')
    return geometry


class _AreaClearance:
    """Existing exact routing predicates plus protected distribution geometry."""
    def __init__(self, board, options):
        self.base = RoutingClearanceIndex(board)
        for track in options.reserved_tracks:
            if track not in board.tracks:
                self.base.add_track(track, locked=True)
        for via in options.reserved_vias:
            if via not in board.vias:
                self.base.add_via(via, locked=True)
        self.board = board
        permitted = set(options.partitionable_zone_ids)
        zones = {z.id: z for z in board.zones}
        power = {d.net for d in board.power_domains}
        returns = {l.return_net for l in board.decoupling_links} | {r.return_via_net for r in board.net_routing_rules}
        returns.update(b.net for b in options.external_boundaries if b.role == 'return')
        macro_ids = {z.id for m in board.hard_macros for z in m.zones}
        if any(identity not in zones or zones[identity].reserve_routing or identity in macro_ids
               or zones[identity].net not in power - returns
               or any(r.shared_reference_layer in zones[identity].layers for r in board.net_routing_rules)
               for identity in permitted):
            raise ValueError('partition permission requires a mutable non-reference power zone')
        self.protected = [(z.net, z.layers, _zone_geometry(z), z.clearance_nm or 0)
                          for z in board.zones if z.id not in permitted]
        self.protected.extend((None, k.layers, _zone_geometry(k), 0)
            for k in (*board.copper_keepouts, *options.protected_corridors) if k.block_zones)
        self.clearances = {r.net: r.clearance_nm or 0 for r in board.net_routing_rules}
        self._area_regions = {}

    def __getattr__(self, name):
        return getattr(self.base, name)

    def can_track(self, net, start, end, width, layer):
        if not self.base.can_track(net, start, end, width, layer):
            return False
        key = net, layer
        if key not in self._area_regions:
            self._area_regions[key] = tuple((area, area.bounds, max(self.board.rules.minimum_clearance_nm,
                clearance, self.clearances.get(net, 0), self.clearances.get(other, 0)))
                for other, layers, area, clearance in self.protected if other != net and layer in layers)
        # Most inner-layer steps have no nearby protected region. Skip polygon
        # construction only after a conservative envelope/clearance broad phase;
        # nearby regions still use exactly the original distance predicate.
        radius = ceil(((width+1)//2)/cos(pi/32)+1)
        x0, x1 = min(start.x_nm, end.x_nm)-radius, max(start.x_nm, end.x_nm)+radius
        y0, y1 = min(start.y_nm, end.y_nm)-radius, max(start.y_nm, end.y_nm)+radius
        geometry = None
        for area, (left, bottom, right, top), margin in self._area_regions[key]:
            if x1+margin < left or right+margin < x0 or y1+margin < bottom or top+margin < y0:
                continue
            if geometry is None:
                geometry = _conflict_geometry(RoundedConvexShape((start, end), (width+1)//2))
            if geometry.distance(area) < margin:
                return False
        return True

    def area_clear(self, net, geometry, layer):
        # Circumscribed buffer protects the gap, including curved boundaries.
        for other, layers, area, clearance in self.protected:
            if other == net or layer not in layers:
                continue
            margin = max(self.board.rules.minimum_clearance_nm, clearance,
                         self.clearances.get(net, 0), self.clearances.get(other, 0))
            if geometry.distance(area) < margin:
                return False
        return True

    def can_route(self, net, start, end, width, layer):
        return all(self.can_track(net, p.start, p.end, p.width_nm, layer)
                   for p in self.route_pieces(net, start, end, width, layer))


def _sample_anchors(points, maximum, preferred=()):
    """Keep nearby launches and spatial coverage, rather than one dense edge."""
    remaining = set(points)
    selected = []
    for point in preferred:
        if point in remaining and len(selected) < maximum:
            selected.append(point)
            remaining.remove(point)
    if remaining and not selected:
        point = min(remaining, key=lambda p: (p.x_nm, p.y_nm))
        selected.append(point)
        remaining.remove(point)
    while remaining and len(selected) < maximum:
        point = min(remaining, key=lambda p: (
            -min(abs(p.x_nm-q.x_nm)+abs(p.y_nm-q.y_nm) for q in selected), p.x_nm, p.y_nm))
        selected.append(point)
        remaining.remove(point)
    return tuple(selected)


def _anchors(board, net, graph, contacts, pads, layer, maximum):
    # Only contacts on a verified explicit copper component can be launches.
    groups = {}
    for pad in pads:
        nodes = graph.pad_nodes.get(pad, ())
        if len(nodes) != 1:
            return None
        root = graph.roots[nodes[0]]
        groups.setdefault(root, [])
    for contact in contacts:
        if contact.net != net or layer not in contact.layers:
            continue
        root = graph.roots.get(contact.identity)
        if root in groups:
            # Pad/track/via centres are proven contacts; polygon vertices alone
            # are boundary touches, not reliable electrical launch sites.
            if contact.identity.startswith(('pad:', 'via:', 'track:')):
                groups[root].extend(contact.shape.spine)
    from .hard_macros import macro_routing_ports, macro_port_layers
    ports = {}
    for pad, port in macro_routing_ports(board, net).items():
        nodes = graph.pad_nodes.get(pad, ())
        if len(nodes) == 1 and layer in macro_port_layers(board, port):
            root = graph.roots[nodes[0]]
            if root in groups:
                groups[root].append(port.position)
                ports.setdefault(root, set()).add(port.position)
    return {root: _sample_anchors(points, maximum,
                sorted(ports.get(root, ()), key=lambda p: (p.x_nm, p.y_nm)))
            for root, points in groups.items()}


def _attachment_pairs(rows, maximum):
    # Rows are already ordered by the requested tree strategy. Give each
    # source-connected launch a turn before trying another edge of the same
    # dense contact. A different net label or an unfilled outline is no anchor.
    selected, parents = [], set()
    for row in rows:
        if row[-2] not in parents:
            selected.append(row)
            parents.add(row[-2])
            if len(selected) == maximum:
                return selected
    return (selected + [row for row in rows if row not in selected])[:maximum]


def _attachment_branch(board, net, layer, pairs, width, clearance, options, budget, detour, failures, *, work=None):
    for *_, p, q in pairs:
        branch = _tree_branch(board, net, layer, p, q, width, clearance, options, 0, detour, None)
        if branch is not None:
            if work is not None:
                work.append(0)
            return branch
    # Alternate launches share the existing allowance. Early blocked exits
    # return unused states; trying another parent never multiplies the cap.
    remaining = budget
    for index, (*_, p, q) in enumerate(pairs):
        if not remaining:
            break
        allowance = remaining if index == len(pairs)-1 else max(1, remaining//2)
        expanded = []
        branch = _tree_branch(board, net, layer, p, q, width, clearance, options,
                              allowance, detour, failures, work=expanded)
        remaining -= sum(expanded)
        if branch is not None:
            if work is not None:
                work.append(budget-remaining)
            return branch
    if work is not None:
        work.append(budget-remaining)
    return None


def _tree(board, obligation, layer, width, clearance, graph, contacts, options, reverse=False, detour=0, failures=None):
    groups = _anchors(board, obligation.net, graph, contacts, obligation.terminals, layer, options.maximum_anchors)
    if groups is None or any(not points for points in groups.values()):
        return None, 'contact_layer_unreachable'
    boundaries = (*obligation.sources, *(f.terminal for f in obligation.feeds))
    # Roots are only used for equivalence. Geometric ordering drives search.
    boundary_roots = {graph.roots[graph.pad_nodes[p][0]] for p in boundaries}
    first = min(boundary_roots, key=lambda r: tuple((p.x_nm, p.y_nm) for p in groups[r]))
    reached = set(groups[first])
    reached_roots = {first}
    alternate_groups = {}
    rule = next((r for r in board.net_routing_rules if r.net == obligation.net), None)
    alternate_layers = tuple(l for l in (options.allowed_layers or board.stackup.copper_layers)
        if l != layer and l in board.stackup.copper_layers and (not rule or not rule.allowed_layers or l in rule.allowed_layers))
    pending = {r: points for r, points in groups.items() if r != first}
    tracks = []
    blocked = set()
    search_remaining, branch_remaining = options.search_states, options.branch_search_states
    while pending:
        choices = [(abs(a.x_nm-b.x_nm)+abs(a.y_nm-b.y_nm), b.x_nm, b.y_nm, a.x_nm, a.y_nm, root, a, b)
                   for root, points in pending.items() if root not in blocked for b in points for a in reached]
        if not choices:
            return None, 'no_tree_within_budget'
        # Bound anchors on the growing tree too; no quadratic unbounded search.
        root = sorted(choices, reverse=reverse)[0][5]
        endpoints = _attachment_pairs([row for row in sorted(choices, reverse=reverse) if row[5] == root],
                                      options.maximum_anchors)
        work = []
        branch = _attachment_branch(board, obligation.net, layer, endpoints, width, clearance, options,
                                    search_remaining, detour, failures, work=work)
        search_remaining -= sum(work)
        if branch is None and options.maximum_branch_layers:
            alternatives = []
            for other in alternate_layers:
                if other not in alternate_groups:
                    alternate_groups[other] = _anchors(board, obligation.net, graph, contacts,
                        obligation.terminals, other, options.maximum_anchors)
                anchors = alternate_groups[other]
                if anchors is None:
                    continue
                def launchable(point):
                    return clearance.area_clear(obligation.net, _conflict_geometry(RoundedConvexShape(
                        (point,), (board.rules.minimum_track_width_nm+1)//2)), other)
                starts = [p for r in reached_roots for p in anchors.get(r, ()) if launchable(p)]
                ends = tuple(p for p in anchors.get(root, ()) if launchable(p))
                pairs = [(abs(p.x_nm-q.x_nm)+abs(p.y_nm-q.y_nm), p.x_nm, p.y_nm, q.x_nm, q.y_nm, p, q)
                         for p in starts for q in ends]
                if not pairs:
                    continue
                # A secondary branch can only use actual contacts of both
                # original explicit components. A layer name or zone outline
                # cannot supply an invented transition through a reference.
                alternatives.append((other, _attachment_pairs(sorted(pairs), options.maximum_anchors)))
                if len(alternatives) >= options.maximum_branch_layers:
                    break
            for index, (other, pairs) in enumerate(alternatives):
                work = []
                branch = _attachment_branch(board, obligation.net, other, pairs, width, clearance, options,
                    branch_remaining//(len(alternatives)-index), detour, failures, work=work)
                branch_remaining -= sum(work)
                if branch is not None:
                    break
        if branch is None:
            # Another load may first extend the connected tree into open
            # space. Defer this contact, then retry only after real growth.
            # All deferred contacts share this growth step's maze allowance.
            blocked.add(root)
            continue
        tracks.extend(t for t in branch if t not in board.tracks and t not in tracks)
        reached_roots.add(root)
        pending.pop(root)
        blocked.clear()
        search_remaining, branch_remaining = options.search_states, options.branch_search_states
        # Reconstruct from every reached component, including legal vertices
        # of our completed branches on this layer. Do not permanently discard
        # a less crowded parent merely because another load was nearer.
        reached = {p for r in reached_roots for p in groups[r]}
        reached.update(p for t in tracks if t.layer == layer for p in (t.start, t.end))
        if pending:
            targets = [p for points in pending.values() for p in points]
            nearby = sorted(reached, key=lambda p: (min(abs(p.x_nm-q.x_nm)+abs(p.y_nm-q.y_nm)
                for q in targets), p.x_nm, p.y_nm))[:max(1, options.maximum_anchors//2)]
            reached = set(_sample_anchors(reached, options.maximum_anchors, nearby))
    return tuple(tracks), 'proposed'


def _tree_branch(board, net, layer, a, b, width, clearance, options, search_states, detour, failures, *, work=None):
    waypoints = [a, b]
    if detour:
        middle = Point((a.x_nm+b.x_nm)//2, (a.y_nm+b.y_nm)//2)
        offset = max(width*2, options.detour_nm//2) * detour
        waypoint = (Point(middle.x_nm, middle.y_nm+offset) if abs(a.x_nm-b.x_nm) >= abs(a.y_nm-b.y_nm)
                    else Point(middle.x_nm+offset, middle.y_nm))
        waypoints = [a, waypoint, b]
    branch = []
    for start, end in zip(waypoints, waypoints[1:]):
        expanded, blockers = [], set()
        def observe(phase, event, data):
            if phase == 'surface_bridge_search' and event == 'finished':
                expanded.append(data.get('expanded_states', 0))
                blockers.update(b['identity'] for b in data.get('blocker_records', ()))
        from .pin_escape import checked_access_paths
        part = next(checked_access_paths(board, clearance, net, start, end, width, layer), None)
        budget = search_states//(len(waypoints)-1)
        steps = (options.step_nm,) if budget else ()
        if (budget >= 2 and options.coarse_step_nm > options.step_nm
                and max(abs(start.x_nm-end.x_nm), abs(start.y_nm-end.y_nm)) >= 4*options.coarse_step_nm):
            steps = (options.coarse_step_nm, options.step_nm)
        for step in steps:
            if part is not None:
                break
            part = surface_path_between(board, clearance, net, start, end, width, layer,
                step_nm=step, maximum_detour_nm=options.detour_nm,
                state_budget=budget//len(steps), on_progress=observe)
        if work is not None:
            work.append(sum(expanded))
        if part is None:
            if failures is not None:
                failures.append(PowerSearchFailure(net, layer, start, end, width,
                                                   sum(expanded), tuple(sorted(blockers))))
            return None
        branch.extend(part)
    return tuple(branch)


def _zones(tracks, identity, minimum_width):
    layers = sorted({t.layer for t in tracks}, key=lambda l: l.value)
    if len(layers) <= 1:
        return _layer_zones(tracks, identity, minimum_width)
    from shapely.geometry import GeometryCollection
    zones, geometries = [], []
    for layer in layers:
        pieces, geometry = _layer_zones(tuple(t for t in tracks if t.layer == layer),
                                        f'{identity}:{layer.value}', minimum_width)
        zones.extend(pieces)
        geometries.append(geometry)
    # Copper on distinct layers contributes separately to the area objective.
    return tuple(zones), GeometryCollection(geometries)


def _layer_zones(tracks, identity, minimum_width):
    from shapely.ops import unary_union
    capsules = [_shape_geometry(RoundedConvexShape((t.start, t.end), t.width_nm//2)) for t in tracks]
    geometry = unary_union(capsules)
    if geometry.is_empty:
        return (), geometry
    polygons = [geometry] if geometry.geom_type == 'Polygon' else list(geometry.geoms)
    # A tree can enclose a void where its branches touch existing contacts.
    # The exporter cannot encode zone holes. Retain the original overlapping
    # capsules in that case: their union is exactly the proposal, without
    # filling the void or introducing global keepouts on another rail.
    if any(p.interiors for p in polygons):
        polygons = capsules
    zones = []
    for index, polygon in enumerate(polygons):
        def ring(coords):
            return PolygonRing(tuple(Point(round(x), round(y)) for x, y in list(coords)[:-1]))
        zones.append(CopperZone(f'power-area:{identity}:{index}', tracks[0].net, (tracks[0].layer,),
            PolygonWithHoles(ring(polygon.exterior.coords), tuple(ring(h.coords) for h in polygon.interiors)),
            priority=index, minimum_width_nm=minimum_width, pad_connection=ZoneConnection.SOLID,
            island_policy=IslandPolicy.KEEP_ALL, minimum_island_area_nm2=None))
    return tuple(zones), geometry


def _join_layer_contacts(board, obligation, layer, options, observations):
    """Join missing layer accesses through real same-net components.

    A local bridge creates no plated transition and assumes no zone fill.
    Alternatives and deferred roots share one branch allowance until actual
    graph growth; the allowance resets only after a proved component join.
    """
    rule = next((r for r in board.net_routing_rules if r.net == obligation.net), None)
    layers = tuple(l for l in (options.allowed_layers or board.stackup.copper_layers)
                   if l in board.stackup.copper_layers and l != layer
                   and (not rule or not rule.allowed_layers or l in rule.allowed_layers))
    width = max(board.rules.minimum_track_width_nm, (rule.width_nm or 0) if rule else 0)
    remaining = options.branch_search_states
    while True:
        graph = explicit_copper_connectivity(board, only_nets=frozenset({obligation.net}),
                                             include_internal_connections=False)
        contacts, _, _ = _explicit_copper_contacts(board, only_nets=frozenset({obligation.net}),
                                                   include_internal_connections=False)
        groups = _anchors(board, obligation.net, graph, contacts, obligation.terminals,
                          layer, options.maximum_anchors)
        if groups is None:
            return board
        missing = {r for r, points in groups.items() if not points}
        if not missing:
            return board
        terminals = {r: tuple(p for p in obligation.terminals
                             if graph.roots[graph.pad_nodes[p][0]] == r) for r in missing}
        others = {l: _anchors(board, obligation.net, graph, contacts, obligation.terminals,
                              l, options.maximum_anchors) for l in layers}
        clearance = _AreaClearance(board, options)
        grew = False
        for root in sorted(missing, key=lambda r: terminals[r]):
            available = tuple(l for l in layers if others[l] is not None and others[l].get(root))
            alternatives = []
            for other in available:
                starts = [p for r, points in groups.items() if points for p in others[other].get(r, ())]
                ends = others[other][root]
                pairs = [(abs(p.x_nm-q.x_nm)+abs(p.y_nm-q.y_nm), p.x_nm, p.y_nm,
                          q.x_nm, q.y_nm, p, q) for p in starts for q in ends]
                if pairs:
                    alternatives.append((other, _attachment_pairs(sorted(pairs), options.maximum_anchors)))
                if len(alternatives) == options.maximum_branch_layers:
                    break
            failures, expanded = [], 0
            for index, (other, pairs) in enumerate(alternatives):
                work = []
                branch = _attachment_branch(board, obligation.net, other, pairs, width, clearance,
                    options, remaining//(len(alternatives)-index), 0, failures, work=work)
                used = sum(work)
                remaining -= used
                expanded += used
                if branch is None:
                    continue
                candidate = replace(board, tracks=(*board.tracks, *(t for t in branch if t not in board.tracks)))
                proof = explicit_copper_connectivity(candidate, only_nets=frozenset({obligation.net}),
                                                     include_internal_connections=False)
                target = proof.roots[proof.pad_nodes[terminals[root][0]][0]]
                if not any(target == proof.roots[proof.pad_nodes[p][0]] for p in obligation.terminals
                           if groups[graph.roots[graph.pad_nodes[p][0]]]):
                    continue
                observations.append(PowerContactObservation(obligation.net, layer, terminals[root],
                    'joined_existing_component', available, other, expanded))
                board, grew, remaining = candidate, True, options.branch_search_states
                break
            if grew:
                break  # Recompute components, accesses and physical obstacles.
            observations.append(PowerContactObservation(obligation.net, layer, terminals[root],
                'contact_layer_unreachable', available, expanded_states=expanded,
                sampled_blockers=tuple(sorted({b for f in failures for b in f.sampled_blockers}))[:32]))
        if not grew:
            return board


def _prepare_contacts(board, obligation, layer, graph, contacts, options, observations=None, *,
                      alternatives=None, trials=None):
    """Use the ordinary bounded plane-contact proposer, then prove its joins.

    One representative per explicit component is enough. The temporary zone is
    a search target only and never appears in the returned board or evidence.
    """
    from .plane import PlaneStitchOptions, stitch_zone_pads
    from .placement import transformed_pad_position
    anchors = _anchors(board, obligation.net, graph, contacts, obligation.terminals, layer, options.maximum_anchors)
    if anchors is None:
        return None, 'ambiguous_terminal'
    missing = {r for r, points in anchors.items() if not points}
    if not missing:
        return board, 'existing_contacts'
    if len(missing) > options.maximum_new_contacts:
        return None, 'contact_budget_exhausted'
    poses = {p.reference: p for p in board.placements}
    selected = []
    for root in missing:
        members = [p for p in obligation.terminals if graph.roots[graph.pad_nodes[p][0]] == root]
        selected.append(min(members, key=lambda p: (transformed_pad_position(board, poses[p.component], p.pad).x_nm,
                                                    transformed_pad_position(board, poses[p.component], p.pad).y_nm)))
    identity = 'power-area-contact:' + sha256(repr((obligation.net, layer)).encode()).hexdigest()[:20]
    target = CopperZone(identity, obligation.net, (layer,), PolygonWithHoles(PolygonRing(board.outline.vertices),
        tuple(PolygonRing(c.vertices) for c in board.outline.cutouts)),
        pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.KEEP_ALL, minimum_island_area_nm2=None)
    temporary = replace(board, zones=(*board.zones, target),
                        copper_keepouts=(*board.copper_keepouts, *options.protected_corridors),
                        tracks=(*board.tracks, *(t for t in options.reserved_tracks if t not in board.tracks)),
                        vias=(*board.vias, *(v for v in options.reserved_vias if v not in board.vias)))
    if options.maximum_contact_alternatives > 1:
        from .power_contacts import propose_contact_arrangements
        def prepare_candidate(candidate, representatives):
            candidate = _join_layer_contacts(candidate, obligation, layer, options,
                observations if observations is not None else [])
            cg = explicit_copper_connectivity(candidate, only_nets=frozenset({obligation.net}),
                                               include_internal_connections=False)
            cc, _, _ = _explicit_copper_contacts(candidate, only_nets=frozenset({obligation.net}),
                                                 include_internal_connections=False)
            groups = _anchors(candidate, obligation.net, cg, cc, obligation.terminals, layer, options.maximum_anchors)
            pending = frozenset(p for p in representatives if groups is None
                                or not groups[cg.roots[cg.pad_nodes[p][0]]])
            return candidate, pending
        pool = propose_contact_arrangements(board, temporary, obligation, layer, selected, options,
            prepare_candidate=prepare_candidate if options.maximum_branch_layers else None)
        if trials is not None:
            trials.append(pool.observation)
        prepared = []
        for candidate, contact_id in zip(pool.boards, pool.identities):
            contact_id = sha256(repr((candidate.tracks[len(board.tracks):],
                                     candidate.vias[len(board.vias):])).encode()).hexdigest()[:20]
            prepared.append((candidate, contact_id))
        if alternatives is not None:
            alternatives.extend(prepared)
        return (prepared[0][0], 'contacts_proposed') if prepared else (None, pool.observation['status'])
    stitched = stitch_zone_pads(temporary, PlaneStitchOptions(only_pads=frozenset(selected),
        preferred_ground_pads=frozenset(), include_surface_zones=True,
        maximum_radius_nm=options.detour_nm, step_nm=min(500000, options.detour_nm),
        maze_state_budget=options.search_states, maze_step_nm=options.step_nm))
    # Transfer only appended copper; all source fields are reconstructed from
    # the original board. Do not retain temporary reservations or metadata.
    candidate = replace(board, tracks=(*board.tracks, *stitched.board.tracks[len(temporary.tracks):]),
                        vias=(*board.vias, *stitched.board.vias[len(temporary.vias):]))
    rules = {r.net: r for r in board.net_routing_rules}
    rule = rules.get(obligation.net)
    if rule and rule.max_vias is not None and sum(v.net == obligation.net for v in candidate.vias) > rule.max_vias:
        return None, 'via_budget_exceeded'
    observations = observations if observations is not None else []
    if options.maximum_branch_layers:
        candidate = _join_layer_contacts(candidate, obligation, layer, options, observations)
    else:
        cg = explicit_copper_connectivity(candidate, only_nets=frozenset({obligation.net}), include_internal_connections=False)
        cc, _, _ = _explicit_copper_contacts(candidate, only_nets=frozenset({obligation.net}), include_internal_connections=False)
        groups = _anchors(candidate, obligation.net, cg, cc, obligation.terminals, layer, options.maximum_anchors)
        if groups is not None:
            for root, points in groups.items():
                if not points:
                    observations.append(PowerContactObservation(obligation.net, layer,
                        tuple(p for p in obligation.terminals if cg.roots[cg.pad_nodes[p][0]] == root),
                        'contact_layer_unreachable'))
    return candidate, 'contacts_proposed'


def plan_power_areas(board, options=None, *, _seed_layer=None, _seed_tree=None, _seed_contact=None,
                     _preferred_contact=None, on_progress=None):
    if _seed_contact is not None and _preferred_contact is not None:
        raise ValueError('contact seed and preference are mutually exclusive')
    options = options or PowerAreaOptions()
    snapshot = physical_board_digest(board)
    obligations = power_obligations(board, options)
    contract = sha256(repr((options, obligations, _seed_layer)).encode()).hexdigest()
    if _seed_tree is not None:
        contract = sha256(repr((contract, _seed_tree)).encode()).hexdigest()
    if _seed_contact is not None:
        contract = sha256(repr((contract, _seed_contact)).encode()).hexdigest()
    if _preferred_contact is not None:
        contract = sha256(repr((contract, 'preferred_contact', _preferred_contact)).encode()).hexdigest()
    failures, conflict_trials, conflict_contexts, contact_observations = [], [], [], []
    contact_trials = []
    core_observations = []
    def result(status, candidates=(), selected=(), diagnostics=(), states=0, exhausted=False):
        candidates, selected = tuple(candidates), tuple(selected)
        emit(on_progress, 'power_area_search', 'finished', status=status, candidates=len(candidates),
             selected=len(selected), allocation_states=states, conflict_trials=len(conflict_trials))
        generated = tuple(z for c in candidates if c.allocation.id in selected for z in c.zones)
        partitioned = tuple(sorted(z.id for z in board.zones if z.id in options.partitionable_zone_ids
            and any(g.net != z.net and set(g.layers) & set(z.layers)
                    and _zone_geometry(g).intersection(_zone_geometry(z)).area > 0 for g in generated)))
        limits = tuple((name, getattr(options, name)) for name in ('maximum_domains', 'candidates_per_domain',
            'maximum_layer_alternatives', 'maximum_terminals', 'maximum_anchors', 'maximum_new_contacts',
            'search_states', 'allocation_states', 'maximum_conflict_trials', 'maximum_branch_layers', 'branch_search_states', 'maximum_native_trials',
            'step_nm', 'coarse_step_nm', 'detour_nm', 'maximum_contact_alternatives',
            'contact_candidates_per_terminal', 'contact_site_queries', 'contact_assignment_states', 'contact_pair_queries'))
        return PowerAreaPlan(snapshot, contract, status, obligations, candidates, selected,
                             tuple(diagnostics), states, exhausted, limits, partitioned, board, tuple(failures),
                             tuple(conflict_trials), tuple(conflict_contexts), tuple(contact_observations), tuple(contact_trials),
                             tuple(core_observations))
    if not obligations:
        return result('no_domains')
    if len(obligations) > options.maximum_domains:
        return result('domain_budget_exhausted', exhausted=True)
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        return result('macros_not_materialized')
    try:
        clearance = _AreaClearance(board, options)
    except ImportError:
        return result('geometry_dependency_unavailable')
    selected_nets = frozenset(o.net for o in obligations)
    contacts, _, _ = _explicit_copper_contacts(board, only_nets=selected_nets, include_internal_connections=False)
    graph = explicit_copper_connectivity(board, only_nets=selected_nets, include_internal_connections=False)
    rules = {r.net: r for r in board.net_routing_rules}
    candidates, shapes, diagnostics = [], {}, []
    for obligation in obligations:
        rule = rules.get(obligation.net)
        if obligation.status != 'boundary_known' or len(obligation.terminals) > options.maximum_terminals:
            diagnostics.append((obligation.net, obligation.status if obligation.status != 'boundary_known' else 'terminal_budget_exhausted'))
            continue
        if rule and rule.kind is not RouteKind.GENERAL:
            diagnostics.append((obligation.net, 'specialized_route_required'))
            continue
        minimum = max(board.rules.minimum_track_width_nm, (rule.width_nm or 0) if rule else 0)
        layers = [l for l in (options.allowed_layers or board.stackup.copper_layers)
                  if (_seed_layer is None or l == _seed_layer) and l in board.stackup.copper_layers
                  and (not rule or not rule.allowed_layers or l in rule.allowed_layers)]
        # Existing complete contacts precede new ones; retain source layer order
        # for ties. Missing contacts consume the same finite layer allowance.
        layer_anchors = {l: _anchors(board, obligation.net, graph, contacts, obligation.terminals, l, options.maximum_anchors)
                         for l in layers}
        # An existing component with no launch outside a protected area cannot
        # form a tree on this layer. Do not spend its alternative on a reference
        # plane when another permitted distribution layer is available.
        layers = [l for l in layers if layer_anchors[l] is None or not any(points and not any(
            clearance.area_clear(obligation.net, _conflict_geometry(RoundedConvexShape((p,), (minimum+1)//2)), l)
            for p in points) for points in layer_anchors[l].values())]
        reachable = {l: layer_anchors[l] is not None and all(layer_anchors[l].values()) for l in layers}
        layers = sorted(layers, key=lambda l: not reachable[l])[:options.maximum_layer_alternatives]
        if not layers:
            diagnostics.append((obligation.net, 'contact_layer_unreachable'))
        prepared = {}
        for attempt in range(options.candidates_per_domain if layers else 0):
            layer = layers[attempt % len(layers)]
            emit(on_progress, 'power_area_candidate', 'started', net=obligation.net,
                 attempt=attempt, layer=layer.value)
            variant = attempt // len(layers)
            identity = sha256(repr((contract, obligation.net, layer.value, attempt)).encode()).hexdigest()[:20]
            if layer not in prepared:
                contact_pool = []
                contact_board, contact_status = _prepare_contacts(board, obligation, layer, graph, contacts, options,
                    contact_observations, alternatives=contact_pool, trials=contact_trials)
                if contact_board is None:
                    prepared[layer] = ()
                    diagnostics.append((obligation.net, contact_status))
                else:
                    scenes = []
                    for contact_index,(cb, cid) in enumerate(contact_pool or ((contact_board, ''),)):
                        if cb is board:
                            scenes.append((board, graph, contacts, clearance, cid, contact_index))
                        else:
                            cg = explicit_copper_connectivity(cb, only_nets=frozenset({obligation.net}), include_internal_connections=False)
                            cc, _, _ = _explicit_copper_contacts(cb, only_nets=frozenset({obligation.net}), include_internal_connections=False)
                            scenes.append((cb, cg, cc, _AreaClearance(cb, options), cid, contact_index))
                    def missing_contacts(scene):
                        groups=_anchors(scene[0],obligation.net,scene[1],scene[2],obligation.terminals,
                                        layer,options.maximum_anchors)
                        return len(obligation.terminals)+1 if groups is None else sum(not points for points in groups.values())
                    # Prefer complete access, then short physical contact paths.
                    # A no-new-via choice may have grown a long surface bridge
                    # across neighbouring rails. Its initially empty proposal
                    # must not outrank an already checked short plated contact.
                    # Keep original pool identities for explicit replay seeds.
                    scenes.sort(key=lambda scene:(missing_contacts(scene),
                        sum(isqrt((t.end.x_nm-t.start.x_nm)**2+(t.end.y_nm-t.start.y_nm)**2)
                            for t in scene[0].tracks[len(board.tracks):]),
                        len(scene[0].vias)-len(board.vias),scene[5]))
                    prepared[layer] = tuple(scenes)
            if not prepared[layer]:
                continue
            contact_seed = _seed_contact if _seed_contact is not None else _preferred_contact
            contact_position = (variant % len(prepared[layer]) if contact_seed is None else
                next(i for i,scene in enumerate(prepared[layer]) if scene[5] == contact_seed % len(prepared[layer])))
            # A pool index from another obstacle scene is a repair preference,
            # not a verified contact. Preserve complete preferences, but do not
            # let an incomplete old bank hide a freshly checked complete one.
            # Strict diagnostic replay still honours its explicit seed.
            if (_preferred_contact is not None and missing_contacts(prepared[layer][contact_position])
                    and not missing_contacts(prepared[layer][0])):
                diagnostics.append((obligation.net,
                    f'contact_preference_reselected:{prepared[layer][contact_position][5]}:{prepared[layer][0][5]}'))
                contact_position = 0
            tree_variant = variant // len(prepared[layer])
            width, reverse, detour = (_seed_tree if _seed_tree is not None else
                (minimum * (1 + tree_variant//3), bool(attempt % 2), (0, 1, -1)[tree_variant % 3]))
            if width < minimum:
                raise ValueError('power tree seed violates minimum width')
            contact_board, contact_graph, contact_objects, contact_clearance, contact_id, contact_variant = prepared[layer][contact_position]
            tracks, status = _tree(contact_board, obligation, layer, width, contact_clearance, contact_graph, contact_objects,
                                   options, reverse=reverse, detour=detour, failures=failures)
            if tracks is None:
                diagnostics.append((obligation.net, status))
                continue
            zones, geometry = _zones(tracks, identity, minimum)
            priority = 1 + max((z.priority for z in board.zones), default=-1)
            zones = tuple(replace(z, priority=priority + index) for index, z in enumerate(zones))
            vias = contact_board.vias[len(board.vias):]
            tracks = (*contact_board.tracks[len(board.tracks):], *tracks)
            from .power_contact_normalization import normalize_planar_contacts
            tracks, vias, normalized = normalize_planar_contacts(board, tracks, vias, options)
            if normalized:
                diagnostics.append((obligation.net, f'planar_contacts_normalized:{normalized}'))
            # The empty tree means pre-existing explicit copper already joins
            # all terminals. Retain a zero-addition candidate without false fill.
            network, limits, references, unresolved = None, (), (), ('electrical_inputs_missing',)
            model = next((m for m in options.electrical_models if m.net == obligation.net), None)
            if model is not None:
                from .power_extract import screen_power_model
                screen = screen_power_model(replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias)), model,
                                            external_boundaries=options.external_boundaries)
                network, limits, references, unresolved = screen.network, screen.limits, screen.references, screen.unresolved
            allocation = PowerAreaCandidate(identity, obligation.net, snapshot, frozenset({f'candidate:{identity}'}),
                network, limits, max(1, round(geometry.area)), via_count=len(vias), voltage_references=references)
            candidates.append(PhysicalAreaCandidate(allocation, tracks, zones, layer, unresolved, vias,
                                                     width, reverse, detour, contact_variant, contact_id))
            shapes[identity] = _candidate_shapes(board, candidates[-1])
    candidates, allocation = _allocate_pool(board, candidates, shapes, clearance, selected_nets, snapshot,
                                            options, options.allocation_states)
    states, exhausted, tried = allocation.states, allocation.exhausted, set()
    missing_contexts_seen, missing_attempts, regrowth_attempts = set(), {}, {}
    conflict_core = None
    repair_groups = []
    rejected_contexts = set()
    def obstacle_key(candidate):
        tracks=tuple(sorted((t.net,t.layer.value,t.width_nm,
            *sorted(((t.start.x_nm,t.start.y_nm),(t.end.x_nm,t.end.y_nm)))) for t in candidate.tracks))
        # Equivalent paths can enumerate zone vertices in opposite directions.
        # Normalize polygon coordinates as well as generated identities.
        zones = tuple(sorted((z.net, tuple(l.value for l in z.layers), z.clearance_nm,
            _zone_geometry(z).normalize().wkb) for z in candidate.zones))
        return tracks,tuple(sorted(candidate.vias,key=repr)),zones
    eligible_missing = {o.net for o in obligations if o.status == 'boundary_known'
        and len(o.terminals) <= options.maximum_terminals
        and (not rules.get(o.net) or rules[o.net].kind is RouteKind.GENERAL)}
    def missing_layers(net):
        return tuple(l for l in (options.allowed_layers or board.stackup.copper_layers)
            if l in board.stackup.copper_layers and (_seed_layer is None or l == _seed_layer)
            and (not rules.get(net) or not rules[net].allowed_layers or l in rules[net].allowed_layers)
        )[:options.maximum_layer_alternatives]
    # A rejected joint assignment can need a new topology, not more allocator
    # states. Reuse the same physical proposer on a disposable board containing
    # compatible alternatives as obstacles. This does not commit those
    # alternatives: the enlarged pool is certified and jointly allocated again.
    # Depth is exactly one; all allocation passes share the original state cap.
    while (not allocation.selected and not exhausted and len(conflict_trials) < options.maximum_conflict_trials):
        counts = {n: sum(c.allocation.domain == n for c in candidates) for n in selected_nets}
        pairs = [(own, held) for own in candidates for held in candidates
                 if own.allocation.domain != held.allocation.domain
                 and own.allocation.resources & held.allocation.resources
                 and (_tree_seed_key(own), (held.allocation.id,)) not in tried]
        if states >= options.allocation_states:
            exhausted = states >= options.allocation_states
            break
        by_domain = {net: tuple(c for c in candidates if c.allocation.domain == net) for net in selected_nets}
        unavoidable = {frozenset((first, second)) for first in selected_nets for second in selected_nets if first < second
            and by_domain[first] and by_domain[second]
            and all(a.allocation.resources & b.allocation.resources for a in by_domain[first] for b in by_domain[second])}
        # Pairwise-compatible choices need not have a joint assignment. Use
        # an actual failed allocator prefix to regrow against multiple owners
        # together. Prefixes are already certified mutually compatible; they
        # remain disposable obstacles, never an accepted partial solution.
        by_id = {c.allocation.id: c for c in candidates}
        # A compatible prefix can contain an irrelevant earlier domain. While
        # an unavoidable pair remains, repairing that necessary conflict takes
        # precedence; otherwise the prefix repeatedly selects its blocked owner
        # and starves the reverse direction. Multi-owner trials still handle
        # failures after every domain pair has a compatible alternative.
        prefixes = allocation.blocked_prefixes
        if not unavoidable and all(by_domain.values()):
            # An allocator prefix can contain unrelated power domains. Isolate
            # a certified conflict before using those owners as obstacles.
            # Recheck a cached core after every pool change; a newly feasible
            # core triggers a fresh full-pool reduction, never partial adoption.
            for names in dict.fromkeys((conflict_core or tuple(sorted(selected_nets)),
                                        tuple(sorted(selected_nets)))):
                core = reduce_power_conflict(tuple(c.allocation for c in candidates if c.allocation.domain in names),
                    domains=names, snapshot=snapshot, maximum_states=options.allocation_states-states,
                    maximum_candidates=options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials,
                    allow_unassessed_geometry=True)
                states += core.states
                core_observations.append(dict(domains=core.domains, states=core.states, exhausted=core.exhausted,
                    feasible=bool(core.allocation.selected), checks=core.checks))
                exhausted = core.exhausted or states >= options.allocation_states
                if exhausted:
                    break
                if not core.allocation.selected:
                    conflict_core, prefixes = core.domains, core.allocation.blocked_prefixes
                    break
                conflict_core = None
            if exhausted:
                break
            if conflict_core and len(conflict_core)>2:
                # Reverse multi-owner repairs as well as pair repairs. The
                # last blocked domain in a DFS must not consume every trial
                # while an earlier provisional rail is the one that can move.
                # Remember overlapping cores: repairing only the newest one
                # can undo the choices needed by its previously solved neighbour.
                # A joint held assignment also rules out moving an owner that
                # cannot address all overlapping conflicts in the finite pool.
                group=set(conflict_core);separate=[]
                for previous in repair_groups:
                    if group & previous:group.update(previous)
                    else:separate.append(previous)
                repair_groups=[*separate,frozenset(group)]
                for names in dict.fromkeys((tuple(sorted(group)),conflict_core)):
                    directions=power_repair_contexts(tuple(c.allocation for c in candidates),
                        domains=selected_nets,repair_domains=names,snapshot=snapshot,
                        maximum_states=options.allocation_states-states,
                        maximum_candidates=options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials,
                        rejected_contexts=rejected_contexts)
                    states+=directions.states
                    core_observations.append(dict(domains=names,states=directions.states,
                        exhausted=directions.exhausted,kind='repair_directions',contexts=directions.contexts))
                    exhausted=directions.exhausted or states>=options.allocation_states
                    if exhausted:break
                    if directions.contexts:
                        # A compatible held group need not admit a new physical
                        # route. Keep the current core's dead-end witnesses as
                        # local fallbacks; fair attempts can then change a
                        # neighbour instead of freezing it through every retry.
                        prefixes=tuple(dict.fromkeys((*prefixes,*directions.contexts)))
                        break
                if exhausted:break
        contexts = ((own, tuple(by_id[i] for i in identities))
            for net, identities in prefixes if len(identities) > 1
            for own in by_domain[net]
            if (_tree_seed_key(own), tuple(sorted(identities))) not in tried)
        # A separate distribution layer can avoid a foreign area without
        # inventing transitions. The ordinary proposer must still prove every
        # contact and the complete allocation is recertified after regrowth.
        context = None if unavoidable else min(contexts, key=lambda row: (
            regrowth_attempts.get(row[0].allocation.domain, 0),
            -len(row[1]),
            (row[0].allocation.domain,tuple(sorted(c.allocation.id for c in row[1]))) in rejected_contexts,
            row[0].layer in {c.layer for c in row[1]},
            row[0].layer in {board.stackup.copper_layers[0], board.stackup.copper_layers[-1]},
            row[0].allocation.area_nm2,
            row[0].layer.value, row[0].allocation.id, tuple(c.allocation.id for c in row[1])), default=None)
        missing = min(((net, held, layer) for net in sorted(eligible_missing) if not by_domain[net]
            for held in sorted(candidates,key=lambda c:(c.allocation.area_nm2,c.allocation.id))
            if held.tracks or held.vias or held.zones
            for layer in missing_layers(net)
            if (net,layer,obstacle_key(held)) not in missing_contexts_seen),
            key=lambda row:missing_attempts.get(row[0],0),default=None)
        if missing is not None:
            # Missing initial candidates still have a source domain and legal
            # layers. A foreign provisional corridor can change the finite
            # contact/boundary pool; regenerate against it without pretending
            # the held choice has been accepted. Full same-source allocation
            # below remains mandatory, including every missing domain.
            net,held,layer=missing
            missing_contexts_seen.add((net,layer,obstacle_key(held)))
            missing_attempts[net]=missing_attempts.get(net,0)+1
            held=(held,);seed_tree=seed_contact=None
            key=('missing',net,layer.value)
        elif context is None:
            if not pairs:
                break
            own, held = min(pairs, key=lambda pair: (
                frozenset(c.allocation.domain for c in pair) not in unavoidable,
                regrowth_attempts.get(pair[0].allocation.domain, 0), counts[pair[1].allocation.domain],
                -counts[pair[0].allocation.domain], pair[1].allocation.area_nm2,
                pair[0].allocation.area_nm2, pair[0].allocation.id, pair[1].allocation.id))
            held = (held,)
            net,layer=own.allocation.domain,own.layer
            seed_tree=(own.tree_width_nm,own.tree_reverse,own.tree_detour)
            seed_contact=own.contact_variant;key=_tree_seed_key(own)
        else:
            own, held = context
            net,layer=own.allocation.domain,own.layer
            seed_tree=(own.tree_width_nm,own.tree_reverse,own.tree_detour)
            seed_contact=own.contact_variant;key=_tree_seed_key(own)
        identities = tuple(sorted(c.allocation.id for c in held))
        # A small candidate family is not evidence that its geometry must be
        # held. Cover both repair directions before repeatedly changing only
        # the neighbor with more currently generated alternatives.
        regrowth_attempts[net] = regrowth_attempts.get(net, 0) + 1
        emit(on_progress, 'power_area_regrowth', 'started', net=net,
             layer=layer.value, held_domains=[c.allocation.domain for c in held], trial=len(conflict_trials))
        tried.add((key, identities))
        disposable = replace(board, tracks=(*board.tracks, *(t for c in held for t in c.tracks)),
            vias=(*board.vias, *(v for c in held for v in c.vias)),
            zones=(*board.zones, *(z for c in held for z in c.zones)), zone_fills=())
        trial = plan_power_areas(disposable, replace(options, nets=(net,),
            plane_return_nets=tuple(n for n in options.plane_return_nets if n == net),
            maximum_domains=1, candidates_per_domain=1,
            maximum_layer_alternatives=1, maximum_conflict_trials=0,
            allocation_states=options.allocation_states-states), _seed_layer=layer,
            _seed_tree=seed_tree, _preferred_contact=seed_contact, on_progress=on_progress)
        states += trial.allocation_states
        exhausted = trial.exhausted or states >= options.allocation_states
        label = identities[0] if len(identities) == 1 else 'joint:'+sha256(repr(identities).encode()).hexdigest()[:20]
        conflict_trials.append((net, label, trial.status))
        conflict_contexts.append((net, identities))
        contact_observations.extend(trial.contact_observations)
        contact_trials.extend(trial.contact_trials)
        failures.extend(replace(f, held_candidate_ids=identities) for f in trial.search_failures)
        diagnostics.extend((name, 'conflict_trial:'+status) for name, status in trial.diagnostics if name == net)
        if exhausted:
            break
        if not trial.candidates:
            # A failure concerns this exact provisional combination. Retain
            # every candidate, but let the next context query choose different
            # neighbours before retrying the same obstruction on another layer.
            rejected_contexts.add((net,identities))
            continue
        alternative = trial.candidates[0]
        identity = alternative.allocation.id
        if any(c.allocation.id == identity for c in candidates):
            continue
        alternative = replace(alternative, allocation=replace(alternative.allocation,
            snapshot=snapshot, resources=frozenset({f'candidate:{identity}'})))
        candidates = (*candidates, alternative)
        shapes[identity] = _candidate_shapes(board, alternative)
        candidates, allocation = _allocate_pool(board, candidates, shapes, clearance, selected_nets, snapshot,
                                                options, options.allocation_states-states)
        states += allocation.states
        exhausted = allocation.exhausted
    diagnostics.extend((identity, status) for identity, status, _ in allocation.assessments)
    return result(allocation.status, candidates, (c.id for c in allocation.selected), diagnostics, states, exhausted)


def refresh_power_area_reservations(board, plan, options, *, on_progress=None):
    """Revalidate a same-source proposal pool and regrow blocked domains only.

    This owns no accepted copper. Every candidate is checked against fresh
    actual reserved copper, corridors and source rules. Regeneration consumes
    remaining conflict slots; allocator passes share the original state cap.
    Native acceptance remains the caller's separate fresh transaction.
    """
    if (plan.source_board != board or plan.snapshot != physical_board_digest(board)
            or plan.obligations != power_obligations(board, options)
            or options.electrical_models
            or len(plan.candidates) > options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials
            or any(getattr(options, k) != v for k,v in plan.work_limits)):
        raise ValueError('same-source geometric proposal pool and unchanged work limits required')
    if plan.allocation_states >= options.allocation_states or len(plan.conflict_trials) > options.maximum_conflict_trials:
        return replace(plan, selected_ids=(), exhausted=True, status='allocation_budget_exhausted')
    clearance = _AreaClearance(board, options)
    diagnostics = list(plan.diagnostics)
    from .power_contact_normalization import normalize_planar_contacts
    def normalize(c):
        tracks, vias, count = normalize_planar_contacts(board, c.tracks, c.vias, options)
        if not count:
            return c
        identity = sha256(repr((c.allocation.id, tracks, vias)).encode()).hexdigest()[:20]
        diagnostics.append((c.allocation.domain, f'planar_contacts_normalized:{count}'))
        return replace(c, tracks=tracks, vias=vias,
            allocation=replace(c.allocation, id=identity, via_count=len(vias)),
            zones=tuple(replace(z,id=z.id.replace(c.allocation.id,identity)) for z in c.zones))
    def eligible(c):
        return (all(clearance.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in c.tracks)
            and all(clearance.can_via(v.net,v.position,v.size_nm,v.from_layer,v.to_layer,v.drill_nm) for v in c.vias)
            and all(clearance.area_clear(z.net,_zone_geometry(z),l) for z in c.zones for l in z.layers))
    normalized = tuple(normalize(c) for c in plan.candidates)
    candidates = tuple(c for c in normalized if eligible(c))
    rejected = tuple(old for old,c in zip(plan.candidates,normalized) if c not in candidates)
    diagnostics.extend((c.allocation.domain,'reservation_geometry_rejected:'+c.allocation.id) for c in rejected)
    states = plan.allocation_states
    trials, contexts = list(plan.conflict_trials), list(plan.conflict_contexts)
    contact_trials = list(plan.contact_trials)
    contact_observations = list(plan.contact_observations)
    search_failures = list(plan.search_failures)
    domains = frozenset(o.net for o in plan.obligations)
    def allocate(pool):
        return _allocate_pool(board,pool,{c.allocation.id:_candidate_shapes(board,c) for c in pool},
            clearance,domains,plan.snapshot,options,options.allocation_states-states)
    candidates, allocation = allocate(candidates)
    states += allocation.states
    # Empty/blocked selected families receive bounded fresh contact/tree search.
    blocked = sorted({c.allocation.domain for c in rejected if c.allocation.id in plan.selected_ids})
    for net in blocked:
        remaining = options.maximum_conflict_trials-len(trials)
        if allocation.selected or states >= options.allocation_states or remaining <= 0:
            break
        room = options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials-len(candidates)
        attempts = min(options.candidates_per_domain,remaining,room)
        if attempts <= 0:
            break
        refreshed = plan_power_areas(board,replace(options,nets=(net,),
            plane_return_nets=tuple(n for n in options.plane_return_nets if n==net),
            maximum_domains=1,candidates_per_domain=attempts,maximum_conflict_trials=0,
            allocation_states=options.allocation_states-states),on_progress=on_progress)
        states += refreshed.allocation_states
        contact_trials.extend(refreshed.contact_trials)
        contact_observations.extend(refreshed.contact_observations)
        search_failures.extend(refreshed.search_failures)
        trials.extend((net,'ordinary-reservation:'+str(i),refreshed.status) for i in range(attempts))
        contexts.append((net,tuple(c.allocation.id for c in rejected if c.allocation.domain==net)))
        candidates = (*candidates, *(c for c in refreshed.candidates if eligible(c)))
        if states >= options.allocation_states:
            break
        candidates, allocation = allocate(candidates)
        states += allocation.states
    selected = tuple(c.id for c in allocation.selected) if states < options.allocation_states else ()
    zones = tuple(z for c in candidates if c.allocation.id in selected for z in c.zones)
    partitioned = tuple(sorted(z.id for z in board.zones if z.id in options.partitionable_zone_ids
        and any(g.net!=z.net and set(g.layers)&set(z.layers)
                and _zone_geometry(g).intersection(_zone_geometry(z)).area>0 for g in zones)))
    return replace(plan,contract_digest=sha256(repr((options,plan.obligations,None)).encode()).hexdigest(),
        candidates=tuple(candidates),selected_ids=selected,diagnostics=tuple(diagnostics),
        allocation_states=states,exhausted=states>=options.allocation_states,
        conflict_trials=tuple(trials),conflict_contexts=tuple(contexts),partitioned_zone_ids=partitioned,
        contact_trials=tuple(contact_trials),contact_observations=tuple(contact_observations),
        search_failures=tuple(search_failures),
        status='geometry_proposal' if selected else 'unresolved')


def _tree_seed_key(candidate):
    return (candidate.allocation.domain, candidate.layer, candidate.tree_width_nm,
            candidate.tree_reverse, candidate.tree_detour, candidate.contact_variant)


def _candidate_shapes(board, candidate):
    from shapely.ops import unary_union
    per_layer = {}
    for track in candidate.tracks:
        per_layer.setdefault(track.layer, []).append(_conflict_geometry(
            RoundedConvexShape((track.start, track.end), (track.width_nm+1)//2)))
    for via in candidate.vias:
        first, last = sorted((board.stackup.copper_layers.index(via.from_layer), board.stackup.copper_layers.index(via.to_layer)))
        for layer in board.stackup.copper_layers[first:last+1]:
            per_layer.setdefault(layer, []).append(_conflict_geometry(RoundedConvexShape((via.position,), (via.size_nm+1)//2)))
    return {layer: unary_union(parts) for layer, parts in per_layer.items()}


def _certify_pool(board, candidates, shapes, clearance):
    # Exact pairwise intersection/clearance certificates, not guessed occupancy.
    # These shared tokens encode precisely the conflicts checked on this snapshot.
    conflicts = {c.allocation.id: {f'candidate:{c.allocation.id}'} for c in candidates}
    indices = {}
    def coexist(first, second):
        # The conservative envelopes are only a broad phase. Drill clearance
        # applies at drills, not to every track in a candidate containing a via.
        # Generated zones are inscribed corridor pieces contained in these
        # tracks; the ordinary exact index owns copper/drill/region spacing.
        if first.allocation.id not in indices:
            indices[first.allocation.id] = RoutingClearanceIndex(replace(board,
                tracks=(*board.tracks, *first.tracks), vias=(*board.vias, *first.vias)))
        index = indices[first.allocation.id]
        # Pad-overlap policy is unchanged by either candidate and remains an
        # independent generation/adoption gate; here only coexistence is tested.
        return (all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in second.tracks)
            and all(index.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer,
                v.drill_nm, allow_pad_overlap=True) for v in second.vias))
    for index, first in enumerate(candidates):
        for second in candidates[:index]:
            a, b = first.allocation, second.allocation
            if a.domain == b.domain:
                continue
            margin = max(board.rules.minimum_clearance_nm, clearance.clearances.get(a.domain, 0), clearance.clearances.get(b.domain, 0),
                         board.rules.minimum_hole_clearance_nm if first.vias or second.vias else 0)
            if (any(shapes[a.id][l].distance(shapes[b.id][l]) < margin for l in shapes[a.id].keys() & shapes[b.id].keys())
                    and not coexist(first, second)):
                token = 'conflict:' + ':'.join(sorted((a.id, b.id)))
                conflicts[a.id].add(token)
                conflicts[b.id].add(token)
    return tuple(replace(c, allocation=replace(c.allocation, resources=frozenset(conflicts[c.allocation.id]))) for c in candidates)


def _allocate_pool(board, candidates, shapes, clearance, selected_nets, snapshot, options, states):
    candidates = _certify_pool(board, candidates, shapes, clearance)
    allocation = allocate_power_areas([c.allocation for c in candidates], domains=selected_nets, snapshot=snapshot,
        maximum_candidates=options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials,
        maximum_states=states, allow_unassessed_geometry=True)
    return candidates, allocation


def apply_power_area_plan(board, plan):
    """Append one joint proposal atomically; rejection returns the exact source.

    Source zones, macro geometry, via arrays, local paths and exits are retained.
    No evidence is carried into the changed board. Native acceptance is pending.
    """
    if physical_board_digest(board) != plan.snapshot:
        raise ValueError('stale power area plan')
    if plan.source_board is None or board.power_domains != plan.source_board.power_domains:
        raise ValueError('stale power area source contracts')
    if not plan.selected or len(plan.selected) != len(plan.obligations):
        return board, 'unresolved'
    tracks = tuple(t for c in plan.selected for t in c.tracks)
    vias = tuple(v for c in plan.selected for v in c.vias)
    zones = tuple(z for c in plan.selected for z in c.zones)
    if not tracks and not zones and not vias:
        return board, 'already_contact_routed'
    candidate = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias), zones=(*board.zones, *zones), zone_fills=())
    def hard(b):
        return {repr(f) for f in run_physical_drc(b).findings if f.severity.value == 'error'
                and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'}}
    if hard(candidate) - hard(board):
        return board, 'hard_geometry_rejected'
    graph = explicit_copper_connectivity(candidate, only_nets=frozenset(o.net for o in plan.obligations),
                                        include_internal_connections=False)
    if any(not graph.net_connected(n) for n in candidate.nets if n.name in {o.net for o in plan.obligations}):
        return board, 'contact_connectivity_rejected'
    return candidate, 'contact_routed_native_pending'


def reselect_power_area_plan(plan, options, rejected_assignments):
    """Choose an untried assignment from the same certified candidate pool.

    Exclude complete rejected combinations, not candidates that may work with
    another domain. All allocator passes share the original state allowance.
    This creates no geometry and cannot promote native connectivity.
    """
    remaining = options.allocation_states - plan.allocation_states
    if remaining <= 0:
        return replace(plan, selected_ids=(), status='allocation_budget_exhausted', exhausted=True)
    allocation = allocate_power_areas([c.allocation for c in plan.candidates],
        domains=tuple(o.net for o in plan.obligations), snapshot=plan.snapshot,
        maximum_candidates=options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials,
        maximum_states=remaining, allow_unassessed_geometry=True,
        excluded_assignments=rejected_assignments)
    return replace(plan, selected_ids=tuple(c.id for c in allocation.selected),
        status=allocation.status, allocation_states=plan.allocation_states+allocation.states,
        exhausted=allocation.exhausted)


def owned_power_copper(pipeline):
    """Exact appended occurrences, separate from ordinary cleanup ownership."""
    preparation = getattr(pipeline, 'power_preparation', None)
    if preparation is not None and preparation.accepted:
        return (preparation.board.tracks[len(preparation.source.tracks):],
                preparation.board.vias[len(preparation.source.vias):])
    plan = getattr(pipeline, 'power_area_plan', None)
    if plan is None or getattr(pipeline, 'power_area_status', None) != 'contact_routed_native_pending':
        return (), ()
    return (tuple(t for c in plan.selected for t in c.tracks),
            tuple(v for c in plan.selected for v in c.vias))
