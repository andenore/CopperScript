"""Explicit binding, pose transforms and transactional hard-macro copper.

This is a deliberately staged prototype: bind -> place -> materialize -> route
ordinary external connections. The complete package-access/critical pipeline
does not yet allocate macro boundary ports. It must not silently flatten a
macro into disposable fanout or claim RF/manufacturing qualification.
"""
from collections import Counter
from dataclasses import replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
import json
from pathlib import Path

from .clusters import cluster_placement_matches, footprint_geometry_digest
from .geometry import RoundedConvexShape, shapes_clear
from .physical import (
    BoardSide, ComponentPlacementRule, CopperKeepout, CopperLayer, MacroPadBinding, MacroPort, PadReference,
    PhysicalBoard, PhysicalHardMacro, PhysicalNet, PlacementTarget,
    Point, PolygonRing, PolygonWithHoles, RigidPlacementCluster,
    RigidPlacementMember, TrackSegment, Via,
)


def _point(value):
    if not isinstance(value, list) or len(value) != 2 or any(type(x) is not int for x in value):
        raise ValueError("macro coordinates require two integer nanometres")
    return Point(*value)


def _keys(row, expected):
    if not isinstance(row, dict) or set(row) != set(expected.split()):
        raise ValueError("unsupported hard-macro fields")


def bind_hard_macro(board: PhysicalBoard, asset: Path, *, expected_sha256: str,
                    name: str, bindings: dict[str, str], net_bindings: dict[str, str]) -> PhysicalBoard:
    """Bind pinned data only. Never execute, fetch, rewire or move components."""
    if board.tracks or board.vias or board.zone_fills or board.materialized_macros:
        raise ValueError("hard-macro binding requires an unrouted, unfilled board")
    raw = Path(asset).read_bytes()
    if sha256(raw).hexdigest() != expected_sha256:
        raise ValueError("hard-macro asset identity changed")
    try:
        data = json.loads(raw)
        _keys(data, "schema source production_publishable anchor members pad_nets isolated_pads tracks vias ports protected_regions keepouts required_layers allowed_rotations internal_clearance_nm unresolved")
        if data["schema"] != "copperlib-physical-hard-macro/v0.1" or data["production_publishable"] is not False:
            raise ValueError("unsupported macro qualification/schema")
        sources = {m["reference"] for m in data["members"]}
        if len(sources) != len(data["members"]) or set(bindings) != sources or len(set(bindings.values())) != len(sources):
            raise ValueError("hard-macro bindings must cover unique members exactly")
        roles = {row[2] for row in data["pad_nets"]}
        if set(net_bindings) != roles or len(set(net_bindings.values())) != len(roles):
            raise ValueError("hard-macro net bindings must cover distinct roles exactly")
        poses = {p.reference: p for p in board.placements}
        assignment = {p: n.name for n in board.nets for p in n.pads}
        pads = {}
        for ref, pad, role in data["pad_nets"]:
            identity = PadReference(bindings[ref], pad)
            if assignment.get(identity) != net_bindings[role]:
                raise ValueError(f"hard-macro pad/net mismatch: {identity}")
            if (ref, pad) in pads:
                raise ValueError("duplicate macro pad binding")
            pads[ref, pad] = identity
        members = []
        local_poses = {}
        rules = {r.reference: r for r in board.placement_rules}
        for row in data["members"]:
            _keys(row, "reference footprint footprint_digest center_nm rotation_degrees edge_clearance_nm")
            pose = poses[bindings[row["reference"]]]
            if pose.footprint != row["footprint"] or footprint_geometry_digest(board.footprints[pose.footprint]) != row["footprint_digest"]:
                raise ValueError(f"hard-macro footprint identity mismatch: {pose.reference}")
            position = _point(row["center_nm"])
            members.append(RigidPlacementMember(pose.reference, pose.footprint, row["footprint_digest"], position, row["rotation_degrees"]))
            local_poses[row["reference"]] = replace(pose, position=position, rotation_degrees=row["rotation_degrees"], side=BoardSide.FRONT)
            if type(row["edge_clearance_nm"]) is not int or row["edge_clearance_nm"] <= 0:
                raise ValueError("macro member edge clearance requires positive integer nm")
            if pose.reference not in rules:
                rules[pose.reference] = ComponentPlacementRule(pose.reference,
                    allowed_orientations=tuple((Decimal(str(angle)) + Decimal(str(row["rotation_degrees"]))) % 360
                                               for angle in data["allowed_rotations"]),
                    edge_clearance_nm=row["edge_clearance_nm"])

        def endpoint(value):
            if isinstance(value, list):
                return _point(value)
            _keys(value, "pad")
            ref, number = value["pad"]
            if (ref, number) not in pads:
                raise ValueError("macro endpoint must have an explicit pad/net binding")
            footprint = board.footprints[local_poses[ref].footprint]
            lands = [p for p in footprint.pads if p.number == number]
            if len(lands) != 1:
                raise ValueError("macro endpoints require a unique physical land")
            from .placement import transformed_local_point
            return transformed_local_point(local_poses[ref], lands[0].position)

        def region(row):
            _keys(row, "id layers vertices block_tracks block_vias block_zones")
            if any(type(row[k]) is not bool for k in ("block_tracks", "block_vias", "block_zones")):
                raise ValueError("macro keepout flags require booleans")
            return CopperKeepout(row["id"], tuple(CopperLayer(x) for x in row["layers"]),
                PolygonWithHoles(PolygonRing(tuple(_point(x) for x in row["vertices"]))),
                block_tracks=row["block_tracks"], block_vias=row["block_vias"], block_zones=row["block_zones"])

        tracks = []
        for row in data["tracks"]:
            _keys(row, "net width_nm layer points")
            if type(row["width_nm"]) is not int:
                raise ValueError("macro widths require integer nanometres")
            points = tuple(endpoint(x) for x in row["points"])
            if len(points) < 2:
                raise ValueError("macro track requires at least two points")
            # A pad-anchored endpoint cannot silently select a different net.
            for value in row["points"]:
                if isinstance(value, dict) and assignment[pads[tuple(value["pad"])]] != net_bindings[row["net"]]:
                    raise ValueError("macro track endpoint net mismatch")
            tracks.extend(TrackSegment(net_bindings[row["net"]], a, b, row["width_nm"], CopperLayer(row["layer"]))
                          for a, b in zip(points, points[1:]))
        vias = []
        for row in data["vias"]:
            _keys(row, "net position_nm size_nm drill_nm from_layer to_layer technology")
            if any(type(row[k]) is not int for k in ("size_nm", "drill_nm")):
                raise ValueError("macro via dimensions require integer nanometres")
            vias.append(Via(net_bindings[row["net"]], _point(row["position_nm"]), row["size_nm"], row["drill_nm"],
                            CopperLayer(row["from_layer"]), CopperLayer(row["to_layer"]), row["technology"]))
        ports = []
        for row in data["ports"]:
            _keys(row, "name net point layer pads")
            ports.append(MacroPort(row["name"], net_bindings[row["net"]], endpoint(row["point"]),
                CopperLayer(row["layer"]), tuple(pads[tuple(p)] for p in row["pads"])))
        anchor = PlacementTarget(bindings[data["anchor"]], None)
        cluster = RigidPlacementCluster(name, anchor, tuple(members), str(data["source"]),
            allowed_rotations=tuple(data["allowed_rotations"]), keepouts=tuple(region(r) for r in data["keepouts"]),
            internal_clearance_nm=data["internal_clearance_nm"])
        macro = PhysicalHardMacro(name, expected_sha256, tuple(tracks), tuple(vias), tuple(ports),
            tuple(region(r) for r in data["protected_regions"]), tuple(CopperLayer(x) for x in data["required_layers"]),
            tuple(MacroPadBinding(pads[ref,pad],net_bindings[role]) for ref,pad,role in data["pad_nets"]),
            tuple(PadReference(bindings[ref],pad) for ref,pad in data["isolated_pads"]))
        return replace(board, rigid_clusters=(*board.rigid_clusters, cluster), hard_macros=(*board.hard_macros, macro),
            placement_rules=tuple(rules.values()),
            metadata={**board.metadata, "physical_hard_macros": "experimental-unqualified", "fabrication_ready": "false"})
    except (KeyError, TypeError, IndexError, StopIteration, InvalidOperation, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid hard-macro asset: {exc}") from exc


def resolved_macro_geometry(board, macro, placements=None):
    """Rotate once in integer nm, including 45 degrees; never mirror layer spans."""
    from .placement import transformed_local_point
    cluster = next(c for c in board.rigid_clusters if c.name == macro.cluster)
    poses = placements if placements is not None else {p.reference: p for p in board.placements}
    anchor = poses[cluster.anchor.reference]
    local_anchor = next(m for m in cluster.members if m.reference == anchor.reference)
    frame = replace(anchor, position=transformed_local_point(anchor, Point(-local_anchor.position.x_nm, -local_anchor.position.y_nm)))
    transform = lambda p: transformed_local_point(frame, p)
    tracks = tuple(replace(t, start=transform(t.start), end=transform(t.end)) for t in macro.tracks)
    vias = tuple(replace(v, position=transform(v.position)) for v in macro.vias)
    ports = tuple(replace(p, position=transform(p.position)) for p in macro.ports)
    regions = tuple(replace(r, outline=PolygonWithHoles(PolygonRing(tuple(transform(p) for p in r.outline.outer.vertices))))
                    for r in macro.protected_regions)
    return tracks, vias, ports, regions


def macro_reservations(board):
    """New copper cannot enter these areas, even when the net already matches."""
    return tuple(r for m in board.hard_macros if m.cluster in board.materialized_macros
                 for r in resolved_macro_geometry(board, m)[3])


def validate_hard_macros(board):
    clusters = {c.name: c for c in board.rigid_clusters}
    names = [m.cluster for m in board.hard_macros]
    if len(set(names)) != len(names) or len(set(board.materialized_macros)) != len(board.materialized_macros):
        raise ValueError("hard-macro identities must be unique")
    if not set(board.materialized_macros) <= set(names):
        raise ValueError("unknown materialized macro owner")
    nets = {n.name: n for n in board.nets}
    assigned = {p: n.name for n in board.nets for p in n.pads}
    for macro in board.hard_macros:
        if macro.cluster not in clusters:
            raise ValueError("hard macro requires an identity-bound placement cluster")
        layers = set(macro.required_layers)
        layers.update(t.layer for t in macro.tracks)
        layers.update(l for v in macro.vias for l in (v.from_layer, v.to_layer))
        layers.update(p.layer for p in macro.ports)
        layers.update(l for r in macro.protected_regions for l in r.layers)
        if not layers <= set(board.stackup.copper_layers):
            raise ValueError("hard-macro layer contract does not match the stackup")
        members = {m.reference for m in clusters[macro.cluster].members}
        if len({b.pad for b in macro.pad_bindings}) != len(macro.pad_bindings):
            raise ValueError("duplicate hard-macro pad/net bindings")
        if any(b.pad.component not in members or assigned.get(b.pad) != b.net for b in macro.pad_bindings):
            raise ValueError("hard-macro pad/net bindings changed")
        if len(set(macro.isolated_pads)) != len(macro.isolated_pads):
            raise ValueError("duplicate isolated macro pads")
        poses = {p.reference:p for p in board.placements}
        for pad in macro.isolated_pads:
            if pad.component not in members or pad in assigned or not any(
                p.number == pad.pad for p in board.footprints[poses[pad.component].footprint].pads):
                raise ValueError("hard-macro isolated pad must remain physically present and unassigned")
        for item in (*macro.tracks, *macro.vias, *macro.ports):
            if item.net not in nets:
                raise ValueError("hard macro references an unknown net")
        for port in macro.ports:
            if any(p.component not in members or assigned.get(p) != port.net for p in port.pads):
                raise ValueError("macro port pad/net binding mismatch")
            shape = RoundedConvexShape((port.position,))
            if any(port.layer in r.layers and not shapes_clear(shape, RoundedConvexShape(r.outline.outer.vertices), 1)
                   for r in macro.protected_regions):
                raise ValueError("macro ports must sit outside protected copper regions")
    if not board.materialized_macros:
        return
    poses = {p.reference: p for p in board.placements}
    if not cluster_placement_matches(board, poses):
        raise ValueError("cannot move a materialized macro; rebuild it from an unrouted source")
    owned_tracks = Counter()
    owned_vias = Counter()
    for m in board.hard_macros:
        if m.cluster in board.materialized_macros:
            t, v, _, _ = resolved_macro_geometry(board, m)
            owned_tracks.update(t)
            owned_vias.update(v)
    if owned_tracks - Counter(board.tracks) or owned_vias - Counter(board.vias):
        raise ValueError("immutable hard-macro copper was changed or removed")
    # Defend all callers, not just the preferred detailed-router entry point.
    regions = macro_reservations(board)
    # One macro's ownership is not permission to enter another macro's private
    # copper region, even on the same net.
    for owner in board.hard_macros:
        if owner.cluster not in board.materialized_macros:
            continue
        tracks, vias, _, _ = resolved_macro_geometry(board, owner)
        other_regions = tuple(r for m in board.hard_macros
                              if m.cluster != owner.cluster and m.cluster in board.materialized_macros
                              for r in resolved_macro_geometry(board, m)[3])
        for track in tracks:
            shape = RoundedConvexShape((track.start,track.end),track.width_nm // 2)
            if any(track.layer in r.layers and not shapes_clear(shape,RoundedConvexShape(r.outline.outer.vertices),1)
                   for r in other_regions):
                raise ValueError("hard-macro copper intrudes into another macro reservation")
        for via in vias:
            a,b = sorted((board.stackup.copper_layers.index(via.from_layer),board.stackup.copper_layers.index(via.to_layer)))
            span = set(board.stackup.copper_layers[a:b+1])
            shape = RoundedConvexShape((via.position,),via.size_nm // 2)
            if any(span.intersection(r.layers) and not shapes_clear(shape,RoundedConvexShape(r.outline.outer.vertices),1)
                   for r in other_regions):
                raise ValueError("hard-macro via intrudes into another macro reservation")
    for track in (Counter(board.tracks) - owned_tracks).elements():
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        if any(track.layer in r.layers and not shapes_clear(shape, RoundedConvexShape(r.outline.outer.vertices), 1) for r in regions):
            raise ValueError("new copper intrudes into a protected hard-macro region")
    for via in (Counter(board.vias) - owned_vias).elements():
        a, b = sorted((board.stackup.copper_layers.index(via.from_layer), board.stackup.copper_layers.index(via.to_layer)))
        span = set(board.stackup.copper_layers[a:b + 1])
        shape = RoundedConvexShape((via.position,), via.size_nm // 2)
        if any(span.intersection(r.layers) and not shapes_clear(shape, RoundedConvexShape(r.outline.outer.vertices), 1) for r in regions):
            raise ValueError("new via intrudes into a protected hard-macro region")


def materialize_hard_macros(board: PhysicalBoard) -> PhysicalBoard:
    """Commit the entire set, or reject without changing any input geometry."""
    if board.materialized_macros:
        validate_hard_macros(board)
        if set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
            raise ValueError("partial hard-macro materialization is unsupported")
        return board  # idempotent; no duplicate owner copper
    if not board.hard_macros:
        return board
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("macro materialization must precede all routing and fill")
    poses = {p.reference: p for p in board.placements}
    if not cluster_placement_matches(board, poses):
        raise ValueError("place the complete rigid macro before materializing copper")
    tracks, vias = [], []
    for macro in board.hard_macros:
        t, v, _, _ = resolved_macro_geometry(board, macro)
        tracks.extend(t)
        vias.extend(v)
    result = replace(board, tracks=tuple(tracks), vias=tuple(vias),
        materialized_macros=tuple(m.cluster for m in board.hard_macros),
        metadata={**board.metadata, "fabrication_ready": "false"})
    from .drc import DrcSeverity, PhysicalDrcPolicy, run_physical_drc, explicit_copper_connectivity
    report = run_physical_drc(result, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    fatal = [f for f in report.findings if f.severity is DrcSeverity.ERROR and f.code != "DRC-OPEN-NET"]
    if fatal:
        raise ValueError("hard-macro physical acceptance failed: " + "; ".join(f"{f.code}: {f.message}" for f in fatal))
    # Verify port -> private-pad continuity using actual copper, not role names.
    for macro in board.hard_macros:
        t, v, ports, _ = resolved_macro_geometry(result, macro)
        cluster = next(c for c in result.rigid_clusters if c.name == macro.cluster)
        members = {m.reference for m in cluster.members}
        poses = tuple(p for p in result.placements if p.reference in members)
        # A port proof cannot borrow a different macro's or outsider's pads.
        owner = PhysicalBoard(result.name, result.outline,
            {p.footprint:result.footprints[p.footprint] for p in poses}, poses,
            tuple(PhysicalNet(n.name,tuple(p for p in n.pads if p.component in members)) for n in result.nets),
            stackup=result.stackup, rules=result.rules, tracks=t, vias=v)
        graph = explicit_copper_connectivity(owner)
        for port in ports:
            # A via port denotes its plated annulus, not imaginary solid copper
            # at its drill centre. Use that actual object's exact graph root.
            root_ids = {graph.roots[f"track:{i}"] for i, track in enumerate(t)
                        if track.net == port.net and track.layer is port.layer
                        and not shapes_clear(RoundedConvexShape((port.position,)),
                                             RoundedConvexShape((track.start,track.end),track.width_nm // 2),1)}
            for i, via in enumerate(v):
                a,b = sorted((result.stackup.copper_layers.index(via.from_layer),result.stackup.copper_layers.index(via.to_layer)))
                if via.net == port.net and via.position == port.position and port.layer in result.stackup.copper_layers[a:b+1]:
                    root_ids.add(graph.roots[f"via:{i}"])
            if not root_ids:
                raise ValueError("macro port has no owner copper contact")
            if len(root_ids) != 1 or any(not graph.pad_nodes.get(pad) or any(graph.roots[n] not in root_ids for n in graph.pad_nodes[pad]) for pad in port.pads):
                raise ValueError("macro port is not connected to its declared private pads")
    return result


def macro_routing_pads(board, net):
    """Use a pad-backed external port instead of routing private pads again.

    Only ports at an actual unique pad centre are presently consumed by the
    ordinary detailed router. Via/free ports are validated/exported but are
    not yet router terminals; their pad groups stay untouched (fail closed).
    """
    from .placement import transformed_local_point
    remove = set()
    poses = {p.reference: p for p in board.placements}
    for m in board.hard_macros:
        if m.cluster not in board.materialized_macros:
            continue
        for port in resolved_macro_geometry(board, m)[2]:
            if port.net != net.name:
                continue
            candidates = []
            for padref in port.pads:
                pose = poses[padref.component]
                lands = [p for p in board.footprints[pose.footprint].pads if p.number == padref.pad]
                if len(lands) == 1 and transformed_local_point(pose, lands[0].position) == port.position:
                    candidates.append(padref)
            if candidates:
                remove.update(set(port.pads) - {min(candidates)})
    return tuple(p for p in net.pads if p not in remove)
