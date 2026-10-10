"""Layer-aware, explicitly owned boundary hand-offs to detailed routing."""
from collections import Counter
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

import pcbir.detailed as detail
from pcbir.boundary_access import analyze_boundary_access, reserve_boundary_access
from pcbir.drc import explicit_copper_connectivity, physical_board_digest, run_physical_drc
from pcbir.physical import CopperLayer, NetRoutingRule, PadReference, PhysicalNet, Point, Stackup, TrackSegment, nm_from_mm
from pcbir.pin_escape import RoutingAccess, verified_routing_access
from pcbir.routing import route_global
from pcbir.routing_clearance import RoutingClearanceIndex
from test_boundary_access import fixture, ring, hard_errors


def reserve(*, back=False):
    fan = fixture()
    if back:
        fan = replace(fan, board=replace(fan.board, copper_keepouts=ring((CopperLayer.FRONT,))))
    proof = analyze_boundary_access(fan.board, fan)
    return fan, proof, reserve_boundary_access(fan.board, fan, proof)


def run(owned, **kwargs):
    source = replace(owned.board, tracks=(), vias=())
    settings = dict(fanout_accesses=owned.routing_accesses,
        fanout_created_tracks=owned.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in owned.created_vias))
    return detail.route_detailed(owned.board, route_global(source),
        detail.DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=2),
        **(settings | kwargs))


def test_materialization_keeps_launch_identity_and_accounts_new_owned_occurrences():
    fan, proof, owned = reserve(back=True)
    pad = PadReference("U", "1")
    assert owned.accesses == fan.accesses and not fan.boundary_accesses
    assert owned.routing_accesses[pad].position == proof.ports[0].position
    assert owned.routing_accesses[pad].layer is CopperLayer.BACK
    added = Counter(owned.board.tracks)-Counter(fan.board.tracks)
    assert added == Counter(owned.created_tracks)-Counter(fan.created_tracks)
    assert owned.added_track_count == len(owned.created_tracks)
    assert owned.created_vias == fan.created_vias
    assert not hard_errors(owned.board)


def test_detailed_search_starts_at_exact_boundary_layer_not_dogbone_via(monkeypatch):
    fan, proof, owned = reserve(back=True)
    anchor = owned.routing_accesses[PadReference("U", "1")]
    searches = []
    real = detail._search
    def check(grid, starts, targets, *args, **kwargs):
        searches.append((tuple(starts), tuple(targets)))
        # Sorted pad identity can make the package port either source or target.
        ports = tuple(n for n in (*starts, *targets) if grid.point(n) == anchor.position)
        assert len(ports) == 1 and grid.layers[ports[0].layer_index] is anchor.layer
        endpoint = starts if ports[0] in starts else targets
        assert len(endpoint) == 1
        assert anchor.position != fan.accesses[PadReference("U", "1")]
        return real(grid, starts, targets, *args, **kwargs)
    monkeypatch.setattr(detail, "_search", check)
    result = run(owned)
    assert searches and result.nets[0].connected, result.nets
    assert explicit_copper_connectivity(result.board).net_connected(result.board.nets[0])
    assert Counter(owned.created_tracks) <= Counter(result.board.tracks)
    assert not hard_errors(result.board)


@pytest.mark.parametrize("defect", ["moved", "rules", "extra_copper", "missing_launch", "incomplete", "duplicate", "already_owned", "missing_collar", "inside_collar", "wrong_edge"])
def test_reservation_rejects_stale_incomplete_or_repeated_evidence_atomically(defect):
    fan, proof, owned = reserve()
    source = fan.board
    if defect == "moved":
        source = replace(source, placements=(replace(source.placements[0], position=Point.mm(7,6)), *source.placements[1:]))
    elif defect == "rules":
        source = replace(source, rules=replace(source.rules, minimum_clearance_nm=nm_from_mm(.3)))
    elif defect == "extra_copper":
        source = replace(source, tracks=(*source.tracks, TrackSegment("A", Point.mm(14,2), Point.mm(15,2), nm_from_mm(.25), CopperLayer.FRONT)))
    elif defect == "missing_launch":
        source = replace(source, tracks=())
    elif defect == "incomplete":
        proof = replace(proof, ports=(), pending_pads=(PadReference("U","1"),))
    elif defect == "duplicate":
        proof = replace(proof, ports=(*proof.ports,*proof.ports))
    elif defect == "missing_collar":
        proof = replace(proof, collars=())
    elif defect == "inside_collar":
        port = proof.ports[0]
        end = Point.mm(7,6)
        proof = replace(proof, ports=(replace(port, position=end,
            path=(replace(port.path[0], end=end),)),))
    elif defect == "wrong_edge":
        proof = replace(proof, ports=(replace(proof.ports[0], edge="invalid"),))
    else:
        fan = owned
        source = owned.board
        proof = replace(proof, source_digest=physical_board_digest(source))
    before = source
    with pytest.raises(ValueError):
        reserve_boundary_access(source, fan, proof)
    assert source == before


def test_existing_input_track_never_acquires_boundary_cleanup_ownership():
    fan, proof, _ = reserve()
    existing = proof.ports[0].path[0]
    board = replace(fan.board, tracks=(*fan.board.tracks, existing))
    proof = analyze_boundary_access(board, fan)
    owned = reserve_boundary_access(board, fan, proof)
    assert existing in owned.board.tracks and existing not in owned.created_tracks


def test_materialization_requires_fresh_native_acceptance_and_rolls_back(monkeypatch):
    import pcbir.boundary_access as boundary
    from pcbir.drc import run_physical_drc
    fan, proof, _ = reserve()
    before = fan.board
    report = run_physical_drc(before)
    finding = next(f for f in report.findings if f.severity.value == "error")
    rejected = replace(report, findings=(replace(finding, code="DRC-FORCED-RESERVATION-FAILURE"),))
    monkeypatch.setattr(boundary, "run_physical_drc", lambda _: rejected)
    with pytest.raises(ValueError, match="native DRC"):
        reserve_boundary_access(before, fan, proof)
    assert fan.board == before and fan.boundary_accesses is None


def test_cleanup_preserves_duplicate_input_occurrences_and_boundary_ownership(monkeypatch):
    fan, proof, _ = reserve()
    protected = proof.ports[0].path[0]
    board = replace(fan.board, tracks=(*fan.board.tracks, protected, protected))
    proof = analyze_boundary_access(board, fan)
    owned = reserve_boundary_access(board, fan, proof)
    monkeypatch.setattr(detail, "_route_net", lambda *args, **kwargs: detail._failed("A", "bounded failure"))
    result = run(owned)
    # A failed area route must retain its verified launch for a later repair,
    # including the two pre-existing occurrences that it never owned.
    assert not result.nets[0].connected
    assert Counter(result.board.tracks) == Counter(owned.board.tracks)
    assert Counter(result.board.tracks)[protected] == 2
    assert result.board.vias == owned.board.vias


@pytest.mark.parametrize("defect", ["missing_path", "claimed_layer", "wrong_end", "reversed", "missing_via"])
def test_invalid_layer_or_path_does_not_fall_back_to_pad_center(defect):
    _, _, owned = reserve(back=True)
    pad = PadReference("U","1")
    anchor = owned.routing_accesses[pad]
    board = owned.board
    if defect == "missing_path":
        board = replace(board, tracks=tuple(t for t in board.tracks if t not in anchor.path))
    elif defect == "claimed_layer":
        anchor = replace(anchor, layer=CopperLayer.FRONT)
    elif defect == "wrong_end":
        anchor = replace(anchor, position=Point.mm(14,5))
    elif defect == "reversed":
        anchor = replace(anchor, path=tuple(replace(t,start=t.end,end=t.start) for t in reversed(anchor.path)))
    else:
        board = replace(board, vias=())
    assert verified_routing_access(board,pad,"A",anchor,RoutingClearanceIndex(board)) is None
    result = run(replace(owned, board=board, boundary_accesses={pad:anchor}))
    assert not result.nets[0].connected and "unverified boundary anchor" in result.nets[0].diagnostics[0]


def test_surface_port_survives_cleanup_of_unnecessary_via_and_subset_repair():
    _, _, owned = reserve()
    pad = PadReference("U","1")
    assert owned.routing_accesses[pad].layer is CopperLayer.FRONT
    result = run(owned)
    assert result.nets[0].connected and not result.board.vias
    assert verified_routing_access(result.board,pad,"A",owned.routing_accesses[pad],RoutingClearanceIndex(result.board))
    # No area copper is reused in this repair: only the exact pad/port path.
    source = replace(owned.board,vias=())
    repaired = run(replace(owned,board=source),only_nets=frozenset({"A"}))
    assert repaired.nets[0].connected and not hard_errors(repaired.board)


def test_failed_full_route_preserves_owned_boundary_and_dogbone_for_repair(monkeypatch):
    fan, _, owned = reserve(back=True)
    protected = TrackSegment("A", Point.mm(14,2), Point.mm(15,2), nm_from_mm(.25), CopperLayer.FRONT)
    owned = replace(owned,board=replace(owned.board,tracks=(*owned.board.tracks,protected)))
    monkeypatch.setattr(detail,"_route_net",lambda *args,**kwargs: detail._failed("A","bounded failure"))
    result = run(owned)
    assert not result.nets[0].connected
    assert Counter(result.board.tracks) == Counter(owned.board.tracks)
    assert result.board.vias == owned.board.vias
    assert protected in result.board.tracks
    assert any(f.code == "DRC-OPEN-NET" for f in run_physical_drc(result.board).findings)


def test_failed_subset_keeps_immutable_boundary_prefix_without_cleanup_ownership(monkeypatch):
    _, _, owned = reserve(back=True)
    monkeypatch.setattr(detail, "_route_net", lambda *args, **kwargs: detail._failed("A", "bounded failure"))
    result = run(owned, only_nets=frozenset({"A"}),
                 fanout_created_tracks=(), fanout_created_vias=frozenset())
    assert not result.nets[0].connected
    assert result.board.tracks == owned.board.tracks and result.board.vias == owned.board.vias


def test_inner_layer_port_requires_actual_via_span():
    _, _, owned = reserve()
    pad = PadReference("U", "1")
    anchor = owned.routing_accesses[pad]
    layer = CopperLayer.INTERNAL_2
    anchor = replace(anchor, layer=layer, path=tuple(replace(t, layer=layer) for t in anchor.path))
    board = replace(owned.board,
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1, layer, CopperLayer.BACK)),
        tracks=tuple(t for t in owned.board.tracks if t not in owned.routing_accesses[pad].path)+anchor.path,
        vias=tuple(replace(v, to_layer=CopperLayer.INTERNAL_1) for v in owned.board.vias))
    assert verified_routing_access(board, pad, "A", anchor, RoutingClearanceIndex(board)) is None
    board = replace(board, vias=tuple(replace(v, to_layer=CopperLayer.BACK) for v in board.vias))
    assert verified_routing_access(board, pad, "A", anchor, RoutingClearanceIndex(board))


@pytest.mark.parametrize("defect", ["forbidden_layer", "narrow_path", "narrow_launch", "wrong_net"])
def test_port_rechecks_net_rules_and_exact_path_identity(defect):
    _, _, owned = reserve(back=True)
    pad = PadReference("U", "1")
    anchor, board = owned.routing_accesses[pad], owned.board
    if defect == "forbidden_layer":
        board = replace(board, net_routing_rules=(NetRoutingRule("A", allowed_layers=(CopperLayer.FRONT,)),))
    elif defect == "narrow_launch":
        board = replace(board, tracks=tuple(replace(t, width_nm=board.rules.minimum_track_width_nm)
                                            if t not in anchor.path else t for t in board.tracks))
    else:
        path = tuple(replace(t, width_nm=board.rules.minimum_track_width_nm) if defect == "narrow_path"
                     else replace(t, net="B") for t in anchor.path)
        board = replace(board, tracks=tuple(t for t in board.tracks if t not in anchor.path)+path,
                        nets=(*board.nets, PhysicalNet("B", ())) if defect == "wrong_net" else board.nets)
        anchor = replace(anchor, path=path)
    assert verified_routing_access(board, pad, "A", anchor, RoutingClearanceIndex(board)) is None


def test_installed_kicad_accepts_complete_boundary_to_area_route():
    from pcbir.plane_verify import verify_filled_planes
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("KiCad not installed")
    _, _, owned = reserve(back=True)
    result = run(owned)
    assert result.nets[0].connected
    evidence = verify_filled_planes(result.board,kicad_cli=Path(cli))
    assert evidence.passed,evidence.findings
