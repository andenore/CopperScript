"""Bounded, coherent circular-array dogbones on an immutable obstacle scene.

These are contact geometry proposals, not package exits, plane connectivity or
native acceptance. Callers select the required pads explicitly and own later
boundary/area/native transactions. No existing copper or placement is released.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass
from hashlib import sha256
from itertools import combinations
import json
from math import isqrt

from .access_diagnostics import AccessDiagnostics, AccessRecorder
from .access_sites import obstacle_boundary_sites
from .drc import physical_board_digest
from .escape_assignment import EscapeConflicts, _BudgetExceeded
from .fanout import _legal_choices
from .geometry import point_on_segment
from .hard_macros import macro_owned_pads
from .pad_via_arrays import array_rules, require_via_in_pad_arrays
from .physical import (BoardSide, CopperLayer, PadKind, PadReference, PadShape,
                       PhysicalBoard, Point, RouteKind, TrackSegment, Via)
from .placement import transformed_local_point
from .progress import emit
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .routing_vias import physical_via_span


FAMILIES = ('outward', 'inward', 'north_east', 'south_east', 'south_west',
            'north_west', 'staggered_rows', 'staggered_columns')


@dataclass(frozen=True, slots=True)
class PackagePatternOptions:
    families: tuple[str, ...] = FAMILIES
    preferred_offset_nm: int = 0
    maximum_patterns: int = 24
    maximum_pads: int = 256
    maximum_site_queries: int = 24_576
    maximum_pair_checks: int = 200_000
    maximum_pair_queries: int = 2_000_000
    frontier_sites_per_pad: int = 24
    frontier_offsets_per_family: int = 16
    maximum_frontier_queries: int = 2048
    diagnostic_samples_per_pad: int = 8

    def __post_init__(self):
        object.__setattr__(self, 'families', tuple(self.families))
        if (not self.families or len(set(self.families)) != len(self.families)
                or any(f not in FAMILIES for f in self.families)):
            raise ValueError('unique supported package pattern families required')
        for name in self.__dataclass_fields__:
            if name == 'families':
                continue
            value = getattr(self, name)
            if type(value) is not int or value < (0 if name in {'diagnostic_samples_per_pad','preferred_offset_nm'} else 1):
                raise ValueError('package pattern work bounds must be positive integers')


@dataclass(frozen=True, slots=True)
class CircularArray:
    component: str
    xs_nm: tuple[int, ...]
    ys_nm: tuple[int, ...]
    pitch_nm: int
    land_diameter_nm: int


@dataclass(frozen=True, slots=True)
class PackageContactIntent:
    pad: PadReference
    role: str = 'signal'
    target_layers: tuple[CopperLayer, ...] = ()

    def __post_init__(self):
        object.__setattr__(self, 'target_layers', tuple(self.target_layers))
        if self.role not in {'signal', 'supply', 'return'}:
            raise ValueError('contact role must be signal, supply or return')
        if len(set(self.target_layers)) != len(self.target_layers):
            raise ValueError('contact target layers must be unique')


@dataclass(frozen=True, slots=True)
class PatternContact:
    intent: PackageContactIntent
    net: str
    tracks: tuple[TrackSegment, ...]
    via: Via
    reused_via: bool


@dataclass(frozen=True, slots=True)
class PackagePattern:
    source_digest: str
    family: str
    offset_nm: int
    contacts: tuple[PatternContact, ...]
    deferred_components: tuple[str, ...] = ()

    @property
    def tracks(self):
        return tuple(dict.fromkeys(t for c in self.contacts for t in c.tracks))

    @property
    def new_vias(self):
        return tuple(dict.fromkeys(c.via for c in self.contacts if not c.reused_via))


@dataclass(frozen=True, slots=True)
class PatternTrial:
    family: str
    offset_nm: int
    tested_pads: int
    blocked_pads: tuple[PadReference, ...]
    conflicts: tuple[tuple[PadReference, PadReference], ...]
    rule_failures: tuple[str, ...]
    candidate: PackagePattern | None


@dataclass(frozen=True, slots=True)
class PackagePatternResult:
    source_digest: str
    options_digest: str
    status: str
    array: CircularArray | None
    intents: tuple[PackageContactIntent, ...]
    candidates: tuple[PackagePattern, ...] = ()
    trials: tuple[PatternTrial, ...] = ()
    unsupported: tuple[tuple[PadReference, str], ...] = ()
    diagnostics: tuple[tuple[PadReference, AccessDiagnostics], ...] = ()
    site_queries: int = 0
    pair_checks: int = 0
    pair_queries: int = 0
    frontier_queries: int = 0
    evaluated_offsets: int = 0
    deferred_components: tuple[str, ...] = ()

    def report(self):
        return dict(status=self.status, source_digest=self.source_digest,
            options_digest=self.options_digest, array=asdict(self.array) if self.array else None,
            required_pads=[str(i.pad) for i in self.intents],
            unsupported=[(str(p), reason) for p, reason in self.unsupported],
            candidates=[dict(family=c.family, offset_nm=c.offset_nm,
                pads=[str(p.intent.pad) for p in c.contacts], new_vias=len(c.new_vias)) for c in self.candidates],
            trials=[dict(family=t.family, offset_nm=t.offset_nm, tested_pads=t.tested_pads,
                blocked_pads=[str(p) for p in t.blocked_pads],
                conflicts=[(str(a), str(b)) for a,b in t.conflicts],
                rule_failures=t.rule_failures,
                complete_geometry=t.candidate is not None and not self.deferred_components,
                prospective_geometry=t.candidate is not None) for t in self.trials],
            work=dict(site_queries=self.site_queries, pair_checks=self.pair_checks,
                pair_queries=self.pair_queries, frontier_queries=self.frontier_queries,
                evaluated_offsets=self.evaluated_offsets),
            deferred_components=self.deferred_components,
            boundary_verified=False, native_accepted=False)


def circular_array(board, component):
    """Detect a complete square-pitch array in footprint-local coordinates."""
    pose = next((p for p in board.placements if p.reference == component), None)
    if pose is None:
        raise ValueError('unknown package component')
    pads = board.footprints[pose.footprint].pads
    if (not pads or any(p.kind is not PadKind.SMD or p.shape is not PadShape.CIRCLE
                       or p.size.width_nm != p.size.height_nm for p in pads)
            or len({p.number for p in pads}) != len(pads)):
        return None
    points = {p.position for p in pads}
    xs = tuple(sorted({p.x_nm for p in points}))
    ys = tuple(sorted({p.y_nm for p in points}))
    pitches = {b-a for axis in (xs, ys) for a,b in zip(axis, axis[1:])}
    sizes = {p.size.width_nm for p in pads}
    if (min(len(xs), len(ys)) < 3 or len(points) != len(xs)*len(ys)
            or len(points) != len(pads) or len(pitches) != 1 or len(sizes) != 1):
        return None
    return CircularArray(component, xs, ys, next(iter(pitches)), next(iter(sizes)))


@dataclass(frozen=True, slots=True)
class BallGrid:
    """A square-pitch ball grid in footprint-local coordinates, possibly depopulated.

    ``xs_nm``/``ys_nm`` are the ideal column and row coordinates (every
    lattice position, populated or not); ``balls`` are the pad numbers on
    the grid. Other pads of the footprint (mounting or aperture pads) are not
    part of it.
    """
    component: str
    xs_nm: tuple[int, ...]
    ys_nm: tuple[int, ...]
    pitch_nm: int
    land_diameter_nm: int
    balls: frozenset[str]
    tolerance_nm: int = 2_000

    def index(self, position: Point) -> tuple[int, int] | None:
        """(column, row) of a lattice position within tolerance, else None."""
        ix = round((position.x_nm - self.xs_nm[0]) / self.pitch_nm)
        iy = round((position.y_nm - self.ys_nm[0]) / self.pitch_nm)
        if (0 <= ix < len(self.xs_nm) and 0 <= iy < len(self.ys_nm)
                and abs(position.x_nm - self.xs_nm[ix]) <= self.tolerance_nm
                and abs(position.y_nm - self.ys_nm[iy]) <= self.tolerance_nm):
            return ix, iy
        return None


def ball_grid(board, component, *, tolerance_nm: int = 2_000, minimum_density: float = 0.5):
    """Detect a ball grid like ``circular_array``, tolerant of real footprints.

    Footprint coordinates carry rounding (a 1.0 mm pitch read as 0.999 and
    1.001 mm), grids leave positions empty (ULX3S's caBGA-381 fills 381 of
    20x20) and footprints add non-ball pads. The balls are the round SMD
    lands of the most common size; their coordinates must sit on one square
    lattice within ``tolerance_nm``, filling at least ``minimum_density`` of
    it. Rectangular pitches are not supported (None).
    """
    from collections import Counter
    pose = next((p for p in board.placements if p.reference == component), None)
    if pose is None:
        raise ValueError('unknown package component')
    round_lands = [p for p in board.footprints[pose.footprint].pads
                   if p.kind is PadKind.SMD and p.shape is PadShape.CIRCLE and p.size.width_nm == p.size.height_nm]
    if len(round_lands) < 9:
        return None
    size = Counter(p.size.width_nm for p in round_lands).most_common(1)[0][0]
    balls = [p for p in round_lands if p.size.width_nm == size]
    if len({p.number for p in balls}) != len(balls):
        return None

    def axis(values):
        """Column centres, then the pitch they share (None when irregular)."""
        centres, group = [], []
        for value in sorted(values):
            if group and value - group[-1] > tolerance_nm:
                centres.append(round(sum(group) / len(group)))
                group = []
            group.append(value)
        centres.append(round(sum(group) / len(group)))
        if len(centres) < 3:
            return None, None
        step = min(b - a for a, b in zip(centres, centres[1:]))
        # The mean spacing, so rounding in single gaps does not accumulate.
        pitch = round((centres[-1] - centres[0]) / round((centres[-1] - centres[0]) / step))
        if any(abs((b - a) - round((b - a) / pitch) * pitch) > 2 * tolerance_nm
               for a, b in zip(centres, centres[1:])):
            return None, None
        return centres, pitch

    xs, pitch_x = axis([p.position.x_nm for p in balls])
    ys, pitch_y = axis([p.position.y_nm for p in balls])
    if xs is None or ys is None or abs(pitch_x - pitch_y) > 2 * tolerance_nm:
        return None
    pitch = round((pitch_x + pitch_y) / 2)
    nx = round((xs[-1] - xs[0]) / pitch) + 1
    ny = round((ys[-1] - ys[0]) / pitch) + 1
    if len(balls) < minimum_density * nx * ny:
        return None
    grid = BallGrid(component, tuple(xs[0] + k * pitch for k in range(nx)),
                    tuple(ys[0] + k * pitch for k in range(ny)), pitch, size,
                    frozenset(p.number for p in balls), tolerance_nm)
    if any(grid.index(p.position) is None for p in balls):
        return None
    return grid


def _direction(array, pad, family):
    x, y = array.xs_nm.index(pad.position.x_nm), array.ys_nm.index(pad.position.y_nm)
    sx = -1 if 2*x < len(array.xs_nm)-1 else 1
    sy = -1 if 2*y < len(array.ys_nm)-1 else 1
    return {'outward': (sx, sy), 'inward': (-sx, -sy),
            'north_east': (1, -1), 'south_east': (1, 1),
            'south_west': (-1, 1), 'north_west': (-1, -1),
            'staggered_rows': (sx, -1 if y % 2 == 0 else 1),
            'staggered_columns': (-1 if x % 2 == 0 else 1, sy)}[family]


def _length_nm(track):
    squared = ((track.end.x_nm-track.start.x_nm)**2 + (track.end.y_nm-track.start.y_nm)**2)
    length = isqrt(squared)
    return length + (length*length < squared)


def propose_package_patterns(board: PhysicalBoard, component: str, intents,
                             options: PackagePatternOptions | None = None, *, on_progress=None,
                             _deferred_components=frozenset()):
    """Return only complete coexisting contact patterns, with bounded rejections.

    Existing critical copper, pads on both faces and every plated layer remain
    obstacles. No unrequested ball is added. Unsupported obligations reject the
    scope rather than disappear from coverage. Shared contacts and mandatory
    via arrays require separate ownership support and are not generated here.
    """
    options = options or PackagePatternOptions()
    intents = tuple(sorted(intents, key=lambda i: i.pad))
    if not intents or len({i.pad for i in intents}) != len(intents):
        raise ValueError('nonempty unique contact intents required')
    if any(i.pad.component != component for i in intents):
        raise ValueError('contact scope must belong to one package')
    require_via_in_pad_arrays(board, 'package pattern proposals')
    source = physical_board_digest(board)
    opts_digest = sha256(json.dumps(asdict(options), sort_keys=True).encode()).hexdigest()
    array = circular_array(board, component)
    result = PackagePatternResult(source, opts_digest, 'unsupported_array', array, intents,
                                  deferred_components=tuple(sorted(_deferred_components)))
    if any(m.cluster not in board.materialized_macros for m in board.hard_macros):
        from dataclasses import replace
        return replace(result, status='unmaterialized_macro_source')
    if array is None:
        return result
    from dataclasses import replace
    if len(array.xs_nm)*len(array.ys_nm) > options.maximum_pads:
        return replace(result, status='package_pad_budget_exhausted')
    pose = next(p for p in board.placements if p.reference == component)
    if pose.rotation_degrees % 45:
        return replace(result, status='unsupported_rotation')
    pads = {p.number: p for p in board.footprints[pose.footprint].pads}
    membership = {p: n.name for n in board.nets for p in n.pads}
    rules = {r.net:r for r in board.net_routing_rules}
    owned = macro_owned_pads(board) | {r.pad for r in array_rules(board)}
    side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    unsupported, spans = [], {}
    for intent in intents:
        ref = intent.pad
        if ref.pad not in pads or ref not in membership:
            raise ValueError('contact intent requires an actual netted land')
        net = membership[ref]; rule = rules.get(net)
        allowed = routing_layers(board, net, rule)
        targets = intent.target_layers or tuple(l for l in allowed if l is not side)
        # Match the shared legal-choice producer's physical transition. A
        # target outside that actual span is unsupported, not silently reached.
        span = physical_via_span(board, side, next(l for l in allowed if l is not side)) if side in allowed and len(allowed)>1 else None
        if ref in owned:
            unsupported.append((ref, 'protected_macro_or_via_array'))
        elif rule and (rule.kind is not RouteKind.GENERAL or rule.differential_partner or rule.require_return_vias):
            unsupported.append((ref, 'specialized_routing_owner'))
        elif (side not in allowed or not targets or any(l not in allowed or l is side for l in targets)
              or span is None):
            unsupported.append((ref, 'layer_access_unavailable'))
        else:
            layers = board.stackup.copper_layers
            if any(l not in layers[layers.index(span[0]):layers.index(span[1])+1] for l in targets):
                unsupported.append((ref, 'target_outside_physical_span'))
            spans[ref] = span
    if unsupported:
        return replace(result, status='unsupported_obligations', unsupported=tuple(unsupported))
    index = RoutingClearanceIndex(board, _deferred_components=_deferred_components)
    conflicts = EscapeConflicts(board, options.maximum_pair_checks, options.maximum_pair_queries)
    recorders = {i.pad:AccessRecorder(options.diagnostic_samples_per_pad) for i in intents}
    cache, trials, candidates, seen = {}, [], [], set()
    site_queries = frontier_queries = evaluated_offsets = 0
    # Leave one nanometre between the actual land and ordinary via annulus.
    radius = array.land_diameter_nm//2 + board.rules.default_via_size_nm//2 + 1
    near = isqrt((radius*radius+1)//2)
    near += 2*near*near < radius*radius
    center = array.pitch_nm//2
    if near > center:
        return replace(result, status='no_interstitial_offset')
    def endpoint(intent, family, offset):
        pad = pads[intent.pad.pad]; dx,dy = _direction(array, pad, family)
        return transformed_local_point(pose, Point(pad.position.x_nm+dx*offset, pad.position.y_nm+dy*offset))
    def evaluate(family, offset):
        nonlocal site_queries, evaluated_offsets
        evaluated_offsets += 1
        contacts, blocked = [], []
        for intent in intents:
            ref = intent.pad; site = endpoint(intent, family, offset); key = ref,site
            if key not in cache:
                if site_queries >= options.maximum_site_queries:
                    raise _BudgetExceeded('site_budget_exhausted')
                site_queries += 1
                pad = pads[ref.pad]; net = membership[ref]
                start = transformed_local_point(pose, pad.position)
                choices = _legal_choices(board, index, net, start,
                    pose, pad, (site,), recorder=recorders[ref], phase='package_pattern')
                # A straight dogbone may have several width-contract pieces.
                direct = next(((ts,v) for ts,v in choices if all(
                    point_on_segment(t.start,start,site) and point_on_segment(t.end,start,site) for t in ts)), None)
                if direct is None:
                    cache[key] = None
                else:
                    ts,v = direct
                    actual = v or next(v for v in board.vias if v.net==net and v.position==site
                                       and (v.from_layer,v.to_layer)==spans[ref][:2])
                    cache[key] = PatternContact(intent, net, ts, actual, v is None)
            contact = cache[key]
            if contact is None:
                blocked.append(ref)
            else:
                contacts.append(contact)
        collisions = []
        for a,b in combinations(contacts, 2):
            if (a.via.position == b.via.position or not conflicts.compatible(
                    (a.tracks, None if a.reused_via else a.via),
                    (b.tracks, None if b.reused_via else b.via))):
                collisions.append((a.intent.pad,b.intent.pad))
        proposal = PackagePattern(source, family, offset, tuple(contacts),tuple(sorted(_deferred_components)))
        failures = (['fabrication_minimum_track_width'] if any(
            t.width_nm < board.rules.minimum_track_width_nm for t in proposal.tracks) else [])
        for net in {c.net for c in contacts}:
            rule = rules.get(net)
            if rule is None:
                continue
            if rule.max_vias is not None and sum(v.net==net for v in (*board.vias,*proposal.new_vias)) > rule.max_vias:
                failures.append(net+':via_budget')
            tracks = tuple(dict.fromkeys((*board.tracks,*proposal.tracks)))
            if rule.max_length_nm is not None and sum(_length_nm(t) for t in tracks if t.net==net) > rule.max_length_nm:
                failures.append(net+':length_budget')
        return PatternTrial(family, offset, len(intents), tuple(blocked), tuple(collisions),
                            tuple(sorted(failures)), None if blocked or collisions or failures else proposal)
    def retain(trial):
        identity = tuple(endpoint(i, trial.family, trial.offset_nm) for i in intents)
        if identity in seen:
            return
        if len(trials) >= options.maximum_patterns:
            raise _BudgetExceeded('pattern_budget_exhausted')
        seen.add(identity); trials.append(trial)
        if trial.candidate is not None:
            candidates.append(trial.candidate)
    status = 'finite_pattern_pool_unresolved'
    emit(on_progress, 'package_patterns', 'started', component=component, pads=len(intents))
    try:
        for family in options.families:
            preferred=options.preferred_offset_nm if near<=options.preferred_offset_nm<=center else None
            for offset in dict.fromkeys((preferred,near,center) if preferred is not None else (near,center)):
                retain(evaluate(family,offset))
            if preferred is not None:
                # Revalidate the placement stage's common offset on this exact
                # source. It replaces, rather than expands, the frontier slot.
                continue
            offsets = set()
            for intent in intents:
                if frontier_queries >= options.maximum_frontier_queries:
                    raise _BudgetExceeded('frontier_budget_exhausted')
                frontier_queries += 1
                start = transformed_local_point(pose, pads[intent.pad.pad].position)
                seed = endpoint(intent, family, center)
                dx,dy = seed.x_nm-start.x_nm,seed.y_nm-start.y_nm
                for site in obstacle_boundary_sites(index, membership[intent.pad], start,
                        board.rules.default_via_size_nm, board.rules.default_via_drill_nm, spans[intent.pad][:2],
                        isqrt(2*center*center)+1, options.frontier_sites_per_pad, seeds=(seed,)):
                    offset = ((site.x_nm-start.x_nm)*dx+(site.y_nm-start.y_nm)*dy)*center//(dx*dx+dy*dy)
                    if near < offset < center and endpoint(intent,family,offset)==site:
                        offsets.add(offset)
            frontier = None
            for offset in sorted(offsets, key=lambda n:(abs(n-center),n))[:options.frontier_offsets_per_family]:
                trial = evaluate(family, offset)
                rank = lambda t:(len(t.blocked_pads),len(t.conflicts),len(t.rule_failures),t.offset_nm)
                if frontier is None or rank(trial) < rank(frontier):
                    frontier = trial
            if frontier is not None:
                retain(frontier)
            emit(on_progress, 'package_pattern_family', 'finished', family=family,
                 candidates=len(candidates), site_queries=site_queries, pair_queries=conflicts.queries)
        if candidates:
            status = 'prospective_contact_patterns' if _deferred_components else 'contact_geometry_proposals'
    except _BudgetExceeded as error:
        status = str(error)
    emit(on_progress, 'package_patterns', 'finished', component=component, status=status,
         candidates=len(candidates), site_queries=site_queries, evaluated_offsets=evaluated_offsets)
    return replace(result, status=status, candidates=tuple(candidates), trials=tuple(trials),
        diagnostics=tuple((i.pad,recorders[i.pad].finish(source,status)) for i in intents),
        site_queries=site_queries, pair_checks=conflicts.checks, pair_queries=conflicts.queries,
        frontier_queries=frontier_queries, evaluated_offsets=evaluated_offsets)
