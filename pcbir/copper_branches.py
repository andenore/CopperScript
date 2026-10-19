"""Bounded track replacement with retained physical junctions and plated objects.

This is a separate disposable transaction, not a partial CopperTrial. Physical
forks and terminal junctions are retained; ordinary corners may move. Vias, source copper and
unattributed geometry cannot be released. Native filled proof remains external.
"""
from dataclasses import dataclass, replace
from fractions import Fraction

from .copper_connectivity import CopperContact, copper_contacts_overlap
from .copper_ownership import (CopperOwnership, _requirement_closure,
    append_copper_ownership)
from .drc import _explicit_copper_contacts, physical_board_digest
from .geometry import RoundedConvexShape, SpatialIndex, SpatialItem, point_segment_distance_squared
from .physical import PhysicalBoard, Point, TrackSegment


@dataclass(frozen=True, slots=True)
class BranchSection:
    occurrences: tuple[str, ...]
    start: Point
    end: Point

    @property
    def occurrence(self):
        return self.occurrences[0]


@dataclass(frozen=True, slots=True)
class TrackBranchTrial:
    original: PhysicalBoard
    held: PhysicalBoard
    ownership: CopperOwnership
    released: tuple[str, ...]
    requirements: tuple[str, ...]
    sections: tuple[BranchSection, ...]
    boundaries: tuple[tuple[str, str, Point], ...]
    maximum_occurrences: int
    maximum_sections: int
    maximum_contact_pairs: int

    def matches(self):
        try:
            return self == prepare_track_branch_trial(self.original, self.ownership, self.released,
                maximum_occurrences=self.maximum_occurrences, maximum_sections=self.maximum_sections,
                maximum_contact_pairs=self.maximum_contact_pairs)
        except ValueError:
            return False


def branch_track_occurrences(ledger):
    """Search eligibility only; a complete fresh path query still chooses cuts."""
    if ledger.frozen:
        return set()
    uses = {u.id: u for u in ledger.uses}
    return {o.identity for o in ledger.occurrences if o.kind == 'track' and o.negotiable
            and all(uses[r].kind in {'contact', 'bypass'}
                    and (uses[r].kind!='contact' or len(uses[r].terminals)==1) for r in
                    _requirement_closure(ledger, o.requirements))}


def track_endpoint_guards(board, identities):
    """Conservative search reservations for end caps that cannot move.

    One-nanometre spines cover the original circular caps. They are index-only
    obstacles, never fabricated copper or authority to release an occurrence.
    """
    guards = {}
    incidence = {}
    for i,track in enumerate(board.tracks):
        if f'track:{i}' in identities:
            for point in (track.start,track.end):
                key=track.net,track.layer,track.width_nm,point
                incidence[key]=incidence.get(key,0)+1
    for i, track in enumerate(board.tracks):
        if f'track:{i}' not in identities:
            continue
        for point in (track.start,track.end):
            if incidence[track.net,track.layer,track.width_nm,point]==2:
                continue  # A replaceable degree-two corner is not a fixed port.
            key = track.net,track.layer,point
            old = guards.get(key)
            if old is None or old.width_nm < track.width_nm:
                guards[key] = replace(track,start=point,end=Point(point.x_nm+1,point.y_nm))
    return tuple(guards.values())


def _assemble_sections(raw, boundaries, objects, selected):
    """Follow degree-two chains, stopping at actual forks and held contacts."""
    incidence, endpoints = {}, {}
    for i,section in enumerate(raw):
        t=objects[section.occurrence].value
        nodes=tuple((t.net,t.layer,t.width_nm,p) for p in (section.start,section.end))
        endpoints[i]=nodes
        for node in nodes:
            incidence.setdefault(node,[]).append(i)
    fixed=set()
    for a,b,p in boundaries:
        t=objects[a].value
        if b not in selected or (objects[b].value.net,objects[b].value.layer,objects[b].value.width_nm)!=(t.net,t.layer,t.width_nm):
            fixed.add((t.net,t.layer,t.width_nm,p))
    anchors={node for node,edges in incidence.items() if len(edges)!=2 or node in fixed}
    visited,assembled=set(),[]
    order=lambda node:(node[0],node[1].value,node[2],node[3].x_nm,node[3].y_nm)
    for start in sorted(anchors,key=order):
        for first in incidence[start]:
            if first in visited:
                continue
            current,edge,owners=start,first,[]
            while True:
                visited.add(edge)
                owners.extend(raw[edge].occurrences)
                a,b=endpoints[edge]
                current=b if current==a else a
                if current in anchors:
                    break
                edge=next(e for e in incidence[current] if e!=edge)
                if edge in visited:
                    raise ValueError('closed track branch requires explicit loop intent')
            if current==start:
                raise ValueError('closed track branch requires explicit loop intent')
            assembled.append(BranchSection(tuple(dict.fromkeys(owners)),start[3],current[3]))
    if len(visited)!=len(raw):
        raise ValueError('closed track branch requires explicit loop intent')
    return tuple(assembled)


def _junction(track, contact):
    """Exact centreline junction; unsupported overlap never becomes an anchor."""
    a, b = track.start, track.end
    if contact.identity.startswith('track:'):
        c, d = contact.shape.spine
        dx, dy, ex, ey = b.x_nm-a.x_nm, b.y_nm-a.y_nm, d.x_nm-c.x_nm, d.y_nm-c.y_nm
        cross = dx*ey-dy*ex
        if cross:
            t = Fraction((c.x_nm-a.x_nm)*ey-(c.y_nm-a.y_nm)*ex, cross)
            u = Fraction((c.x_nm-a.x_nm)*dy-(c.y_nm-a.y_nm)*dx, cross)
            x, y = a.x_nm+t*dx, a.y_nm+t*dy
            if 0 <= t <= 1 and 0 <= u <= 1 and x.denominator == y.denominator == 1:
                return Point(int(x), int(y))
        else:
            shared = {p for p in (a, b, c, d)
                      if point_segment_distance_squared(p, a, b) == 0
                      and point_segment_distance_squared(p, c, d) == 0}
            if len(shared) == 1:
                return shared.pop()
        raise ValueError('unsupported branch track overlap')
    # An existing end cap can enter a pad or annulus away from its centre.
    # Retaining this endpoint and width retains that exact positive-area cap.
    for point in (a, b):
        cap = CopperContact('anchor',track.net,(track.layer,),RoundedConvexShape((point,),track.width_nm//2))
        if copper_contacts_overlap(cap,contact):
            return point
    points = contact.shape.spine
    x = Fraction(sum(p.x_nm for p in points), len(points))
    y = Fraction(sum(p.y_nm for p in points), len(points))
    if x.denominator == y.denominator == 1:
        p = Point(int(x), int(y))
        if point_segment_distance_squared(p, a, b) == 0:
            return p
    # Interior annulus/pad contact: split at an exact integral projection. A
    # rational-only junction remains unsupported; never round an authority port.
    dx,dy=b.x_nm-a.x_nm,b.y_nm-a.y_nm
    t=((x-a.x_nm)*dx+(y-a.y_nm)*dy)/(dx*dx+dy*dy)
    px,py=a.x_nm+t*dx,a.y_nm+t*dy
    if 0<=t<=1 and px.denominator==py.denominator==1:
        return Point(int(px),int(py))
    raise ValueError('unsupported branch contact anchor')


def _retained_track_contact(track, contact, anchors):
    """Recognize the finite copper neighborhood of an existing track junction.

    Fine-grid approach edges can enter a held endpoint's circular cap before
    reaching its centreline. Bound that contact along the held spine to one
    combined copper radius; it cannot become a walk along a shared trunk.
    """
    try:
        point=_junction(track,contact)
        if point in anchors:
            return point
    except ValueError:
        pass
    a,b=contact.shape.spine
    dx,dy=b.x_nm-a.x_nm,b.y_nm-a.y_nm
    length_squared=dx*dx+dy*dy
    radius=contact.shape.radius_nm+track.width_nm//2
    candidate=CopperContact('candidate',track.net,(track.layer,),
        RoundedConvexShape((track.start,track.end),track.width_nm//2))
    for anchor in anchors:
        if point_segment_distance_squared(anchor,a,b)!=0:
            continue
        if any(((p.x_nm-anchor.x_nm)*dx+(p.y_nm-anchor.y_nm)*dy)**2>
               radius*radius*length_squared for p in (track.start,track.end)):
            continue
        cap=replace(contact,shape=RoundedConvexShape((anchor,),contact.shape.radius_nm))
        if copper_contacts_overlap(candidate,cap):
            return anchor
    return None


class RetainedJunctionClearance:
    """A replacement may touch held same-net copper only at its old anchors.

    Ordinary clearance permits same-net shortcuts. That would let a repair
    walk along a shared trunk and only discover the lost topology at capture.
    """
    def __init__(self, base, net, anchors):
        self.base, self.net, self.anchors = base, net, frozenset(anchors)
        self.contacts, _, _ = _explicit_copper_contacts(base.board, only_nets=frozenset({net}),
                                                       include_internal_connections=False)
        self.by_id = {c.identity:c for c in self.contacts}
        self.index = SpatialIndex(SpatialItem(c.identity,c.shape.bounds) for c in self.contacts)

    def __getattr__(self, name):
        return getattr(self.base, name)

    def can_route(self, net, start, end, width, layer):
        return all(self.can_track(net,t.start,t.end,t.width_nm,layer)
                   for t in self.base.route_pieces(net,start,end,width,layer))

    def can_track(self, net, start, end, width, layer):
        if not self.base.can_track(net,start,end,width,layer):
            return False
        candidate = CopperContact('proposal',net,(layer,),RoundedConvexShape((start,end),width//2))
        track = TrackSegment(net,start,end,width,layer)
        for identity in self.index.query(candidate.shape.bounds):
            other = self.by_id[identity]
            if not copper_contacts_overlap(candidate,other):
                continue
            if not identity.startswith('track:'):
                old = TrackSegment(net,*sorted(self.anchors,key=lambda p:(p.x_nm,p.y_nm)),width,layer)
                try:
                    if _junction(old,other) in self.anchors:
                        continue  # Enter pad/annulus copper before the retained endpoint.
                except ValueError:
                    pass
                return False
            if _retained_track_contact(track,other,self.anchors) is None:
                return False
        return True


def prepare_track_branch_trial(board, ledger, occurrences, *, maximum_occurrences=64,
                               maximum_sections=128, maximum_contact_pairs=50000):
    """Cut exact generated tracks while retaining all their physical junctions."""
    if any(type(v) is not int or v <= 0 for v in
           (maximum_occurrences, maximum_sections, maximum_contact_pairs)):
        raise ValueError('positive track branch bounds required')
    if ledger.frozen or not ledger.matches(board):
        raise ValueError('fresh disposable branch ownership required')
    selected = set(occurrences)
    if not selected or len(selected) > maximum_occurrences:
        raise ValueError('track branch occurrence budget exhausted')
    if not selected <= branch_track_occurrences(ledger):
        raise ValueError('protected, ambiguous, unsupported or plated branch blocker')
    objects = {o.identity: o for o in ledger.occurrences}
    nets = frozenset(objects[k].value.net for k in selected)
    contacts, _, _ = _explicit_copper_contacts(board, only_nets=nets, include_internal_connections=False)
    by_id = {c.identity: c for c in contacts}
    index = SpatialIndex(SpatialItem(c.identity, c.shape.bounds) for c in contacts)
    sections, boundaries, pairs = [], [], 0
    for identity in sorted(selected):
        track = objects[identity].value
        current = by_id[identity]
        points = {track.start, track.end}
        for other_id in sorted(index.query(current.shape.bounds)):
            if other_id == identity:
                continue
            pairs += 1
            if pairs > maximum_contact_pairs:
                raise ValueError('track branch contact budget exhausted')
            other = by_id[other_id]
            if not copper_contacts_overlap(current, other):
                continue
            try:
                point = _junction(track, other)
            except ValueError as error:
                raise ValueError(f'{error}: {identity} -> {other_id}') from error
            points.add(point)
            boundaries.append((identity, other_id, point))
        points = sorted(points, key=lambda p: (p.x_nm-track.start.x_nm)**2+(p.y_nm-track.start.y_nm)**2)
        sections.extend(BranchSection((identity,), a, b) for a, b in zip(points, points[1:]))
        if len(sections) > maximum_sections:
            raise ValueError('track branch section budget exhausted')
    sections=_assemble_sections(sections,boundaries,objects,selected)
    requirements = _requirement_closure(ledger, {r for k in selected for r in objects[k].requirements})
    held = replace(board, tracks=tuple(t for i, t in enumerate(board.tracks) if f'track:{i}' not in selected))
    return TrackBranchTrial(board, held, ledger, tuple(sorted(selected)), tuple(sorted(requirements)),
        tuple(sections), tuple(sorted(boundaries,key=lambda b:(b[0],b[1],b[2].x_nm,b[2].y_nm))),
        maximum_occurrences, maximum_sections, maximum_contact_pairs)


def rebuild_track_branch_ownership(frame, paths, *, producer='track-branch-replacement'):
    """Replace each old section once, retaining widths, layers and obligations.

    This checks structural authority, not route clearance or native acceptance.
    The caller must freshly prove all affected requirements before proposing it.
    """
    if not frame.matches():
        raise ValueError('fresh exact track branch frame required')
    paths = tuple(tuple(p) for p in paths)
    if len(paths) != len(frame.sections) or any(not p for p in paths):
        raise ValueError('complete branch section replacements required')
    objects = {o.identity: o for o in frame.ownership.occurrences}
    replacements = {k: [] for k in frame.released}
    for section, path in zip(frame.sections, paths):
        original = objects[section.occurrence].value
        if (path[0].start != section.start or path[-1].end != section.end
                or any(a.end != b.start for a, b in zip(path, path[1:]))
                or any((t.net, t.layer, t.width_nm) != (original.net, original.layer, original.width_nm)
                       for t in path)):
            raise ValueError('branch replacement changed an anchor, layer or width')
        for identity in section.occurrences:
            replacements[identity].extend(path)
    # No new same-net shortcut may bypass an original junction or erase a
    # deliberately parallel path. Require every new contact to correspond to
    # an old contact at the same retained junction.
    additions = tuple(t for path in paths for t in path)
    after = replace(frame.held, tracks=(*frame.held.tracks, *additions))
    contacts, _, _ = _explicit_copper_contacts(after, only_nets=frozenset(t.net for t in additions),
                                              include_internal_connections=False)
    by_id = {c.identity: c for c in contacts}
    index = SpatialIndex(SpatialItem(c.identity, c.shape.bounds) for c in contacts)
    old_retained = [o for o in frame.ownership.occurrences if o.kind == 'track' and o.identity not in replacements]
    names = {f'track:{i}': o.identity for i, o in enumerate(old_retained)}
    owners = {}
    cursor = len(frame.held.tracks)
    for section_index,path in enumerate(paths):
        for t in path:
            owners[f'track:{cursor}'] = section_index
            cursor += 1
    allowed = {(i,b,p) for i,section in enumerate(frame.sections) for a,b,p in frame.boundaries
               if a in section.occurrences and b not in replacements and p in (section.start,section.end)}
    work, observed_contacts = 0, set()
    for new_id, owner in owners.items():
        current = by_id[new_id]
        for other_id in index.query(current.shape.bounds):
            if other_id == new_id:
                continue
            work += 1
            if work > frame.maximum_contact_pairs:
                raise ValueError('track branch replacement contact budget exhausted')
            if not copper_contacts_overlap(current, by_id[other_id]):
                continue
            if owners.get(other_id) == owner:
                # Consecutive new sections of the same original conductor may
                # meet. Nonlocal overlap would collapse a replacement loop.
                left = after.tracks[int(new_id.split(':')[1])]
                right = after.tracks[int(other_id.split(':')[1])]
                if (abs(int(new_id.split(':')[1])-int(other_id.split(':')[1])) != 1
                        or _junction(left, by_id[other_id]) not in {left.start, left.end}.intersection((right.start, right.end))):
                    raise ValueError('branch replacement self contact')
                continue
            if other_id in owners:
                left,right=frame.sections[owner],frame.sections[owners[other_id]]
                point=_junction(after.tracks[int(new_id.split(':')[1])],by_id[other_id])
                if point not in {left.start,left.end}.intersection((right.start,right.end)):
                    raise ValueError('branch replacement introduced an unplanned same-net contact')
                continue
            old_other = names.get(other_id,other_id)
            if not other_id.startswith('track:'):
                if any(a==owner and b==old_other for a,b,p in allowed):
                    observed_contacts.update((a,b,p) for a,b,p in allowed if a==owner and b==old_other)
                    continue
                raise ValueError('branch replacement introduced an unplanned same-net contact')
            anchors={p for a,b,p in allowed if a==owner and b==old_other}
            point = _retained_track_contact(after.tracks[int(new_id.split(':')[1])],by_id[other_id],anchors)
            if (owner, old_other, point) not in allowed:
                raise ValueError('branch replacement introduced an unplanned same-net contact')
            observed_contacts.add((owner,old_other,point))
    if allowed-observed_contacts:
        raise ValueError('branch replacement lost an original physical contact')
    replacement_values = {objects[k].value: tuple(v) for k, v in replacements.items()}
    uses = tuple(replace(u, tracks=tuple(dict.fromkeys(t for old in u.tracks for t in replacement_values.get(old, (old,)))))
                 for u in frame.ownership.uses)
    retained, number = [], 0
    for o in frame.ownership.occurrences:
        if o.identity in replacements:
            continue
        retained.append(replace(o, index=number) if o.kind == 'track' else o)
        if o.kind == 'track':
            number += 1
    ledger = CopperOwnership(frame.ownership.source_digest, physical_board_digest(frame.held), tuple(retained))
    return after, append_copper_ownership(ledger, frame.held, after, producer, uses=uses, trial_mutable=True)
