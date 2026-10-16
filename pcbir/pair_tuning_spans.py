"""Virtual candidate-only spans for a failed legacy pair-tuning attempt.

No independent search: offer the same bounded tuner the same copper union with
only artificial straight segmentation removed. Commit only a checked success.
"""
from collections import Counter
from dataclasses import replace
from math import hypot
from typing import Callable

from .breakout import BreakoutRegions
from .copper_connectivity import CopperContact, copper_contacts_overlap
from .drc import _explicit_copper_contacts
from .geometry import RoundedConvexShape, point_on_segment, segment_distance_squared
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment, Via
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board

Node = tuple[str, CopperLayer, Point]


def _length(tracks):
    return sum(round(hypot(t.end.x_nm-t.start.x_nm, t.end.y_nm-t.start.y_nm)) for t in tracks)


def _overlap(a, b):
    """Positive collinear centreline overlap, independent of object identity."""
    if a.net != b.net or a.layer is not b.layer:
        return False
    dx, dy = a.end.x_nm-a.start.x_nm, a.end.y_nm-a.start.y_nm
    if not (dx or dy) or any(dx*(p.y_nm-a.start.y_nm)-dy*(p.x_nm-a.start.x_nm)
                             for p in (b.start,b.end)):
        return False
    axis = (lambda p:p.x_nm) if dx else (lambda p:p.y_nm)
    return max(min(axis(a.start),axis(a.end)),min(axis(b.start),axis(b.end))) < min(
        max(axis(a.start),axis(a.end)),max(axis(b.start),axis(b.end)))


def tune_virtual_pair_spans(
    first: list[TrackSegment], second: list[TrackSegment], max_skew_nm: int,
    amplitude_limit_nm: int, margin_nm: int, *, source: PhysicalBoard,
    held: PhysicalBoard, protected_nodes: tuple[Node, ...] = (),
    proposed_vias: tuple[Via, ...] = (), tuner: Callable,
) -> int:
    """Fallback after legacy returns zero; unsupported/blocked keeps exact lists.

    ``held`` is the entire immutable prefix, not merely ``source`` or a net
    filter. Explicit access/contact nodes belong in ``protected_nodes``.
    Source intent/prefix, overlap, physical pad contacts and via nodes are also
    recovered here. Existing outer candidate gates remain authoritative.
    """
    if (not first or not second or amplitude_limit_nm <= 0
            or abs(_length(first)-_length(second)) <= max_skew_nm):
        return 0
    if (replace(held,tracks=source.tracks,vias=source.vias) != source
            or len(held.tracks) < len(source.tracks) or len(held.vias) < len(source.vias)
            or any(a is not b for a,b in zip(source.tracks,held.tracks))
            or any(a is not b for a,b in zip(source.vias,held.vias))):
        return 0
    from .hard_macros import validate_hard_macros
    from .pad_via_arrays import require_via_in_pad_arrays
    try:
        validate_hard_macros(held)
        require_via_in_pad_arrays(held,"pair tuning spans")
    except ValueError:
        return 0
    originals = (tuple(first),tuple(second))
    target_index = 0 if _length(first) < _length(second) else 1
    target, partner = originals[target_index], originals[1-target_index]
    names = {t.net for t in target}
    if len(names) != 1 or any(t.start == t.end for t in target):
        return 0
    # Conservatively decline branches, overlaps or interior intersections.
    # They need an explicit graph owner, not list-index coalescing.
    degree = Counter((t.net,t.layer,p) for t in target for p in (t.start,t.end))
    if any(n > 2 for n in degree.values()):
        return 0
    for i,a in enumerate(target):
        for b in target[i+1:]:
            if a.layer is b.layer and segment_distance_squared(a.start,a.end,b.start,b.end) == 0:
                if _overlap(a,b) or not {a.start,a.end}.intersection((b.start,b.end)):
                    return 0
    protected = set(protected_nodes)
    layers = held.stackup.copper_layers
    for via in (*held.vias,*proposed_vias):
        lo,hi = sorted((layers.index(via.from_layer),layers.index(via.to_layer)))
        protected.update((via.net,layer,via.position) for layer in layers[lo:hi+1])
    for edge in held.tracks:
        protected.update((edge.net,edge.layer,p) for p in (edge.start,edge.end))
    # Interior contacts to actual physical lands cannot be silently erased.
    # This fallback supports endpoint-pad contacts; other contact graphs decline.
    contacts,_,_ = _explicit_copper_contacts(source,only_nets=frozenset(names),
                                            include_internal_connections=False)
    terminal_nodes = {node for node,n in degree.items() if n == 1}
    assigned = {p:net.name for net in source.nets for p in net.pads if net.name in names}
    from .physical import BoardSide, PadReference
    poses = {p.reference:p for p in source.placements}
    pad_centres = {}
    for pose in poses.values():
        for index,pad in enumerate(source.footprints[pose.footprint].pads):
            net = assigned.get(PadReference(pose.reference,pad.number))
            if net is not None:
                layer = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
                pad_centres[f"pad:{pose.reference}.{pad.number}:{index}"] = (
                    net,layer,transformed_local_point(pose,pad.position))
    for contact in contacts:
        if not contact.identity.startswith("pad:"):
            continue
        if any(copper_contacts_overlap(contact,CopperContact("candidate",t.net,(t.layer,),
                    RoundedConvexShape((t.start,t.end),t.width_nm//2))) for t in target):
            node = pad_centres.get(contact.identity)
            if node not in terminal_nodes:
                return 0
            protected.add(node)
    for t in target:
        for h in held.tracks:
            if (t.net != h.net or t.layer is not h.layer
                    or segment_distance_squared(t.start,t.end,h.start,h.end) != 0
                    or _overlap(t,h)):
                continue
            contacts = {p for p in (t.start,t.end,h.start,h.end)
                        if point_on_segment(p,t.start,t.end) and point_on_segment(p,h.start,h.end)}
            if not contacts:
                return 0  # Unmodelled interior crossing/contact; never erase it.
            protected.update((t.net,t.layer,p) for p in contacts)
    locked = {i for i,t in enumerate(target) if any(_overlap(t,h) for h in held.tracks)
              or any(net == t.net and layer is t.layer and point_on_segment(p,t.start,t.end)
                     and p not in (t.start,t.end) for net,layer,p in protected)}
    # A held branch may contact by finite copper width even when its centreline
    # misses every candidate node. Such contact roles are not disposable.
    # Filled-region contacts need a separate exact native-fill proof; decline.
    if held.zone_fills:
        return 0
    held_contacts,_,_ = _explicit_copper_contacts(
        held,only_nets=frozenset(names),include_internal_connections=False)
    if any(not c.identity.startswith(("pad:","track:","via:","polygon:"))
           for c in held_contacts):
        return 0
    for prefix,items in (("track",held.tracks),("via",held.vias)):
        expected = {f"{prefix}:{i}" for i,item in enumerate(items) if item.net in names}
        actual = [c.identity for c in held_contacts if c.identity.startswith(prefix+":")]
        if len(actual) != len(expected) or set(actual) != expected:
            return 0  # No authoritative occurrence-to-contact mapping.
    copper = tuple(c for c in held_contacts if not c.identity.startswith("pad:"))
    for i,t in enumerate(target):
        candidate = CopperContact(f"candidate:{i}",t.net,(t.layer,),
                                  RoundedConvexShape((t.start,t.end),t.width_nm//2))
        if any(copper_contacts_overlap(candidate,c) for c in copper):
            locked.add(i)  # Original list occurrence; never a copied source owner.
    regions = BreakoutRegions(source)
    def token(t):
        return regions.region(t.net,(t.start,t.end))
    groups = []
    for i,t in enumerate(target):
        if groups:
            members,span = groups[-1]
            join = (t.net,t.layer,t.start)
            forward = ((span.end.x_nm-span.start.x_nm)*(t.end.x_nm-t.start.x_nm)
                       + (span.end.y_nm-span.start.y_nm)*(t.end.y_nm-t.start.y_nm)) > 0
            if (i not in locked and not any(j in locked for j,_ in members)
                    and span.net == t.net and span.layer is t.layer and span.width_nm == t.width_nm
                    and span.end == t.start and join not in protected and degree[join] == 2
                    and forward and (span.start.x_nm == span.end.x_nm == t.end.x_nm
                                     or span.start.y_nm == span.end.y_nm == t.end.y_nm)):
                # Construct only after proving a nonzero forward continuation.
                # Candidate list order need not follow electrical path order.
                merged = replace(span,end=t.end)
                if (token(span) == token(t) == token(merged)
                        and regions.split_tracks((merged,),merged.width_nm) == (merged,)):
                    groups[-1] = ((*members,(i,t)),merged)
                    continue
        groups.append((((i,t),),t))
    if not any(len(members)>1 for members,_ in groups):
        return 0
    virtual = [span for _,span in groups]
    recovery = {id(span):tuple(t for _,t in members) for members,span in groups}
    work = [list(originals[0]),list(originals[1])]
    work[target_index] = virtual
    added = tuner(*work,max_skew_nm,amplitude_limit_nm,margin_nm)
    if added <= 0 or tuple(work[1-target_index]) != partner:
        return 0
    proposed = tuple(t for edge in work[target_index] for t in recovery.get(id(edge),(edge,)))
    # Recut only new bump geometry; retained occurrences retain their identity.
    old_ids = {id(t) for t in target}
    rule = next((r for r in source.net_routing_rules if r.net in names),None)
    width = rule.width_nm if rule and rule.width_nm else source.rules.default_track_width_nm
    proposed = tuple(piece for t in proposed for piece in (
        (t,) if id(t) in old_ids else regions.split_tracks((t,),width)))
    if (any(not any(t is target[i] for t in proposed) for i in locked)
            or abs(_length(proposed)-_length(partner)) > max_skew_nm):
        return 0
    required = {node for node in protected|terminal_nodes if any(
        node[0] == t.net and node[1] is t.layer and point_on_segment(node[2],t.start,t.end)
        for t in target)}
    if any(not any(net == t.net and layer is t.layer and point_on_segment(p,t.start,t.end)
                   for t in proposed) for net,layer,p in required):
        return 0
    try:
        combined = replace(held,tracks=(*held.tracks,*partner,*proposed),
                            vias=(*held.vias,*proposed_vias))
        index = RoutingClearanceIndex(combined)
    except ValueError:
        return 0
    if any(not _track_inside_board(combined,t.start,t.end,t.width_nm)
           or not index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in proposed):
        return 0
    work = [list(originals[0]),list(originals[1])]
    work[target_index] = list(proposed)
    first[:],second[:] = work
    return _length(proposed)-_length(target)
