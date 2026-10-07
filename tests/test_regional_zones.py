"""Source-to-physical regional pours, with no fabricated filled connectivity."""
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, KiCadPcbBackend, Point,
                   compile_source, prototype_physicalize)
from pcbir.quantities import millimeters
from pcbir.zone_geometry import lower_zone_outline


def board(parameters, declarations=""):
    return prototype_physicalize(compile_source('''board Regional {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component C1: CAPACITOR { footprint = "0402"; }
        net V3V3 { R1.1; C1.1; }
        net GND { R1.2; C1.2; }
        constraint copper_zone(V3V3) { layers = "F.Cu"; ''' + parameters + " }\n" + declarations + "}"))


@pytest.mark.parametrize("parameters", [
    'x = 4mm; y = 4mm; width = 31mm; height = 24mm;',
    'polygon_mm = "4,4;35,4;35,28;4,28";',
])
def test_regional_zone_source_exports_requested_geometry(parameters):
    physical = board(parameters + ' priority = 3; pad_connection = solid;')
    zone = physical.zones[0]
    assert set(zone.outline.outer.vertices) == {Point.mm(4, 4), Point.mm(35, 4), Point.mm(35, 28), Point.mm(4, 28)}
    assert zone.layers == (CopperLayer.FRONT,) and zone.priority == 3
    assert not physical.zone_fills and physical.metadata["fabrication_ready"] == "false"
    exported = KiCadPcbBackend().generate(physical).artifacts[0].content
    assert "(priority 3)" in exported and '(net_name "V3V3")' in exported


def test_named_region_resolves_even_when_declared_after_zone():
    physical = board('region = "logic"; inset = 1mm;', '''
        constraint placement_region(R1, C1) {
            name = "logic"; x = 4mm; y = 4mm; width = 31mm; height = 24mm;
        }''')
    assert set(physical.zones[0].outline.outer.vertices) == {Point.mm(5, 5), Point.mm(34, 5), Point.mm(34, 27), Point.mm(5, 27)}


@pytest.mark.parametrize("parameters, message", [
    ('region = "missing";', "unknown or ambiguous"),
    ('polygon_mm = "4,4;35,4;35,28;4,28"; x = 4mm;', "mutually exclusive"),
    ('x = 4mm; y = 4mm; width = 31mm;', "requires x"),
    ('polygon_mm = "4,4;35,4";', "invalid copper_zone"),
    ('polygon_mm = "4,4;35,4;4,28;35,28";', "invalid copper_zone"),
    ('polygon_mm = "-4,4;35,4;35,28;4,28";', "inside the board"),
    ('polygon_mm = "4,4;35,4;30,20;4,28"; inset = 1mm;', "nonzero copper_zone inset"),
    ('priority = 1.5;', "nonnegative integer"),
    ('priority = -1;', "nonnegative integer"),
])
def test_ambiguous_or_invalid_regional_zones_fail(parameters, message):
    with pytest.raises(ValueError, match=message):
        board(parameters)


def test_default_zone_remains_whole_outline_and_inset_is_exact():
    physical = board('inset = 0.5mm;')
    assert set(physical.zones[0].outline.outer.vertices) == {
        Point.mm(.5, .5), Point.mm(99.5, .5), Point.mm(99.5, 79.5), Point.mm(.5, 79.5)}


def test_zone_cannot_cross_concave_board_void_between_vertices():
    outline = BoardOutline(tuple(Point.mm(x, y) for x, y in (
        (0, 0), (20, 0), (20, 20), (12, 20), (12, 8), (8, 8), (8, 20), (0, 20))))
    with pytest.raises(ValueError, match="inside the board"):
        lower_zone_outline({"polygon_mm": "4,4;16,4;16,16;4,16"}, outline, ())


def _reserved_board(layer=CopperLayer.FRONT):
    from pcbir import CopperZone, PhysicalNet, PolygonRing, PolygonWithHoles, NetRoutingRule, Stackup
    from test_routing import _two_terminal_board
    physical = _two_terminal_board()
    return replace(physical, nets=(*physical.nets, PhysicalNet("POWER", ())),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        net_routing_rules=(NetRoutingRule("SIGNAL", allowed_layers=(CopperLayer.FRONT,), max_vias=0),),
        zones=(CopperZone("power-region", "POWER", (layer,),
            PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in ((15, 10), (25, 10), (25, 20), (15, 20))))),
            reserve_routing=True),))


def test_source_reservation_is_opt_in_boolean_and_preserved_in_intent():
    from pcbir.serializer import board_to_dict
    physical = board('reserve_routing = true;')
    assert physical.zones[0].reserve_routing
    assert not board('').zones[0].reserve_routing
    with pytest.raises(ValueError, match="must be boolean"):
        board('reserve_routing = "true";')
    source = compile_source('''board Intent { use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        net POWER { R1.1; }
        constraint copper_zone(POWER) { layers = "B.Cu"; reserve_routing = true; }
    }''')
    assert board_to_dict(source)["constraints"][0]["parameters"]["reserve_routing"] is True


@pytest.mark.parametrize("layer", [CopperLayer.FRONT, CopperLayer.INTERNAL_2, CopperLayer.BACK])
def test_reserved_region_checks_track_width_and_clearance_but_allows_owner_and_vias(layer):
    from pcbir import TrackSegment, Via, nm_from_mm
    from pcbir.geometry import RoundedConvexShape
    from pcbir.routing_clearance import RoutingClearanceIndex
    physical = _reserved_board(layer)
    index = RoutingClearanceIndex(physical)
    start, end = Point.mm(10, 15), Point.mm(30, 15)
    width = nm_from_mm(.2)
    assert not index.can_track("SIGNAL", start, end, width, layer)
    assert index.blocking_track_nets(TrackSegment("SIGNAL", start, end, width, layer)) == (frozenset(), True)
    assert not index.can_area("SIGNAL", RoundedConvexShape((start, end), width // 2), layer)
    assert index.can_track("POWER", start, end, width, layer)
    other_layer = CopperLayer.BACK if layer is CopperLayer.FRONT else CopperLayer.FRONT
    assert index.can_track("SIGNAL", start, end, width, other_layer)
    margin = physical.rules.minimum_clearance_nm + width // 2
    tangent = 10_000_000 - margin
    assert index.can_track("SIGNAL", Point(10_000_000, tangent), Point(30_000_000, tangent), width, layer)
    assert not index.can_track("SIGNAL", Point(10_000_000, tangent + 1), Point(30_000_000, tangent + 1), width, layer)
    via = Via("SIGNAL", Point.mm(20, 15), nm_from_mm(.6), nm_from_mm(.3))
    assert index.can_via(via.net, via.position, via.size_nm, via.from_layer, via.to_layer, via.drill_nm)
    assert index.blocking_via_nets(via) == (frozenset(), False)
    legacy = RoutingClearanceIndex(replace(physical, zones=(replace(physical.zones[0], reserve_routing=False),)))
    assert legacy.can_track("SIGNAL", start, end, width, layer)


def test_zone_and_both_net_clearances_apply_to_bounded_terminal_access():
    from pcbir import NetRoutingRule, nm_from_mm
    from pcbir.pin_access import local_access_path
    from pcbir.routing_clearance import RoutingClearanceIndex
    physical = _reserved_board()
    for zone_clearance, own_clearance, foreign_clearance in ((.6, .2, .2), (.2, .6, .2), (.2, .2, .6)):
        physical = replace(physical,
            zones=(replace(physical.zones[0], clearance_nm=nm_from_mm(zone_clearance)),),
            net_routing_rules=(NetRoutingRule("POWER", clearance_nm=nm_from_mm(own_clearance)),
                               NetRoutingRule("SIGNAL", clearance_nm=nm_from_mm(foreign_clearance))))
        index = RoutingClearanceIndex(physical)
        assert not index.can_track("SIGNAL", Point.mm(10, 9.4), Point.mm(30, 9.4), nm_from_mm(.2), CopperLayer.FRONT)
        assert index.can_track("SIGNAL", Point.mm(10, 9.3), Point.mm(30, 9.3), nm_from_mm(.2), CopperLayer.FRONT)
    assert local_access_path(physical, index, "SIGNAL", Point.mm(14, 15), Point.mm(16, 15),
        CopperLayer.FRONT, nm_from_mm(.2), step_nm=nm_from_mm(.5), detour_nm=nm_from_mm(1), maximum_states=100) is None
    assert local_access_path(physical, index, "POWER", Point.mm(14, 15), Point.mm(16, 15),
        CopperLayer.FRONT, nm_from_mm(.2), step_nm=nm_from_mm(.5), detour_nm=nm_from_mm(1), maximum_states=100)


def test_reservation_handles_concave_polygon_and_hole_without_bounding_box_block():
    from pcbir import PolygonRing, PolygonWithHoles, nm_from_mm
    from pcbir.routing_clearance import RoutingClearanceIndex
    physical = _reserved_board()
    outline = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((10, 5), (30, 5), (30, 10), (20, 10), (20, 25), (10, 25)))),
        (PolygonRing(tuple(Point.mm(x, y) for x, y in ((12, 12), (18, 12), (18, 18), (12, 18)))),))
    index = RoutingClearanceIndex(replace(physical, zones=(replace(physical.zones[0], outline=outline),)))
    assert index.can_track("SIGNAL", Point.mm(13, 15), Point.mm(17, 15), nm_from_mm(.2), CopperLayer.FRONT)
    assert not index.can_track("SIGNAL", Point.mm(13, 15), Point.mm(21, 15), nm_from_mm(.2), CopperLayer.FRONT)
    assert index.can_track("SIGNAL", Point.mm(25, 15), Point.mm(30, 15), nm_from_mm(.2), CopperLayer.FRONT)


def test_distinct_rail_reservations_cannot_overlap_but_layer_and_owner_are_scoped():
    from pcbir import PhysicalNet
    physical = _reserved_board()
    original = physical.zones[0]
    other = replace(original, id="other", net="OTHER")
    with pytest.raises(ValueError, match="overlap or touch"):
        replace(physical, nets=(*physical.nets, PhysicalNet("OTHER", ())), zones=(original, other))
    replace(physical, zones=(original, replace(other, net="POWER")))
    replace(physical, nets=(*physical.nets, PhysicalNet("OTHER", ())),
            zones=(original, replace(other, layers=(CopperLayer.BACK,))))
    replace(physical, nets=(*physical.nets, PhysicalNet("OTHER", ())),
            zones=(original, replace(other, reserve_routing=False)))


def test_reservation_invalidates_fingerprints_and_final_drc_checks_existing_tracks():
    from pcbir import TrackSegment, nm_from_mm, run_physical_drc
    from pcbir.drc import physical_board_digest, placement_copper_findings
    from pcbir.routing import _placement_fingerprint
    physical = _reserved_board()
    legacy = replace(physical, zones=(replace(physical.zones[0], reserve_routing=False),))
    assert physical_board_digest(physical) != physical_board_digest(legacy)
    assert _placement_fingerprint(physical) != _placement_fingerprint(legacy)
    track = TrackSegment("SIGNAL", Point.mm(10, 15), Point.mm(30, 15), nm_from_mm(.2), CopperLayer.FRONT)
    physical = replace(physical, tracks=(track,))
    assert any(f.code == "DRC-ZONE-RESERVATION" for f in placement_copper_findings(physical))
    assert any(f.code == "DRC-ZONE-RESERVATION" for f in run_physical_drc(physical).findings)
    export = KiCadPcbBackend().generate(physical).artifacts[0].content
    assert "(keepout" not in export  # Native pours keep normal owner connectivity and antipads.


@pytest.mark.parametrize("rotation", [0, 90])
def test_reservation_validates_rotated_macro_copper_before_acceptance(tmp_path, rotation):
    from pcbir import CopperZone, PhysicalNet, PolygonRing, PolygonWithHoles
    from pcbir.clusters import cluster_placements
    from pcbir.hard_macros import materialize_hard_macros
    from test_hard_macros import fixture
    original, _, bind = fixture(tmp_path)
    outline = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((9, 9), (13, 9), (13, 13), (9, 13)))))
    source = replace(original, nets=(*original.nets, PhysicalNet("POWER", ())),
        zones=(CopperZone("private-power", "POWER", (CopperLayer.FRONT,), outline, reserve_routing=True),))
    bound = bind(input_board=source)
    poses = {p.reference: p for p in bound.placements}
    poses.update(cluster_placements(bound, bound.rigid_clusters[0], replace(poses["U1"], rotation_degrees=rotation)))
    bound = replace(bound, placements=tuple(poses.values()))
    with pytest.raises(ValueError, match="DRC-ZONE-RESERVATION"):
        materialize_hard_macros(bound)
    owner_region = replace(bound, zones=(replace(bound.zones[0], net="N"),))
    assert materialize_hard_macros(owner_region).materialized_macros


def test_global_and_detailed_routing_detour_around_reserved_region():
    from pcbir import route_global, route_detailed, GlobalRouterOptions, DetailedRouterOptions, nm_from_mm
    from pcbir.geometry import RoundedConvexShape
    from pcbir.zone_geometry import ZoneRoutingReservations
    physical = _reserved_board()
    global_route = route_global(physical, GlobalRouterOptions(tile_size_nm=nm_from_mm(2), maximum_iterations=2))
    route = next(r for r in global_route.routes if r.net == "SIGNAL")
    assert route.connected
    reservations = ZoneRoutingReservations(physical)
    for segment in route.segments:
        assert reservations.can_area("SIGNAL", RoundedConvexShape((segment.start, segment.end), nm_from_mm(.125)), segment.layer)
    detailed = route_detailed(physical, global_route, DetailedRouterOptions(pitch_nm=nm_from_mm(.5),
                              enable_soft_ripup=True, route_smoothing=True))
    assert next(r for r in detailed.nets if r.net == "SIGNAL").connected
    assert all(reservations.can_area(t.net, RoundedConvexShape((t.start, t.end), t.width_nm // 2), t.layer)
               for t in detailed.board.tracks)


@pytest.mark.parametrize("sever", [False, True])
def test_native_fill_connects_surface_region_around_foreign_through_via_and_detects_severing(tmp_path, sever):
    import json
    import shutil
    import subprocess
    from pcbir import (CopperKeepout, CopperZone, IslandPolicy, PhysicalNet,
                       PolygonRing, PolygonWithHoles, TrackSegment, Via, nm_from_mm)
    from pcbir.drc import explicit_copper_connectivity
    from test_routing import _two_terminal_board
    cli = shutil.which("kicad-cli")
    if cli is None:
        pytest.skip("native zone verification requires kicad-cli")
    physical = _two_terminal_board()
    polygon = lambda x0, y0, x1, y1: PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))))
    physical = replace(physical,
        nets=(*physical.nets, PhysicalNet("FOREIGN", ())),
        # The copper clearance here is smaller than the drill clearance: native
        # fill must enlarge this via's antipad beyond the copper-only margin.
        rules=replace(physical.rules, minimum_clearance_nm=nm_from_mm(.1)),
        tracks=tuple(TrackSegment("SIGNAL", Point.mm(x, 15), Point.mm(x, 16), nm_from_mm(.25), CopperLayer.FRONT)
                     for x in (5, 35)),
        vias=tuple(Via("SIGNAL", Point.mm(x, 16), nm_from_mm(.6), nm_from_mm(.3)) for x in (5, 35))
              + (Via("FOREIGN", Point.mm(20, 15), nm_from_mm(.45), nm_from_mm(.2)),),
        zones=(CopperZone("power", "SIGNAL", (CopperLayer.BACK,), polygon(1, 1, 39, 29),
                         reserve_routing=True, island_policy=IslandPolicy.KEEP_ALL,
                         minimum_island_area_nm2=None),),
        copper_keepouts=(CopperKeepout("sever", (CopperLayer.BACK,), polygon(19, 0, 21, 30),
                            block_tracks=False, block_vias=False, block_zones=True),) if sever else ())
    assert not explicit_copper_connectivity(physical).net_connected(physical.nets[0])
    pcb = tmp_path / "board.kicad_pcb"
    pcb.write_text(KiCadPcbBackend().generate(physical).artifacts[0].content)
    report = tmp_path / "drc.json"
    subprocess.run([cli, "pcb", "drc", "--refill-zones", "--save-board", "--format", "json",
                    "--output", str(report), str(pcb)], check=True, capture_output=True, text=True, timeout=60)
    drc = json.loads(report.read_text())
    assert bool(drc["unconnected_items"]) is sever
    assert not any(v["type"] in {"shorting_items", "clearance", "hole_clearance"} for v in drc["violations"])
    assert "(filled_polygon" in pcb.read_text()
