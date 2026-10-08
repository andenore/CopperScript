"""Explicit binding, pose transforms and transactional hard-macro copper.

This is a deliberately staged prototype: bind -> place -> materialize -> route
ordinary external connections. Planning materializes an immutable scratch
prefix; feedback rebuilds it after moving complete placement units. This must
not flatten owner copper into disposable fanout or claim RF qualification.
"""
from collections import Counter
from dataclasses import replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
from functools import lru_cache
import json
from pathlib import Path

from .clusters import (cluster_placement_matches, footprint_geometry_digest,
                       legacy_footprint_geometry_digest)
from .geometry import RoundedConvexShape, circle_inside_shape, point_in_polygon, segment_in_polygon, segments_intersect, shapes_clear
from .placement import transformed_local_point
from .syntax import CopperScriptError
from .physical import (
    BoardSide, ComponentPlacementRule, CopperKeepout, CopperLayer, CopperZone, MacroPadBinding, MacroPlaneReturn, MacroPort, PadReference,
    PhysicalBoard, PhysicalHardMacro, PhysicalNet, PlacementTarget,
    PadKind, Point, PolygonRing, PolygonWithHoles, RigidPlacementCluster, ZoneConnection,
    RigidPlacementMember, TrackSegment, Via,
)


def _point(value):
    if not isinstance(value, list) or len(value) != 2 or any(type(x) is not int for x in value):
        raise ValueError("macro coordinates require two integer nanometres")
    return Point(*value)


def _keys(row, expected):
    if not isinstance(row, dict) or set(row) != set(expected.split()):
        raise ValueError("unsupported hard-macro fields")


def _macro_footprint_digest(footprint, reference, asset):
    """Verify legacy module-root asset IDs without weakening geometry checks."""
    if footprint.name == reference:
        return footprint_geometry_digest(footprint)
    metadata = footprint.metadata
    module = metadata.get("module_path")
    identity = metadata.get("source_asset")
    if (not module or not identity or metadata.get("resolution") != "managed" or
            "\\" in reference or any(p in {"", ".", ".."} for p in reference.split("/"))):
        return None
    # A component may select a package asset by its direct URL while the
    # identity-bound macro records the corresponding logical KiCad namespace.
    # Require the URL to end in the exact namespace/leaf pair before normalizing
    # the resolved identity; unrelated files cannot satisfy the binding.
    if ":" in reference and "/" not in reference:
        namespace, leaf = reference.split(":", 1)
        if not namespace or not leaf or not identity.endswith(f"/{namespace}.pretty/{leaf}.kicad_mod"):
            return None
        normalized = replace(footprint, name=reference, source_library_id=reference,
                             metadata={**metadata, "resolved_reference": reference})
        return footprint_geometry_digest(normalized)
    if (not reference.endswith(".kicad_mod") or
            any(p in {"", ".", ".."} for p in reference.split("/")) or
            identity != module + "/" + reference):
        return None
    # A module-relative ID is meaningful only within the macro's own module.
    source = Path(metadata["source_path"]).resolve()
    root = source.parents[len(reference.split("/")) - 1]
    if not Path(asset).resolve().is_relative_to(root):
        return None
    legacy = replace(footprint, name=reference, source_library_id=reference,
                     metadata={**metadata, "resolved_reference": reference})
    return footprint_geometry_digest(legacy)


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
        version = data["schema"]
        fields = "schema source production_publishable anchor members pad_nets isolated_pads tracks vias ports protected_regions keepouts required_layers allowed_rotations internal_clearance_nm unresolved"
        if version == "copperlib-physical-hard-macro/v0.2":
            fields += " zones plane_returns"
        _keys(data, fields)
        if version not in {"copperlib-physical-hard-macro/v0.1", "copperlib-physical-hard-macro/v0.2"} or data["production_publishable"] is not False:
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
            footprint = board.footprints[pose.footprint]
            digest = _macro_footprint_digest(footprint, row["footprint"], asset)
            legacy_digest = (legacy_footprint_geometry_digest(footprint)
                             if digest is not None else None)
            if digest != row["footprint_digest"] and legacy_digest != row["footprint_digest"]:
                raise ValueError(f"hard-macro footprint identity mismatch: {pose.reference}")
            position = _point(row["center_nm"])
            members.append(RigidPlacementMember(pose.reference, pose.footprint, footprint_geometry_digest(footprint), position, row["rotation_degrees"]))
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
            row = {"finish": "standard", **row}
            _keys(row, "net position_nm size_nm drill_nm from_layer to_layer technology finish")
            if any(type(row[k]) is not int for k in ("size_nm", "drill_nm")):
                raise ValueError("macro via dimensions require integer nanometres")
            vias.append(Via(net_bindings[row["net"]], _point(row["position_nm"]), row["size_nm"], row["drill_nm"],
                            CopperLayer(row["from_layer"]), CopperLayer(row["to_layer"]), row["technology"],
                            row["finish"]))
        ports = []
        for row in data["ports"]:
            _keys(row, "name net point layer pads")
            ports.append(MacroPort(row["name"], net_bindings[row["net"]], endpoint(row["point"]),
                CopperLayer(row["layer"]), tuple(pads[tuple(p)] for p in row["pads"])))
        zones = []
        for row in data.get("zones", ()):
            _keys(row, "id net layers vertices priority clearance_nm minimum_width_nm pad_connection")
            if (type(row["priority"]) is not int or type(row["clearance_nm"]) is not int
                    or type(row["minimum_width_nm"]) is not int):
                raise ValueError("macro zone dimensions require integer nanometres")
            from .mechanical import validated_ring
            vertices = validated_ring(tuple(_point(p) for p in row["vertices"]))
            zones.append(CopperZone(f"{name}/{row['id']}", net_bindings[row["net"]],
                tuple(CopperLayer(layer) for layer in row["layers"]),
                PolygonWithHoles(PolygonRing(vertices)),
                priority=row["priority"], clearance_nm=row["clearance_nm"],
                minimum_width_nm=row["minimum_width_nm"], pad_connection=ZoneConnection(row["pad_connection"])))
        plane_returns = []
        for row in data.get("plane_returns", ()):
            _keys(row, "net layers pads dedicated_contacts")
            contacts = []
            for contact in row["dedicated_contacts"]:
                _keys(contact, "pad via_position_nm")
                contacts.append((pads[tuple(contact["pad"])], _point(contact["via_position_nm"])))
            plane_returns.append(MacroPlaneReturn(net_bindings[row["net"]],
                tuple(CopperLayer(layer) for layer in row["layers"]),
                tuple(pads[tuple(p)] for p in row["pads"]), tuple(contacts)))
        anchor = PlacementTarget(bindings[data["anchor"]], None)
        cluster = RigidPlacementCluster(name, anchor, tuple(members), str(data["source"]),
            allowed_rotations=tuple(data["allowed_rotations"]), keepouts=tuple(region(r) for r in data["keepouts"]),
            internal_clearance_nm=data["internal_clearance_nm"])
        macro = PhysicalHardMacro(name, expected_sha256, tuple(tracks), tuple(vias), tuple(ports),
            tuple(region(r) for r in data["protected_regions"]), tuple(CopperLayer(x) for x in data["required_layers"]),
            tuple(MacroPadBinding(pads[ref,pad],net_bindings[role]) for ref,pad,role in data["pad_nets"]),
            tuple(PadReference(bindings[ref],pad) for ref,pad in data["isolated_pads"]),
            tuple(zones), tuple(plane_returns))
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
    return _transformed_macro_geometry(macro, frame)


def resolved_macro_zones(board, macro, placements=None):
    """Resolve asset-owned local zones at the same rigid pose as owner tracks."""
    cluster = next(c for c in board.rigid_clusters if c.name == macro.cluster)
    poses = placements if placements is not None else {p.reference: p for p in board.placements}
    anchor = poses[cluster.anchor.reference]
    local_anchor = next(m for m in cluster.members if m.reference == anchor.reference)
    frame = replace(anchor, position=transformed_local_point(anchor, Point(-local_anchor.position.x_nm, -local_anchor.position.y_nm)))
    return tuple(replace(zone, outline=PolygonWithHoles(PolygonRing(tuple(
        transformed_local_point(frame, p) for p in zone.outline.outer.vertices)))) for zone in macro.zones)


def _zone_outlines_overlap(first, second):
    """Conservative closed-polygon collision check; v0.2 zones have no holes."""
    if not set(first.layers).intersection(second.layers):
        return False
    a, b = first.outline.outer.vertices, second.outline.outer.vertices
    edges = lambda points: tuple(zip(points, (*points[1:], points[0])))
    return (any(segments_intersect(*x, *y) for x in edges(a) for y in edges(b))
            or any(point_in_polygon(p, b) for p in a)
            or any(point_in_polygon(p, a) for p in b))


def _host_zone_overlaps_macro(board, macro, hosts):
    layers = {layer for zone in macro.zones for layer in zone.layers}
    if not layers:
        return False
    return any(_zone_outlines_overlap(host, replace(region, layers=tuple(layers.intersection(region.layers))))
               for host in hosts for region in resolved_macro_geometry(board, macro)[3]
               if layers.intersection(region.layers))


@lru_cache(maxsize=256)
def _transformed_macro_geometry(macro, frame):
    """Share immutable copper/region transforms across placement probes."""
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
        layers.update(l for z in macro.zones for l in z.layers)
        layers.update(l for r in macro.plane_returns for l in r.layers)
        if not layers <= set(board.stackup.copper_layers):
            raise ValueError("hard-macro layer contract does not match the stackup")
        for zone in macro.zones:
            points = zone.outline.outer.vertices
            edges = tuple(zip(points, (*points[1:], points[0])))
            if not all(any(layer in region.layers and all(segment_in_polygon(a, b, region.outline.outer.vertices)
                       for a, b in edges) for region in macro.protected_regions) for layer in zone.layers):
                raise ValueError("hard-macro zone must stay inside its protected region")
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
        for item in (*macro.tracks, *macro.vias, *macro.ports, *macro.zones):
            if item.net not in nets:
                raise ValueError("hard macro references an unknown net")
        for port in macro.ports:
            if any(p.component not in members or assigned.get(p) != port.net for p in port.pads):
                raise ValueError("macro port pad/net binding mismatch")
            shape = RoundedConvexShape((port.position,))
            if any(port.layer in r.layers and not shapes_clear(shape, RoundedConvexShape(r.outline.outer.vertices), 1)
                   for r in macro.protected_regions):
                raise ValueError("macro ports must sit outside protected copper regions")
        if len({r.net for r in macro.plane_returns}) != len(macro.plane_returns):
            raise ValueError("duplicate macro plane-return net")
        for plane in macro.plane_returns:
            bound = {b.pad for b in macro.pad_bindings if b.net == plane.net}
            if (not bound or set(plane.pads) != bound or len(plane.pads) != len(bound)
                    or any(p.net == plane.net for p in macro.ports)):
                raise ValueError("macro plane return must cover every private pad without a port")
            if any(pad not in bound for pad, _ in plane.dedicated_contacts):
                raise ValueError("macro plane contact references an unbound pad")
            if (len({pad for pad, _ in plane.dedicated_contacts}) != len(plane.dedicated_contacts)
                    or len({xy for _, xy in plane.dedicated_contacts}) != len(plane.dedicated_contacts)):
                raise ValueError("macro plane contacts require distinct pads and vias")
    if not board.materialized_macros:
        return
    poses = {p.reference: p for p in board.placements}
    if not cluster_placement_matches(board, poses):
        raise ValueError("cannot move a materialized macro; rebuild it from an unrouted source")
    owned_tracks = Counter()
    owned_vias = Counter()
    owned_zones = {}
    for m in board.hard_macros:
        if m.cluster in board.materialized_macros:
            t, v, _, _ = resolved_macro_geometry(board, m)
            _validate_macro_via_permissions(board, m, v)
            owned_tracks.update(t)
            owned_vias.update(v)
            for zone in resolved_macro_zones(board, m):
                if zone.id in owned_zones:
                    raise ValueError("duplicate hard-macro zone owner")
                owned_zones[zone.id] = zone
    if owned_tracks - Counter(board.tracks) or owned_vias - Counter(board.vias):
        raise ValueError("immutable hard-macro copper was changed or removed")
    board_zones = {z.id: z for z in board.zones}
    if any(board_zones.get(zone_id) != zone for zone_id, zone in owned_zones.items()):
        raise ValueError("immutable hard-macro zone was changed or removed")
    host_zones = tuple(zone for zone in board.zones if zone.id not in owned_zones)
    if any(_host_zone_overlaps_macro(board, macro, host_zones) for macro in board.hard_macros
           if macro.cluster in board.materialized_macros):
        raise ValueError("host zone overlaps a private hard-macro region")
    from .placement import resolved_copper_keepouts
    if any(_zone_outlines_overlap(zone, keepout) for zone in owned_zones.values()
           for keepout in resolved_copper_keepouts(board) if keepout.block_zones):
        raise ValueError("hard-macro zone is blocked by a copper keepout")
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


def _validate_macro_via_permissions(board, macro, vias):
    """An asset's finish string cannot authorize otherwise forbidden via-in-pad."""
    from .drc import placed_pad_shape
    permitted = {rule.pad for rule in board.via_in_pad_rules}
    bound = {binding.pad for binding in macro.pad_bindings if binding.net == "GND"}
    for via in vias:
        if via.finish != "filled-capped":
            continue
        if (via.net != "GND" or board.metadata.get("fabrication_profile") != "jlcpcb-six-layer"
                or len(board.stackup.copper_layers) != 6
                or (via.from_layer, via.to_layer) != (CopperLayer.FRONT, CopperLayer.BACK)):
            raise ValueError("macro filled-capped vias require the six-layer GND through-via process")
        annulus = RoundedConvexShape((via.position,), via.size_nm // 2)
        contacts = []
        for pose in board.placements:
            for pad in board.footprints[pose.footprint].pads:
                if pad.kind is not PadKind.SMD:
                    continue
                shape = placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose)
                if shapes_clear(annulus, shape, 1):
                    continue
                ref = PadReference(pose.reference, pad.number)
                if ref not in permitted or ref not in bound:
                    raise ValueError("macro via-in-pad requires explicit permission for the contacted private pad")
                if not circle_inside_shape(via.position, via.size_nm // 2, shape):
                    raise ValueError("macro via-in-pad annulus must be contained in its permitted pad")
                contacts.append(ref)
        if len(contacts) > 1:
            raise ValueError("macro via-in-pad cannot overlap multiple pad lands")


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
    tracks, vias, zones = [], [], []
    for macro in board.hard_macros:
        t, v, _, _ = resolved_macro_geometry(board, macro)
        tracks.extend(t)
        vias.extend(v)
        zones.extend(resolved_macro_zones(board, macro))
    if any(_host_zone_overlaps_macro(board, macro, board.zones) for macro in board.hard_macros):
        raise ValueError("host zone overlaps a private hard-macro region")
    if any(_zone_outlines_overlap(zone, other) for index, zone in enumerate(zones)
           for other in zones[:index]):
        raise ValueError("private hard-macro zones overlap")
    result = replace(board, tracks=tuple(tracks), vias=tuple(vias), zones=(*board.zones, *zones),
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
        # A wholly private net has no boundary port that could prove it later.
        # Check all its physical lands now; never borrow another owner's copper.
        port_nets = {port.net for port in ports}
        planes = {plane.net: plane for plane in macro.plane_returns}
        for net in owner.nets:
            bound = tuple(b.pad for b in macro.pad_bindings if b.net == net.name)
            if not bound:
                continue
            if net.name in port_nets:
                covered = {pad for port in ports if port.net == net.name for pad in port.pads}
                if not set(bound) <= covered:
                    raise ValueError(f"macro ports omit private pads on net {net.name!r}")
                continue
            if net.name in planes:
                plane = planes[net.name]
                zones = tuple(zone for zone in result.zones if zone.net == net.name
                              and set(zone.layers).intersection(plane.layers))
                if not zones:
                    raise ValueError(f"macro plane return {net.name!r} has no declared plane")
                usable_vias = {}
                for index, via in enumerate(v):
                    span = result.stackup.copper_layers
                    a, b = sorted((span.index(via.from_layer), span.index(via.to_layer)))
                    if via.net == net.name and any(
                        layer in zone.layers and layer in span[a:b+1]
                        and point_in_polygon(via.position, zone.outline.outer.vertices)
                        and not any(point_in_polygon(via.position, h.vertices) for h in zone.outline.holes)
                        for zone in zones for layer in plane.layers):
                        usable_vias[via.position] = graph.roots[f"via:{index}"]
                usable_roots = set(usable_vias.values())
                if any(not graph.pad_nodes.get(pad) or any(
                    graph.roots[node] not in usable_roots for node in graph.pad_nodes[pad]) for pad in bound):
                    raise ValueError(f"macro plane return {net.name!r} lacks a local via for a private pad")
                for pad, local_xy in plane.dedicated_contacts:
                    matching = [v[index] for index, source_via in enumerate(macro.vias)
                                if source_via.net == net.name and source_via.position == local_xy]
                    if len(matching) != 1 or matching[0].position not in usable_vias or any(
                        graph.roots[node] != usable_vias[matching[0].position]
                        for node in graph.pad_nodes.get(pad, ())):
                        raise ValueError(f"macro dedicated plane contact for {pad} is missing")
                continue
            # Ownership is per pad: an unbound pad on a member IC is public
            # just like a consumer on another component. It cannot disappear
            # from the connectivity proof for a claimed internal net.
            if any(p not in bound for p in next(n for n in result.nets if n.name == net.name).pads):
                raise ValueError(f"macro net {net.name!r} requires an external port")
            roots = {graph.roots[node] for pad in bound for node in graph.pad_nodes.get(pad, ())}
            if len(roots) != 1 or any(not graph.pad_nodes.get(pad) for pad in bound):
                raise ValueError(f"macro internal net {net.name!r} is not connected")
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
    """Collapse proved private groups without modifying the actual netlist.

    The retained PadReference is a stable routing identity, NOT a request to
    start copper at that pad. Position and layers come from the boundary port.
    """
    remove = set()
    for representative, port in macro_routing_ports(board, net.name).items():
        remove.update(set(port.pads) - {representative})
    return tuple(p for p in net.pads if p not in remove)


def macro_routing_ports(board, net_name):
    """Pad, free-track and plated-via terminals after owner continuity proof."""
    result = {}
    covered = set()
    for m in board.hard_macros:
        if m.cluster not in board.materialized_macros:
            continue
        for port in sorted(resolved_macro_geometry(board, m)[2], key=lambda p: p.name):
            if port.net != net_name or not port.pads:
                continue
            if covered.intersection(port.pads):
                raise ValueError("overlapping macro routing port pad groups are unsupported")
            covered.update(port.pads)
            poses = {p.reference: p for p in board.placements}
            at_port = [ref for ref in port.pads if any(
                transformed_local_point(poses[ref.component], pad.position) == port.position
                for pad in board.footprints[poses[ref.component].footprint].pads
                if pad.number == ref.pad)]
            result[at_port[0] if len(at_port) == 1 else min(port.pads)] = port
    return result


def macro_port_layers(board, port):
    """Only actual port copper layers: a via barrel exposes its full span."""
    layers = {port.layer}
    for via in board.vias:
        if via.net == port.net and via.position == port.position:
            a, b = sorted((board.stackup.copper_layers.index(via.from_layer),
                           board.stackup.copper_layers.index(via.to_layer)))
            layers.update(board.stackup.copper_layers[a:b+1])
    return tuple(layer for layer in board.stackup.copper_layers if layer in layers)


def macro_owned_pads(board):
    return frozenset(b.pad for m in board.hard_macros
                     if m.cluster in board.materialized_macros for b in m.pad_bindings)


def macro_source(board):
    """Recover an unrouted source; NEVER discard ordinary or critical copper."""
    if board.zone_fills:
        raise ValueError("routing planning requires an unfilled source")
    if not board.tracks and not board.vias and not board.materialized_macros:
        return board
    validate_hard_macros(board)
    tracks, vias = Counter(), Counter()
    for macro in board.hard_macros:
        if macro.cluster in board.materialized_macros:
            t, v, _, _ = resolved_macro_geometry(board, macro)
            tracks.update(t)
            vias.update(v)
    if Counter(board.tracks) != tracks or Counter(board.vias) != vias:
        raise ValueError("routing planning requires no copper except immutable hard macros")
    owned_zone_ids = {zone.id for macro in board.hard_macros for zone in resolved_macro_zones(board, macro)}
    return replace(board, tracks=(), vias=(), zones=tuple(z for z in board.zones if z.id not in owned_zone_ids),
                   materialized_macros=())


def apply_hard_macro_scene(board, scene_path, *, locked=True, offline=False):
    """Explicit pinned data binding; URL assets share the package cache/lock."""
    scene_path = Path(scene_path).resolve()
    try:
        scene = json.loads(scene_path.read_text(encoding="utf-8"))
        _keys(scene, "asset asset_sha256 name bindings net_bindings")
        asset = scene["asset"]
        if asset.startswith(("github.com/", "gitlab.com/", "https://github.com/", "https://gitlab.com/")):
            from .packages import resolve_module_asset
            asset_path = resolve_module_asset(scene_path, asset, locked=locked, offline=offline)
        else:
            asset_path = scene_path.parent / asset
        return bind_hard_macro(board, asset_path,
            expected_sha256=scene["asset_sha256"], name=scene["name"],
            bindings=scene["bindings"], net_bindings=scene["net_bindings"])
    except (OSError, json.JSONDecodeError, KeyError, TypeError, CopperScriptError) as exc:
        raise ValueError(f"invalid hard-macro scene: {exc}") from exc
