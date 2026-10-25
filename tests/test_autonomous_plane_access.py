"""Real copper geometry, no project-specific repair selectors or search mocks."""
from dataclasses import replace
from collections import Counter
from hashlib import sha256
import json

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone, FootprintPad,
    GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size,
    Stackup, nm_from_mm, route_global, DetailedRouterOptions,
    PlacementPlannerOptions, PlacementRoutingFeedbackOptions)
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions
from pcbir.flow import run_routing_pipeline
from pcbir.package_access import PackageAccessOptions, preflight_package_access
from pcbir.clusters import footprint_geometry_digest
from pcbir.hard_macros import bind_hard_macro, materialize_hard_macros, validate_hard_macros
from pcbir.pad_via_arrays import require_via_in_pad_arrays, via_in_pad_array_vias
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


@pytest.mark.parametrize("rotation", (0, 90, 180, 270))
@pytest.mark.parametrize("renamed", (False, True))
def test_scarce_plane_access_negotiated_without_board_selectors(rotation, renamed):
    board = constrained_contact_board(rotation, renamed)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    baseline = preflight_package_access(board, guides, settings(),
        options=PackageAccessOptions(maximum_pattern_trials=0))
    if rotation == 0:
        assert baseline.plane_stitch.pending_pads
    events = []
    result = preflight_package_access(board, guides, settings(),
        on_progress=lambda *event: events.append(event))
    assert result.ready
    assert result.source == board
    assert result.board.placements == board.placements
    if baseline.pending_pads:
        assert result.pattern_trials[0].outcome == "accepted"
        assert result.pattern_trials[0].revalidated
        assert any(p == "package_pattern_trial" and e == "finished"
                   and d["outcome"] == "accepted" for p, e, d in events)
    assert not hard_findings(result.board)
    assert not result.board.zone_fills
    assert result.board.metadata["fabrication_ready"] == "false"
    assert result == preflight_package_access(board, guides, settings())


def test_standard_pipeline_preserves_negotiated_contact_through_area_routing():
    board = constrained_contact_board()
    result = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        fanout_options=settings(), package_access_options=PackageAccessOptions(maximum_trials=0),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"),
            maximum_passes=1, defer_zone_nets=True))
    assert result.package_access.ready
    assert result.package_access.pattern_trials[0].outcome == "accepted"
    assert next(n for n in result.detailed.nets if n.net == "CLOCK").connected
    contacts = result.plane_stitch.board
    assert all(t in result.board.tracks for t in contacts.tracks if t.net == "RAIL")
    assert all(v in result.board.vias for v in contacts.vias if v.net == "RAIL")
    assert not hard_findings(result.board)
    assert not result.board.zone_fills


def test_impossible_contact_stops_area_and_preserves_failure_evidence():
    board = constrained_contact_board()
    board = replace(board, copper_keepouts=(CopperKeepout("no-contact-via",
        board.stackup.copper_layers, board.zones[0].outline,
        block_tracks=False, block_vias=True, block_zones=False),))
    result = run_routing_pipeline(board, legacy_gates=True,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        fanout_options=settings(), package_access_options=PackageAccessOptions(maximum_trials=0))
    assert not result.package_access.ready
    assert result.plane_stitch.pending_pads
    assert result.detailed.metrics.passes == 0
    assert result.board.metadata["fabrication_ready"] == "false"
    assert all(t.outcome != "accepted" for t in result.package_access.pattern_trials)
    assert result.package_access.source == board


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


def owned_pipeline(board, legacy_gates=False):
    return run_routing_pipeline(board, legacy_gates=legacy_gates,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        fanout_options=settings(), package_access_options=PackageAccessOptions(maximum_trials=0),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"),
            maximum_passes=1, defer_zone_nets=True))


@pytest.mark.parametrize("required_array", (False, True))
@pytest.mark.parametrize("renamed", (False, True))
def test_negotiation_and_area_routing_preserve_exact_owner_copper(tmp_path, required_array, renamed):
    board = with_test_owners(constrained_contact_board(renamed=renamed), tmp_path,
                             required_array=required_array)
    owner = materialize_hard_macros(board)
    assert not hard_findings(owner)
    result = owned_pipeline(board)
    assert result.package_access.ready
    assert result.package_access.pattern_trials[0].outcome == "accepted"
    assert result.board.placements == board.placements
    signal = "DATA_X" if renamed else "CLOCK"
    assert next(n for n in result.detailed.nets if n.net == signal).connected
    assert not hard_findings(result.board)
    assert Counter(owner.tracks) <= Counter(result.board.tracks)
    assert Counter(owner.vias) <= Counter(result.board.vias)
    assert result.board.hard_macros == owner.hard_macros
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "autonomous owner regression")
    if required_array:
        arrays = Counter(via_in_pad_array_vias(board))
        assert sum(arrays.values()) == 4
        actual = Counter(result.board.vias)
        assert all(actual[v] == count for v, count in arrays.items())
        assert all(v.finish == "filled-capped" for v in arrays)
    assert not result.board.zone_fills


def test_immutable_macro_blocking_contact_is_not_evicted(tmp_path):
    board = with_test_owners(constrained_contact_board(), tmp_path, blocking_macro=True,
                             required_array=True)
    owner = materialize_hard_macros(board)
    assert not hard_findings(owner)
    result = owned_pipeline(board, legacy_gates=True)
    assert not result.package_access.ready
    assert result.plane_stitch.pending_pads
    assert result.detailed.metrics.passes == 0
    assert result.package_access.pattern_trials
    assert all(t.outcome != "accepted" for t in result.package_access.pattern_trials)
    assert Counter(owner.tracks) <= Counter(result.board.tracks)
    assert Counter(owner.vias) <= Counter(result.board.vias)
    arrays = Counter(via_in_pad_array_vias(board))
    actual = Counter(result.board.vias)
    assert all(actual[v] == count for v, count in arrays.items())
    assert result.board.placements == board.placements
    assert result.board.hard_macros == board.hard_macros
    assert not hard_findings(result.board)
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "blocked autonomous owner regression")
    assert result.package_access.source == board
    assert result.board.metadata["fabrication_ready"] == "false"


def critical_contact_trap(*, renamed=False, maximum_length="12"):
    board = constrained_contact_board(renamed=renamed)
    refs = ("ENDPOINT_23", "ENDPOINT_48") if renamed else ("CRIT_A", "CRIT_B")
    net = "TIMING_X" if renamed else "TIMING"
    return replace(board, placements=(*board.placements,
        Placement(refs[0], "terminal", Point.mm(4, 3)),
        Placement(refs[1], "terminal", Point.mm(4, 10))),
        nets=(*board.nets, PhysicalNet(net, tuple(PadReference(ref, "1") for ref in refs))),
        net_routing_rules=(NetRoutingRule(net, RouteKind.CRITICAL, max_vias=0,
            allowed_layers=(CopperLayer.FRONT,), max_length_nm=nm_from_mm(maximum_length)),))


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("owned", (False, True))
def test_joint_pattern_revalidates_critical_ordinary_and_boundary_owners(tmp_path, renamed, owned):
    board = critical_contact_trap(renamed=renamed)
    if owned:
        board = with_test_owners(board, tmp_path, required_array=True)
    owner = materialize_hard_macros(board)
    assert not hard_findings(owner)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    first = preflight_package_access(board, guides, settings(),
        options=PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=1))
    # The current bounded local critical search admits this fixture in the
    # first joint pattern; a second ordering trial is no longer necessary.
    assert first.ready and not first.plane_stitch.pending_pads
    events = []
    result = preflight_package_access(board, guides, settings(),
        options=PackageAccessOptions(maximum_trials=0), on_progress=lambda *e: events.append(e))
    assert result.ready and result.boundary.native_accepted
    assert result.pattern_trials[-1].strategy == "critical_and_plane_first"
    assert result.pattern_trials[-1].outcome == "accepted" and result.pattern_trials[-1].revalidated
    assert len(result.pattern_trials) == 1
    # This ordering admits ordinary exits before the final plane stitch. Do
    # not relabel those exits as a plane-first reservation in the report.
    assert result.critical.plane_reservation_track_count == 0
    assert result.critical.plane_reservation_via_count == 0
    assert result.critical.reserved_track_count == len(result.fanout.created_tracks)
    assert result.critical.reserved_via_count == len(result.fanout.created_vias)
    report = json.loads(result.critical.to_json())
    assert report["plane_reservation_track_count"] == result.critical.plane_reservation_track_count
    assert report["plane_reservation_via_count"] == 0
    timing = "TIMING_X" if renamed else "TIMING"
    rail = "SUPPLY_Z" if renamed else "RAIL"
    assert all(n.connected for n in result.critical.nets)
    assert not any(v.net == timing for v in result.board.vias)
    assert all(t.layer is CopperLayer.FRONT for t in result.board.tracks if t.net == timing)
    assert all(t.net != rail for t in result.fanout.created_tracks)
    assert all(v.net != rail for v in result.fanout.created_vias)
    assert Counter(owner.tracks) <= Counter(result.board.tracks)
    assert Counter(owner.vias) <= Counter(result.board.vias)
    arrays = Counter(via_in_pad_array_vias(board))
    actual = Counter(result.board.vias)
    assert all(actual[v] == count for v, count in arrays.items())
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "plane-first all-owner regression")
    assert result.board.placements == board.placements and result.source == board
    assert not result.accepted_moves and not hard_findings(result.board)
    assert any(p == "package_critical_search" and e == "started" and not d["plane_contacts_first"]
               for p, e, d in events)
    assert not result.board.zone_fills and result.board.metadata["fabrication_ready"] == "false"


@pytest.mark.parametrize("renamed", (False, True))
def test_plane_first_cannot_trade_a_successful_critical_route_for_plane_access(renamed):
    # Seven millimetres allows only the direct critical path. Reserving the
    # contact would require a longer detour; never relax the declared limit.
    board = critical_contact_trap(renamed=renamed, maximum_length="7")
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    first = preflight_package_access(board, guides, settings(),
        options=PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=1))
    assert not first.ready and not first.failed_critical_nets
    result = preflight_package_access(board, guides, settings(),
        options=PackageAccessOptions(maximum_trials=0))
    assert not result.ready and not result.failed_critical_nets
    assert result.pattern_trials[-1].strategy == "plane_first"
    assert result.pattern_trials[-1].outcome != "accepted" and result.pattern_trials[-1].revalidated
    assert result.board == first.board and result.critical == first.critical
    assert result.fanout == first.fanout and result.plane_stitch == first.plane_stitch
    assert not result.critical.plane_reservation_via_count
    assert result.source == board and not hard_findings(result.board)


def test_pipeline_preserves_plane_first_ownership_through_area_routing():
    board = critical_contact_trap()
    result = owned_pipeline(board)
    assert result.package_access.ready
    assert result.package_access.pattern_trials[-1].strategy == "critical_and_plane_first"
    assert next(n for n in result.detailed.nets if n.net == "CLOCK").connected
    early = tuple(v for v in result.package_access.board.vias if v.net == "RAIL")
    assert early and all(v in result.board.vias for v in early)
    assert not hard_findings(result.board)


def test_critical_only_feedback_refuses_to_relabel_plane_reservations():
    from pcbir.critical import route_critical_nets
    from pcbir.critical_feedback import improve_critical_placement
    from pcbir.plane import PlaneStitchOptions
    board = critical_contact_trap()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    critical = route_critical_nets(board, guides, plane_stitch_options=PlaneStitchOptions())
    assert critical.plane_reservation_via_count and not critical.reserved_via_count
    with pytest.raises(ValueError, match="package-access controller"):
        improve_critical_placement(board, guides, critical, maximum_trials=0)
    with pytest.raises(ValueError, match="guides are stale"):
        route_critical_nets(board, replace(guides, placement_fingerprint="stale"),
                            plane_stitch_options=PlaneStitchOptions())


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
