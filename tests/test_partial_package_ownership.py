"""Partial ordinary-owner proposals use real clearance and routing geometry."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import (ComponentPlacementRule, GlobalRouterOptions, PadReference, PhysicalNet,
                   Placement, Point, nm_from_mm, route_global)
from pcbir import CopperKeepout, FootprintPad, PolygonRing, PolygonWithHoles, Size
from pcbir.physical import DecouplingLink, PhysicalFootprint, TrackSegment, CopperLayer
from pcbir.fanout import route_fanout
from pcbir.critical import route_critical_nets
from pcbir.hard_macros import materialize_hard_macros, validate_hard_macros
from pcbir.pad_via_arrays import require_via_in_pad_arrays, via_in_pad_array_vias
from pcbir.package_access import (PackageAccessOptions, _access_result, _improvement_outcome,
                                  _partial_ordinary_pattern, preflight_package_access)
import pcbir.package_access as access
from pcbir.drc import run_physical_drc
from test_package_access import pattern_trap
from test_autonomous_plane_access import with_test_owners


def fixture(tmp_path, renamed=False, owners=False, length_mm=2):
    board, _, settings = pattern_trap()
    if renamed:
        names = {"U": "PACKAGE_X", "JA": "REMOTE_X", "JB": "REMOTE_Y"}
        nets = {"A": "FEED_Q", "B": "SIGNAL_K"}
        board = replace(board,
            placements=tuple(replace(p, reference=names[p.reference]) for p in board.placements),
            nets=tuple(replace(n, name=nets[n.name], pads=tuple(
                PadReference(names[p.component], p.pad) for p in n.pads)) for n in board.nets),
            net_routing_rules=tuple(replace(r, net=nets[r.net]) for r in board.net_routing_rules),
            placement_rules=tuple(replace(r, reference=names[r.reference]) for r in board.placement_rules))
    # A remote ordinary owner must retain its original copper and access anchor.
    remote = "CLOCK_Z" if renamed else "C"
    board = replace(board,
        placements=(*board.placements, Placement("QUIET_PACKAGE", "two", Point.mm(5, 13)),
                    Placement("QUIET_REMOTE", "one", Point.mm(13, 14))),
        nets=(*board.nets, PhysicalNet(remote, (PadReference("QUIET_PACKAGE", "1"),
                                               PadReference("QUIET_REMOTE", "1")))))
    if owners:
        # Reuse synthetic ownership assets, not a vendor/reusable part definition.
        board = replace(board, footprints={**board.footprints, "terminal": board.footprints["one"]})
        board = with_test_owners(board, tmp_path, required_array=True)
    board = replace(board, placement_rules=tuple(ComponentPlacementRule(
        p.reference, fixed_position=p.position, allowed_orientations=(p.rotation_degrees,),
        edge_clearance_nm=nm_from_mm("0.25"))
        for p in board.placements))
    board = replace(board, net_routing_rules=tuple(replace(r, max_length_nm=nm_from_mm(length_mm))
                                                  for r in board.net_routing_rules))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    baseline = preflight_package_access(board, guides, settings,
        options=PackageAccessOptions(maximum_pattern_trials=0, reserve_plane_contacts=False))
    return board, guides, settings, baseline, remote


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("owners", (False, True))
def test_partial_proposal_rebuilds_intersecting_net_and_preserves_unaffected_ownership(tmp_path, renamed, owners):
    board, guides, settings, baseline, remote = fixture(tmp_path, renamed, owners)
    owner = materialize_hard_macros(board)
    probe = route_critical_nets(board, guides)
    proposed, displaced, track_count, via_count, outcome = _partial_ordinary_pattern(
        baseline.fanout, owner, probe.board, settings, 8)
    assert outcome == "proposed" and displaced == (("SIGNAL_K" if renamed else "B"),)
    assert track_count and via_count
    assert all(t in proposed.created_tracks for t in baseline.fanout.created_tracks if t.net == remote)
    assert all(v in proposed.created_vias for v in baseline.fanout.created_vias if v.net == remote)
    quiet = PadReference("QUIET_PACKAGE", "1")
    assert proposed.routing_accesses[quiet] == baseline.fanout.routing_accesses[quiet]
    assert proposed.escaped_pads == baseline.fanout.escaped_pads
    verified = route_critical_nets(board, guides, reserved_accesses=proposed)
    candidate = _access_result(board, guides, proposed, verified, None)
    assert candidate.ready and _improvement_outcome(baseline, candidate) == "accepted"
    assert candidate.board.placements == board.placements
    assert Counter(owner.tracks) <= Counter(candidate.board.tracks)
    assert Counter(owner.vias) <= Counter(candidate.board.vias)
    if owners:
        validate_hard_macros(candidate.board)
        require_via_in_pad_arrays(candidate.board, "partial proposal regression")
        assert len(via_in_pad_array_vias(candidate.board)) == 4
    assert not [f for f in run_physical_drc(candidate.board).findings if f.severity.value == "error"
                and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]
    assert (proposed, displaced, track_count, via_count, outcome) == _partial_ordinary_pattern(
        baseline.fanout, owner, probe.board, settings, 8)


@pytest.mark.parametrize("renamed", (False, True))
def test_partial_budget_does_not_remove_macro_or_array_owner(tmp_path, renamed):
    board, guides, settings, baseline, _ = fixture(tmp_path, renamed, owners=True)
    owner = materialize_hard_macros(board)
    before = baseline.board
    probe = route_critical_nets(board, guides)
    result = _partial_ordinary_pattern(baseline.fanout, owner, probe.board, settings, 0)
    assert result[0] is None and result[-1] == "partial_owner_budget_exhausted"
    assert baseline.board == before and baseline.source == board
    assert Counter(owner.vias) <= Counter(before.vias)
    validate_hard_macros(before)
    require_via_in_pad_arrays(before, "rejected partial regression")


@pytest.mark.parametrize("renamed", (False, True))
def test_partial_final_gate_rejects_unconnectable_source_without_owner_loss(tmp_path, renamed):
    board, guides, settings, baseline, _ = fixture(tmp_path, renamed, owners=True, length_mm=1)
    owner = materialize_hard_macros(board)
    probe = route_critical_nets(board, guides)
    proposed, _, _, _, _ = _partial_ordinary_pattern(baseline.fanout, owner, probe.board, settings, 8)
    candidate = _access_result(board, guides, proposed,
        route_critical_nets(board, guides, reserved_accesses=proposed), None)
    assert candidate.failed_critical_nets and _improvement_outcome(baseline, candidate) != "accepted"
    assert baseline.source == board and baseline.board.placements == board.placements
    validate_hard_macros(baseline.board)
    require_via_in_pad_arrays(baseline.board, "failed final partial gate")
    assert Counter(owner.tracks) <= Counter(baseline.board.tracks)
    assert Counter(owner.vias) <= Counter(baseline.board.vias)


@pytest.mark.parametrize("budget", (0, 8))
def test_partial_fallback_shares_pattern_budget_after_first_all_owner_search_exhaustion(tmp_path, monkeypatch, budget):
    board, guides, settings, baseline, _ = fixture(tmp_path)
    real = access.route_fanout
    calls = []
    def first_exhaustion(source, options, **kwargs):
        result = real(source, options, **kwargs)
        calls.append(kwargs)
        if len(calls) == 2:  # Initial escapes succeed; first all-owner proposal exhausts.
            quiet = PadReference("QUIET_PACKAGE", "1")
            return replace(result, accesses={p:v for p,v in result.accesses.items() if p != quiet},
                           pending_pads=(*result.pending_pads, quiet))
        return result
    monkeypatch.setattr(access, "route_fanout", first_exhaustion)
    result = preflight_package_access(board, guides, settings,
        options=PackageAccessOptions(reserve_plane_contacts=False, maximum_partial_owner_nets=budget))
    assert len(result.pattern_trials) == (2 if budget else 1)
    assert result.pattern_trials[0].outcome == "lost_ordinary_access"
    if budget:
        trial = result.pattern_trials[1]
        assert trial.strategy == "partial_ordinary" and trial.outcome == "accepted" and trial.revalidated
        assert trial.displaced_nets == ("B",) and trial.retained_track_count and trial.retained_via_count
        assert calls[-1] == {"only_nets": frozenset({"B"})}
        assert result.ready and result.source == board
    else:
        assert result.failed_critical_nets and result.board == baseline.board


def test_cli_partial_owner_budget_is_explicit_and_zero_disables():
    from pcbir.cli import _parser
    parser = _parser()
    assert parser.parse_args(["route-board", "board.copper"]).package_partial_owner_nets == 8
    assert parser.parse_args(["route-board", "board.copper", "--package-partial-owner-nets", "0"]).package_partial_owner_nets == 0
    with pytest.raises(SystemExit):
        parser.parse_args(["route-board", "board.copper", "--package-partial-owner-nets", "9"])


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("hits_feed", (False, True))
def test_actual_feed_copper_is_protected_without_locking_unrelated_same_net_escape(renamed, hits_feed):
    board, _, settings = pattern_trap()
    package = board.footprints["two"]
    package = replace(package, pads=(*package.pads,
        FootprintPad("3",Point.mm(0,"0.4"),Size.mm("0.3","0.3"))))
    capacitor = PhysicalFootprint("bypass", (
        FootprintPad("1",Point(0,0),Size.mm("0.3","0.3")),
        FootprintPad("2",Point.mm(0,1),Size.mm("0.3","0.3"))),Size.mm(1,2))
    names = {"A":"CLOCK_Q", "B":"SUPPLY_R", "GND":"RETURN_S"} if renamed else {n:n for n in ("A","B","GND")}
    board = replace(board, footprints={**board.footprints,"two":package,"bypass":capacitor},
        placements=tuple(replace(p,footprint="bypass") if p.reference=="JB" else p for p in board.placements),
        nets=(*tuple(replace(n,name=names[n.name],pads=(*n.pads,PadReference("U","3"))
                    if n.name=="B" else n.pads) for n in board.nets),
              PhysicalNet(names["GND"],(PadReference("JB","2"),))),
        net_routing_rules=tuple(replace(r,net=names[r.net]) for r in board.net_routing_rules),
        decoupling_links=(DecouplingLink(PadReference("JB","1"),PadReference("U","2"),
                           PadReference("JB","2"),names["B"],names["GND"]),))
    original = route_fanout(board, settings)
    assert original.decoupling_tracks and not original.decoupling_pending
    assert original.created_vias and all(v.net==names["B"] for v in original.created_vias)
    # A disposable clearance obstacle, not an asserted connected critical route.
    site = original.created_vias[0].position
    start,end = ((Point.mm("5.5","6.6"),Point.mm("7.5","6.6")) if hits_feed else
                 (Point(site.x_nm-100000,site.y_nm),Point(site.x_nm+100000,site.y_nm)))
    obstacle = replace(board, tracks=(TrackSegment(names["A"],start,end,100000,CopperLayer.FRONT),))
    proposed, displaced, _, _, reason = _partial_ordinary_pattern(original,board,obstacle,settings,8)
    assert displaced == (names["B"],)
    if hits_feed:
        assert proposed is None and reason == "protected_ordinary_owner"
        assert Counter(original.decoupling_tracks) <= Counter(original.created_tracks)
    else:
        assert proposed is not None and reason == "proposed" and not proposed.pending_pads
        assert Counter(original.decoupling_tracks) <= Counter(proposed.created_tracks)
        assert proposed.decoupling_tracks == original.decoupling_tracks
        assert proposed.created_vias != original.created_vias
        assert proposed.escaped_pads == original.escaped_pads
        assert proposed.board.placements == board.placements


@pytest.mark.parametrize("reason", ("protected_ordinary_owner", "partial_owner_budget_exhausted"))
def test_ineligible_partial_does_not_suppress_established_critical_only_fallback(tmp_path, monkeypatch, reason):
    board, guides, settings, baseline, _ = fixture(tmp_path, owners=True)
    real = access.route_fanout
    calls = []
    def exhausted_first(source, options, **kwargs):
        result = real(source, options, **kwargs)
        calls.append(kwargs)
        if len(calls) == 2:
            quiet = PadReference("QUIET_PACKAGE", "1")
            return replace(result, accesses={p:v for p,v in result.accesses.items() if p != quiet},
                           pending_pads=(*result.pending_pads, quiet))
        return result
    monkeypatch.setattr(access, "route_fanout", exhausted_first)
    real_access = access._access_result
    access_calls = []
    def initial_contact_exhaustion(*args, **kwargs):
        result = real_access(*args, **kwargs)
        access_calls.append(result)
        if len(access_calls) == 1:
            # Force an initial contact-search exhaustion so both normal owner
            # orders are eligible. Final contact geometry/acceptance stay real.
            return replace(result, plane_stitch=replace(result.plane_stitch,
                           pending_pads=(PadReference("ARRAY_OWNER", "EP"),)))
        return result
    monkeypatch.setattr(access, "_access_result", initial_contact_exhaustion)
    # Inject cheap eligibility rejection, not candidate geometry or route success.
    monkeypatch.setattr(access, "_partial_ordinary_release", lambda *args: (("B",), reason))
    result = preflight_package_access(board, guides, settings)
    assert result.ready and len(result.pattern_trials) == 2
    assert result.pattern_trials[0].strategy == "critical_and_plane_first"
    last = result.pattern_trials[1]
    assert last.strategy == "critical_first" and last.outcome == "accepted" and last.revalidated
    assert last.partial_skipped_reason == reason and len(calls) == 3 and calls[-1] == {}
    assert result.source == board and result.board.placements == board.placements
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "ineligible partial fallback")


@pytest.mark.parametrize("renamed", (False, True))
def test_real_partial_pattern_preserves_scarce_neighbor_after_all_owner_ordering_loses_it(renamed):
    board, _, settings = pattern_trap()
    package = board.footprints["two"]
    package = replace(package, pads=(*package.pads, FootprintPad("3", Point.mm(0, "0.4"), Size.mm("0.3", "0.3"))))
    board = replace(board, footprints={**board.footprints, package.name: package},
        placements=(*board.placements, Placement("JC", "one", Point.mm(13, 10))),
        nets=(*board.nets, PhysicalNet("C", (PadReference("U", "3"), PadReference("JC", "1")))))
    # Four actual via windows: ordinary B's preferred launch, its first and
    # alternate post-critical launches, and C's sole legal launch. All searches
    # and clearance checks are real; no patched candidate/connected flags.
    windows = tuple((p.x_nm-660000, p.y_nm-660000, p.x_nm+660000, p.y_nm+660000)
                    for p in (Point.mm(4,"6.6"), Point.mm(6,"6.6"), Point.mm(7,"6.6"), Point.mm(6,"7.4")))
    xs = sorted({0,16000000, *(x for w in windows for x in (w[0],w[2]))})
    ys = sorted({0,16000000, *(y for w in windows for y in (w[1],w[3]))})
    blocked = []
    for x, xx in zip(xs,xs[1:]):
        for y, yy in zip(ys,ys[1:]):
            if any(a <= x and xx <= c and b <= y and yy <= d for a,b,c,d in windows):
                continue
            blocked.append(CopperKeepout(f"via-window-{len(blocked)}", board.stackup.copper_layers,
                PolygonWithHoles(PolygonRing((Point(x,y),Point(xx,y),Point(xx,yy),Point(x,yy)))),
                block_tracks=False, block_vias=True, block_zones=False))
    board = replace(board, copper_keepouts=tuple(blocked))
    if renamed:
        refs = {p.reference:"OWNER_"+str(i) for i,p in enumerate(board.placements)}
        nets = {n.name:"WIRE_"+str(i) for i,n in enumerate(board.nets)}
        board = replace(board, placements=tuple(replace(p,reference=refs[p.reference]) for p in board.placements),
            nets=tuple(replace(n,name=nets[n.name],pads=tuple(PadReference(refs[p.component],p.pad) for p in n.pads)) for n in board.nets),
            net_routing_rules=tuple(replace(r,net=nets[r.net]) for r in board.net_routing_rules),
            placement_rules=tuple(replace(r,reference=refs[r.reference]) for r in board.placement_rules))
    settings = replace(settings, constrained_pins_first=False, joint_escapes=False,
                       refinement_step_nm=settings.step_nm, surface_fallback=False)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    baseline = preflight_package_access(board,guides,settings,options=PackageAccessOptions(maximum_pattern_trials=0))
    all_owner = preflight_package_access(board,guides,settings,options=PackageAccessOptions(maximum_pattern_trials=1))
    result = preflight_package_access(board,guides,settings)
    assert baseline.failed_critical_nets and not baseline.pending_pads
    assert all_owner.pattern_trials[0].outcome == "lost_ordinary_access" and all_owner.failed_critical_nets
    assert result.ready and len(result.pattern_trials) == 2
    trial = result.pattern_trials[1]
    assert trial.strategy == "partial_ordinary" and trial.outcome == "accepted" and trial.revalidated
    quiet = PadReference(refs["U"] if renamed else "U", "3")
    assert result.fanout.routing_accesses[quiet] == baseline.fanout.routing_accesses[quiet]
    assert result.source == board and result.board.placements == board.placements


@pytest.mark.parametrize("value", (-1, 9, True, 1.5))
def test_partial_owner_budget_rejects_invalid_bounds(value):
    with pytest.raises(ValueError, match="partial ordinary"):
        PackageAccessOptions(maximum_partial_owner_nets=value)
