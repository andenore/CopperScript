"""Resolve named board frames and source-owned attachments, without electrical edits."""
from dataclasses import dataclass, replace
from decimal import Decimal, localcontext, ROUND_HALF_EVEN

from .physical import BoardDatum, BoardEdge, BoardSide, Point, PhysicalAttachment, PadKind


@dataclass(frozen=True, slots=True)
class MechanicalAttachment:
    id: str
    reference: str
    target: str | None
    position: Point
    offset: Point
    anchor: str
    anchor_pad: str | None
    anchor_point: Point | None
    rotation: Decimal
    side: BoardSide
    location: object


def resolve_datums(items, point):
    nodes = {item.name: item for item in items}
    result, visiting = {}, []
    def resolve(name):
        if name in result:
            return result[name]
        if name not in nodes:
            raise ValueError(f"unknown datum {name!r}")
        if name in visiting:
            raise ValueError("cyclic datum dependency: " + " -> ".join((*visiting, name)))
        visiting.append(name)
        p = nodes[name].parameters
        if set(p) - {"position", "relative_to", "offset"} or (("position" in p) == ("relative_to" in p)):
            raise ValueError("datum requires exactly position or relative_to/offset")
        if "position" in p:
            if "offset" in p:
                raise ValueError("absolute datum cannot also have an offset")
            datum = BoardDatum(name, point(p["position"]))
        else:
            if not isinstance(p["relative_to"], str) or "offset" not in p:
                raise ValueError("relative datum requires a named parent and offset")
            parent, offset = resolve(p["relative_to"]), point(p["offset"])
            datum = BoardDatum(name, Point(parent.position.x_nm+offset.x_nm, parent.position.y_nm+offset.y_nm),
                               parent.id, offset)
        visiting.pop()
        result[name] = datum
        return datum
    return tuple(resolve(name) for name in sorted(nodes))


def resolve_edges(items, outline, point):
    if items and outline.circular_boundary:
        raise ValueError("straight named edges require a polygon outline; circular arc edges are not sampled")
    from .mechanical import boundary_line_pairs
    pairs = boundary_line_pairs(outline)
    result = []
    for item in items:
        if set(item.parameters) != {"start", "end"}:
            raise ValueError("named edge requires exactly start and end")
        edge = BoardEdge(item.name, point(item.parameters["start"]), point(item.parameters["end"]))
        if (edge.start, edge.end) not in pairs and (edge.end, edge.start) not in pairs:
            raise ValueError(f"named edge {edge.id!r} is not an actual outer boundary segment")
        result.append(edge)
    return tuple(result)


def resolve_attachments(items, datums, edges, outline, point):
    datums, edges = {d.id:d for d in datums}, {e.id:e for e in edges}
    from .mechanical import boundary_line_pairs,ring_edges
    pairs = boundary_line_pairs(outline)
    winding = 1 if sum(a.x_nm*b.y_nm-b.x_nm*a.y_nm for a,b in ring_edges(outline.vertices)) > 0 else -1
    result, references = [], set()
    for item in items:
        p = item.parameters
        allowed = {"component", "target", "position", "offset", "anchor", "anchor_pad", "anchor_point", "rotation", "side"}
        if set(p)-allowed or {"component", "rotation", "side"}-set(p) or (("target" in p) == ("position" in p)):
            raise ValueError("attachment requires component, rotation, side and exactly target or position")
        reference = p["component"]
        if not isinstance(reference, str) or not reference or reference in references:
            raise ValueError("attachment component must be unique and explicitly named")
        references.add(reference)
        offset = point(p["offset"]) if "offset" in p else Point(0,0)
        target = p.get("target")
        if target is None:
            if "offset" in p:
                raise ValueError("numeric attachment uses position directly, not an additional offset")
            position = point(p["position"])
        elif target in datums:
            origin = datums[target].position
            position = Point(origin.x_nm+offset.x_nm, origin.y_nm+offset.y_nm)
        elif target in edges:
            edge = edges[target]
            dx, dy = edge.end.x_nm-edge.start.x_nm, edge.end.y_nm-edge.start.y_nm
            inward = winding * (1 if (edge.start, edge.end) in pairs else -1)
            with localcontext() as context:
                context.prec = 60
                length = Decimal(dx*dx+dy*dy).sqrt()
                if not 0 <= offset.x_nm <= length:
                    raise ValueError("edge attachment along-distance lies outside the segment")
                x = Decimal(edge.start.x_nm)+(Decimal(dx*offset.x_nm-dy*offset.y_nm*inward)/length)
                y = Decimal(edge.start.y_nm)+(Decimal(dy*offset.x_nm+dx*offset.y_nm*inward)/length)
                position = Point(int(x.to_integral_value(rounding=ROUND_HALF_EVEN)), int(y.to_integral_value(rounding=ROUND_HALF_EVEN)))
        else:
            raise ValueError(f"unknown attachment target {target!r}")
        anchor = p.get("anchor", "origin")
        if anchor not in {"origin", "pad", "mating_face"}:
            raise ValueError("anchor must be origin, pad or mating_face")
        if (anchor == "pad") != ("anchor_pad" in p) or (anchor == "mating_face") != ("anchor_point" in p):
            raise ValueError("pad/mating_face anchors require exactly their corresponding anchor_pad/anchor_point")
        pad = str(p["anchor_pad"]) if "anchor_pad" in p else None
        if pad is not None and (isinstance(p["anchor_pad"], bool) or not isinstance(p["anchor_pad"], (str,int)) or not pad):
            raise ValueError("anchor_pad must identify a physical pad")
        if type(p["rotation"]) not in {int,float} or not Decimal(str(p["rotation"])).is_finite():
            raise ValueError("attachment rotation requires finite numeric degrees")
        result.append(MechanicalAttachment(item.name, reference, target, position, offset, anchor, pad,
            point(p["anchor_point"]) if "anchor_point" in p else None,
            (Decimal(str(p["rotation"])) % 360 + 360) % 360, BoardSide(p["side"]), item.location))
    return tuple(result)


def bind_attachments(attachments, footprints, placements, rules, merge, *, explicit_angles=frozenset()):
    from .placement import transformed_local_point
    poses, by_reference = {p.reference:p for p in placements}, {r.reference:r for r in rules}
    resolved = []
    for binding in attachments:
        if binding.reference not in poses:
            raise ValueError(f"attachment {binding.id!r} requires a selected component footprint")
        pose = poses[binding.reference]
        local = binding.anchor_point or Point(0,0)
        if binding.anchor == "pad":
            pads = [p for p in footprints[pose.footprint].pads if p.number == binding.anchor_pad
                    and p.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}]
            if len(pads) != 1:
                raise ValueError("attachment anchor_pad must identify exactly one electrical physical land")
            local = pads[0].position
        zero = replace(pose, position=Point(0,0), rotation_degrees=binding.rotation, side=binding.side)
        offset = transformed_local_point(zero, local)
        origin = Point(binding.position.x_nm-offset.x_nm, binding.position.y_nm-offset.y_nm)
        previous = by_reference.get(binding.reference)
        values = {"fixed_position": origin, "fixed_rotation_degrees": binding.rotation, "side": binding.side}
        for key, value in values.items():
            if previous and getattr(previous,key) is not None and getattr(previous,key) != value:
                raise ValueError(f"attachment {binding.id!r} conflicts with existing {key}")
        if previous and binding.reference in explicit_angles and binding.rotation not in previous.allowed_orientations:
            raise ValueError(f"attachment {binding.id!r} conflicts with allowed orientations")
        by_reference[binding.reference] = merge(previous,binding.reference,**values,
            allowed_orientations=previous.allowed_orientations if previous and binding.reference in explicit_angles else (binding.rotation,),priority=max(previous.priority if previous else 0,1000))
        resolved.append(PhysicalAttachment(binding.id,binding.reference,binding.target,binding.position,binding.offset,
                                          binding.anchor,local,binding.rotation,binding.side))
    return tuple(by_reference[r] for r in sorted(by_reference)), tuple(resolved)
