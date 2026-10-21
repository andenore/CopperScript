"""Snapshot-bound occurrence ownership for disposable contact preparations.

This records producer evidence, not electrical qualification. Unattributed or
ambiguous copper stays held. An accepted/frozen ledger cannot release copper.
"""
from dataclasses import dataclass, fields, replace

from .drc import physical_board_digest
from .physical import PadReference, PhysicalBoard, TrackSegment, Via


@dataclass(frozen=True, slots=True)
class CopperUse:
    id: str
    kind: str
    terminals: tuple[PadReference, ...]
    tracks: tuple[TrackSegment, ...] = ()
    vias: tuple[Via, ...] = ()
    dependencies: tuple[str, ...] = ()

    def __post_init__(self):
        if not self.id or not self.kind or not self.terminals:
            raise ValueError('copper use requires an identity, kind and terminals')
        for name in ('terminals', 'tracks', 'vias', 'dependencies'):
            object.__setattr__(self, name, tuple(getattr(self, name)))


@dataclass(frozen=True, slots=True)
class CopperOccurrence:
    kind: str
    index: int
    value: TrackSegment | Via
    producer: str
    trial_mutable: bool = False
    requirements: tuple[str, ...] = ()
    ambiguous: bool = False

    @property
    def identity(self):
        return f'{self.kind}:{self.index}'

    @property
    def negotiable(self):
        return self.trial_mutable and bool(self.requirements) and not self.ambiguous


@dataclass(frozen=True, slots=True)
class CopperOwnership:
    source_digest: str
    board_digest: str
    occurrences: tuple[CopperOccurrence, ...]
    uses: tuple[CopperUse, ...] = ()
    frozen: bool = False

    def matches(self, board):
        expected = {(kind, i): value for kind, values in (('track', board.tracks), ('via', board.vias))
                    for i, value in enumerate(values)}
        observed = {(o.kind, o.index): o.value for o in self.occurrences}
        return (self.board_digest == physical_board_digest(board)
                and len(observed) == len(self.occurrences) and observed == expected)

    def report(self):
        return dict(source_digest=self.source_digest, board_digest=self.board_digest, frozen=self.frozen,
            occurrences=[dict(identity=o.identity, producer=o.producer, net=o.value.net,
                negotiable=o.negotiable and not self.frozen, requirements=o.requirements,
                ambiguous=o.ambiguous) for o in self.occurrences],
            uses=[dict(id=u.id, kind=u.kind, terminals=[str(p) for p in u.terminals],
                       dependencies=u.dependencies) for u in self.uses])


def begin_copper_ownership(board: PhysicalBoard) -> CopperOwnership:
    digest = physical_board_digest(board)
    return CopperOwnership(digest, digest, tuple(CopperOccurrence(kind, i, value, 'held-source')
        for kind, values in (('track', board.tracks), ('via', board.vias)) for i, value in enumerate(values)))


def record_copper_uses(ledger, board, uses):
    if ledger.frozen or not ledger.matches(board):
        raise ValueError('fresh disposable copper ownership required')
    collected = {u.id: u for u in ledger.uses}
    for use in uses:
        if any(p not in {p for n in board.nets for p in n.pads} for p in use.terminals):
            raise ValueError('copper use names an unknown terminal')
        if any(t not in board.tracks for t in use.tracks) or any(v not in board.vias for v in use.vias):
            raise ValueError('copper use names absent physical copper')
        old = collected.get(use.id)
        if old and (old.kind != use.kind or old.terminals != use.terminals):
            raise ValueError('copper use identity changed its source obligation')
        collected[use.id] = replace(use,
            tracks=tuple(dict.fromkeys((*old.tracks, *use.tracks))) if old else use.tracks,
            vias=tuple(dict.fromkeys((*old.vias, *use.vias))) if old else use.vias,
            dependencies=tuple(sorted(set(old.dependencies) | set(use.dependencies))) if old else use.dependencies)
    if any(d not in collected for u in collected.values() for d in u.dependencies):
        raise ValueError('copper use has an unknown dependency')
    uses = tuple(collected[key] for key in sorted(collected))
    counts = {}
    for occurrence in ledger.occurrences:
        counts[occurrence.kind, occurrence.value] = counts.get((occurrence.kind, occurrence.value), 0) + 1
    occurrences = tuple(replace(o,
        requirements=tuple(u.id for u in uses if o.value in (u.tracks if o.kind == 'track' else u.vias)),
        ambiguous=counts[o.kind, o.value] > 1) for o in ledger.occurrences)
    return replace(ledger, occurrences=occurrences, uses=uses)


def prospective_contact_uses(board, terminals, *, maximum_states=10_000):
    """Exact bare-copper witness dependencies reaching prospective zone vias.

    Shared positive-area contact predicates define every edge. A bounded BFS
    records one actual witness, not every branch of a same-net component.
    Zone outlines remain search targets, never native fill evidence. Unknown
    or multi-land terminals without one physical witness stay unattributed.
    """
    from collections import defaultdict, deque
    from .drc import _explicit_copper_contacts
    from .copper_connectivity import copper_contacts_overlap
    from .geometry import SpatialIndex, SpatialItem
    from .plane import _via_contacts_zone
    from .placement import resolved_copper_keepouts
    if type(maximum_states) is not int or maximum_states <= 0:
        raise ValueError('positive contact witness state bound required')
    membership = {p: n.name for n in board.nets for p in n.pads}
    terminals = tuple(sorted(set(terminals)))
    contacts, pads, _ = _explicit_copper_contacts(board,
        only_nets=frozenset(membership[p] for p in terminals if p in membership),
        include_internal_connections=False)
    by_id = {c.identity: c for c in contacts}
    spatial = SpatialIndex(SpatialItem(c.identity, c.shape.bounds) for c in contacts)
    adjacency = defaultdict(list)
    for c in contacts:
        for identity in spatial.query(c.shape.bounds):
            if identity > c.identity and copper_contacts_overlap(c, by_id[identity]):
                adjacency[c.identity].append(identity)
                adjacency[identity].append(c.identity)
    keepouts = resolved_copper_keepouts(board)
    goals = set()
    for i, via in enumerate(board.vias):
        a, b = sorted((board.stackup.copper_layers.index(via.from_layer),
                       board.stackup.copper_layers.index(via.to_layer)))
        span = set(board.stackup.copper_layers[a:b+1])
        if any(z.net == via.net and _via_contacts_zone(via.position, z, span, keepouts) for z in board.zones):
            goals.add(f'via:{i}')
    uses = []
    for pad in terminals:
        nodes = pads.get(pad, ())
        if len(nodes) != 1 or pad not in membership:
            continue
        parents = {nodes[0]: None}
        queue = deque(nodes)
        while queue and len(parents) <= maximum_states:
            node = queue.popleft()
            if node in goals:
                witness = []
                while node is not None:
                    witness.append(node)
                    node = parents[node]
                tracks = tuple(board.tracks[int(n.split(':')[1])] for n in reversed(witness) if n.startswith('track:'))
                vias = tuple(board.vias[int(n.split(':')[1])] for n in reversed(witness) if n.startswith('via:'))
                net = membership[pad]
                uses.append(CopperUse(f'contact:{net}:{pad.component}.{pad.pad}', 'contact', (pad,), tracks, vias))
                break
            for neighbor in sorted(adjacency[node]):
                if neighbor not in parents:
                    if len(parents) >= maximum_states:
                        break
                    parents[neighbor] = node
                    queue.append(neighbor)
    return tuple(uses)


def append_copper_ownership(ledger, before, after, producer, *, uses=(), trial_mutable=False,
                            permitted_fields=frozenset({'metadata'})):
    if ledger.frozen or not ledger.matches(before):
        raise ValueError('fresh disposable copper ownership required')
    if not producer:
        raise ValueError('copper producer identity required')
    if (after.tracks[:len(before.tracks)] != before.tracks or after.vias[:len(before.vias)] != before.vias
            or len(after.tracks) < len(before.tracks) or len(after.vias) < len(before.vias)):
        raise ValueError('copper capture requires an unchanged ordered prefix')
    if any(getattr(before, f.name) != getattr(after, f.name) for f in fields(before)
           if f.name not in {'tracks', 'vias'} | set(permitted_fields)):
        raise ValueError('copper capture changed held physical intent')
    additions = tuple(CopperOccurrence(kind, i, value, producer, trial_mutable)
        for kind, old, new in (('track', before.tracks, after.tracks), ('via', before.vias, after.vias))
        for i, value in enumerate(new) if i >= len(old))
    observed = replace(ledger, board_digest=physical_board_digest(after),
                       occurrences=(*ledger.occurrences, *additions))
    return record_copper_uses(observed, after, uses)


@dataclass(frozen=True, slots=True)
class CopperTrial:
    original: PhysicalBoard
    held: PhysicalBoard
    ownership: CopperOwnership
    released: tuple[str, ...]
    requirements: tuple[str, ...]

    def matches(self):
        if self.ownership.frozen or not self.ownership.matches(self.original):
            return False
        removed = set(self.released)
        objects = {o.identity: o for o in self.ownership.occurrences}
        if not removed <= objects.keys() or any(not objects[k].negotiable for k in removed):
            return False
        requirements = _requirement_closure(self.ownership, {r for k in removed for r in objects[k].requirements})
        if requirements != set(self.requirements):
            return False
        if any(o.trial_mutable and requirements.intersection(o.requirements)
               and o.identity not in removed for o in objects.values()):
            return False
        expected = replace(self.original,
            tracks=tuple(t for i, t in enumerate(self.original.tracks) if f'track:{i}' not in removed),
            vias=tuple(v for i, v in enumerate(self.original.vias) if f'via:{i}' not in removed))
        return expected == self.held


def _requirement_closure(ledger, selected):
    selected = set(selected)
    while True:
        expanded = selected | {d for u in ledger.uses if u.id in selected for d in u.dependencies}
        expanded.update(u.id for u in ledger.uses if selected.intersection(u.dependencies))
        if expanded == selected:
            return selected
        selected = expanded


def prepare_copper_trial(board, ledger, occurrences, *, maximum_occurrences=64):
    """Dependency closure on exact generated occurrences; no in-place mutation."""
    if type(maximum_occurrences) is not int or maximum_occurrences <= 0:
        raise ValueError('positive ownership closure bound required')
    if ledger.frozen or not ledger.matches(board):
        raise ValueError('fresh disposable copper ownership required')
    objects = {o.identity: o for o in ledger.occurrences}
    selected = set(occurrences)
    if not selected or not selected <= objects.keys():
        raise ValueError('known exact copper occurrences required')
    while True:
        if len(selected) > maximum_occurrences:
            raise ValueError('ownership dependency closure budget exhausted')
        if any(not objects[key].negotiable for key in selected):
            raise ValueError('protected, ambiguous or unattributed copper cannot be released')
        required = _requirement_closure(ledger, {r for key in selected for r in objects[key].requirements})
        expanded = selected | {o.identity for o in objects.values()
                               if o.trial_mutable and required.intersection(o.requirements)}
        if expanded == selected:
            break
        selected = expanded
    held = replace(board,
        tracks=tuple(t for i, t in enumerate(board.tracks) if f'track:{i}' not in selected),
        vias=tuple(v for i, v in enumerate(board.vias) if f'via:{i}' not in selected))
    return CopperTrial(board, held, ledger, tuple(sorted(selected)), tuple(sorted(required)))


def rebuild_copper_ownership(frame, after, producer, *, uses):
    """Rebase one exact private replacement, retaining every outside producer."""
    if not frame.matches():
        raise ValueError('fresh complete disposable ownership frame required')
    selected = set(frame.released)
    counters = {'track': 0, 'via': 0}
    retained = []
    for occurrence in frame.ownership.occurrences:
        if occurrence.identity in selected:
            continue
        retained.append(replace(occurrence, index=counters[occurrence.kind]))
        counters[occurrence.kind] += 1
    kept_uses = tuple(u for u in frame.ownership.uses if u.id not in frame.requirements)
    observed = CopperOwnership(frame.ownership.source_digest, physical_board_digest(frame.held),
                               tuple(retained), kept_uses)
    supplied = tuple(uses)
    if {u.id for u in supplied} != set(frame.requirements):
        raise ValueError('replacement must retain every released source obligation')
    old = {u.id: u for u in frame.ownership.uses}
    if any(u.kind != old[u.id].kind or u.terminals != old[u.id].terminals for u in supplied):
        raise ValueError('replacement changed a source obligation')
    return append_copper_ownership(observed, frame.held, after, producer,
        uses=supplied, trial_mutable=True)
