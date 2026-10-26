"""Real copper geometry, no project-specific repair selectors or search mocks."""
from dataclasses import replace
from hashlib import sha256
import json


from pcbir import (BoardOutline, CopperLayer, CopperZone, FootprintPad, GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size, Stackup, nm_from_mm, route_global)
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions
from pcbir.clusters import footprint_geometry_digest
from pcbir.hard_macros import bind_hard_macro
from pcbir.physical import PadViaInPadRule
from pcbir.physical import NetRoutingRule, RouteKind


def constrained_contact_board(rotation=0, renamed=False):
    package = PhysicalFootprint("paired-lands", (
        FootprintPad("1", Point.mm(0, -1), Size.mm("0.3", "0.3")),
        FootprintPad("2", Point.mm(0, "-0.4"), Size.mm("0.3", "0.3"))), Size.mm(2, 3))
    terminal = PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.3", "0.3")),), Size.mm(1, 1))
    # A real narrow source-defined contact window, not a mocked via generator.
    zone = CopperZone("contact-window", "RAIL", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
            ("3.95", "5.95"), ("4.05", "5.95"), ("4.05", "6.05"), ("3.95", "6.05"))))))
    board = PhysicalBoard("scarce-contact", BoardOutline.rectangle(16, 16),
        {package.name: package, terminal.name: terminal},
        (Placement("DEVICE", package.name, Point.mm(5, 7)),
         Placement("POWER", terminal.name, Point.mm(3, 6)),
         Placement("REMOTE", terminal.name, Point.mm(13, "6.6"))),
        (PhysicalNet("RAIL", (PadReference("DEVICE", "1"), PadReference("POWER", "1"))),
         PhysicalNet("CLOCK", (PadReference("DEVICE", "2"), PadReference("REMOTE", "1")))),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                        CopperLayer.INTERNAL_2, CopperLayer.BACK)), zones=(zone,))
    def rotated(point):
        x, y = point.x_nm - nm_from_mm(8), point.y_nm - nm_from_mm(8)
        for _ in range(rotation // 90):
            x, y = -y, x
        return Point(x + nm_from_mm(8), y + nm_from_mm(8))
    names = {"DEVICE": "IC_72", "POWER": "CONNECTOR_9", "REMOTE": "LOAD_4",
             "RAIL": "SUPPLY_Z", "CLOCK": "DATA_X"} if renamed else {}
    def named(value):
        return names.get(value, value)
    return replace(board,
        placements=tuple(replace(p, reference=named(p.reference), position=rotated(p.position),
                                 rotation_degrees=rotation) for p in board.placements),
        nets=tuple(replace(n, name=named(n.name), pads=tuple(
            PadReference(named(p.component), p.pad) for p in n.pads)) for n in board.nets),
        zones=(replace(zone, net=named(zone.net), outline=PolygonWithHoles(
            PolygonRing(tuple(rotated(p) for p in zone.outline.outer.vertices)))),))


def settings():
    return FanoutOptions(minimum_component_pads=2,
        maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False)


def hard_findings(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def test_cli_reserves_package_access_by_default_with_explicit_opt_out():
    from pcbir.cli import _parser
    default = _parser().parse_args(["route-board", "board.copper"])
    assert default.fanout and default.early_plane_stitch
    assert _parser().parse_args(["route-board", "board.copper", "--fanout"]).fanout
    assert not _parser().parse_args(["route-board", "board.copper", "--no-fanout"]).fanout


def with_test_owners(board, tmp_path, *, blocking_macro=False, required_array=False):
    """Synthetic owner geometry only, not a reusable regulator/component design.

    The blocking macro owns the sole rail contact window; the nonblocking macro
    supplies a protected track/via prefix elsewhere. A required return-pad array
    lies in the ordinary signal corridor, so area routing must route around it.
    """
    terminal = board.footprints["terminal"]
    first, second = ((Point.mm(4, 2), Point.mm(4, 10)) if blocking_macro
                     else (Point.mm(10, 11), Point.mm(12, 11)))
    offset = [second.x_nm-first.x_nm, second.y_nm-first.y_nm]
    midpoint = [offset[0] // 2, offset[1] // 2]
    # The blocking via owns the contact, but its protected strip stays outside
    # the neighboring package courtyard. Port copper may extend past the strip.
    region = ([[nm_from_mm("-0.2"), nm_from_mm("0.5")],
               [nm_from_mm("-0.05"), nm_from_mm("0.5")],
               [nm_from_mm("-0.05"), nm_from_mm("7.5")],
               [nm_from_mm("-0.2"), nm_from_mm("7.5")]] if blocking_macro else
              [[nm_from_mm("0.5"), nm_from_mm("-0.25")],
               [nm_from_mm("1.5"), nm_from_mm("-0.25")],
               [nm_from_mm("1.5"), nm_from_mm("0.25")],
               [nm_from_mm("0.5"), nm_from_mm("0.25")]])
    board = replace(board, placements=(*board.placements,
        Placement("OWNER_A", terminal.name, first), Placement("OWNER_B", terminal.name, second)),
        nets=(*board.nets, PhysicalNet("OWNER_NET", (
            PadReference("OWNER_A", "1"), PadReference("OWNER_B", "1")))))
    if required_array:
        exposed = PhysicalFootprint("synthetic-exposed-land", (
            FootprintPad("EP", Point(0, 0), Size.mm("1.5", "1.5")),), Size.mm(2, 2))
        board = replace(board, footprints={**board.footprints, exposed.name: exposed},
            placements=(*board.placements, Placement("ARRAY_OWNER", exposed.name, Point.mm(9, "6.6"))),
            nets=(*board.nets, PhysicalNet("GND", (PadReference("ARRAY_OWNER", "EP"),))),
            stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                            CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)),
            zones=(*board.zones, CopperZone("array-return", "GND", (CopperLayer.INTERNAL_2,),
                PolygonWithHoles(PolygonRing(board.outline.vertices)))),
            via_in_pad_rules=(PadViaInPadRule(PadReference("ARRAY_OWNER", "EP"),
                rows=2, columns=2, pitch_nm=nm_from_mm("0.6")),),
            metadata={"fabrication_profile": "jlcpcb-six-layer"})
    asset = dict(schema="copperlib-physical-hard-macro/v0.1", source="synthetic ownership regression",
        production_publishable=False, anchor="first", members=[dict(reference=ref,
            footprint=terminal.name, footprint_digest=footprint_geometry_digest(terminal),
            center_nm=center, rotation_degrees="0", edge_clearance_nm=250000)
            for ref, center in (("first", [0, 0]), ("second", offset))],
        pad_nets=[["first", "1", "owned"], ["second", "1", "owned"]], isolated_pads=[],
        tracks=[dict(net="owned", width_nm=250000, layer="F.Cu",
            points=[{"pad": ["first", "1"]}, {"pad": ["second", "1"]}])],
        vias=[dict(net="owned", position_nm=midpoint, size_nm=800000, drill_nm=400000,
            from_layer="F.Cu", to_layer="B.Cu", technology=None)],
        ports=[dict(name="owned", net="owned", point={"pad": ["first", "1"]}, layer="F.Cu",
            pads=[["first", "1"], ["second", "1"]])],
        protected_regions=[dict(id="immutable-corridor", layers=["F.Cu"], vertices=region,
            block_tracks=True, block_vias=True, block_zones=False)], keepouts=[],
        required_layers=["F.Cu", "B.Cu"], allowed_rotations=[0], internal_clearance_nm=0,
        unresolved=["Synthetic test only; not vendor geometry or hardware qualification"])
    raw = (json.dumps(asset, sort_keys=True) + "\n").encode()
    path = tmp_path / "synthetic-owner.json"
    path.write_bytes(raw)
    return bind_hard_macro(board, path, expected_sha256=sha256(raw).hexdigest(), name="protected-owner",
        bindings={"first": "OWNER_A", "second": "OWNER_B"}, net_bindings={"owned": "OWNER_NET"})


def test_early_plane_contacts_preserve_specialized_coupled_pair_routing():
    from pcbir.critical import route_critical_nets
    from pcbir.plane import PlaneStitchOptions
    board = constrained_contact_board()
    package = PhysicalFootprint("synthetic-pair", (
        FootprintPad("1", Point.mm(0, "-0.5"), Size.mm("0.4", "0.4")),
        FootprintPad("2", Point.mm(0, "0.5"), Size.mm("0.4", "0.4"))), Size.mm(1, 2))
    board = replace(board, footprints={**board.footprints, package.name: package},
        placements=(*board.placements,
            Placement("PAIR_A", package.name, Point.mm(7, 12)),
            Placement("PAIR_B", package.name, Point.mm(13, 12))),
        nets=(*board.nets, *(PhysicalNet(net, tuple(PadReference(ref, pad)
            for ref in ("PAIR_A", "PAIR_B"))) for net, pad in (("DP", "1"), ("DM", "2")))),
        net_routing_rules=tuple(NetRoutingRule(net, RouteKind.DIFFERENTIAL,
            width_nm=nm_from_mm("0.25"), differential_partner=partner,
            pair_gap_nm=nm_from_mm("0.2"), max_skew_nm=nm_from_mm(10),
            maximum_uncoupled_length_nm=nm_from_mm(10))
            for net, partner in (("DP", "DM"), ("DM", "DP"))))
    assert not hard_findings(board)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    ordinary_order = route_critical_nets(board, guides)
    result = route_critical_nets(board, guides, plane_stitch_options=PlaneStitchOptions())
    assert result.nets == ordinary_order.nets
    assert result.nets[0].connected and result.nets[0].strategy == "aligned_pair"
    assert result.nets[0].coupled_length_nm > 0
    assert result.plane_reservation_via_count == 1 and not result.reserved_via_count
    assert all(t in result.locked_tracks for t in ordinary_order.locked_tracks)
    assert all(v in result.locked_vias for v in result.board.vias if v.net == "RAIL")
    assert not hard_findings(result.board) and not result.board.zone_fills
    assert result == route_critical_nets(board, guides, plane_stitch_options=PlaneStitchOptions())
