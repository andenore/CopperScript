"""D-PHY plan R12: tune bundle pairs of a length_match group as they route."""
from __future__ import annotations

from dataclasses import replace
import json

import pcbir.critical as critical
from pcbir import (
    CriticalRoutingStatus, NetMatchGroup, PhysicalBoard, TuningStyle, nm_from_mm, route_critical_nets, route_global,
)
from pcbir.critical_tuning import match_tuning_line

from test_critical_bundles import _bundle_board, _keepout, _rows
from test_length_match_tuning import _hard, _lane, _length

# Three pairs: LANE_B on top under a wall, LANE_A in the middle and LANE_C at
# the bottom, routed in that order (outermost first). LANE_C jogs down 1.2 mm
# and is the longest; LANE_A runs straight beside LANE_B at the clearance.
TOP_WALL = _keepout("wall-above", "10", "0", "20", "8.6")


def _bundle(connector_rows, package_rows=("-1.0", "0", "1.4"), *, max_skew="0.05") -> PhysicalBoard:
    board = _bundle_board(_rows(package_rows, "0.5"), _rows(connector_rows, "0.4"), (TOP_WALL,))
    return replace(board, net_routing_rules=tuple(
        replace(rule, clearance_nm=nm_from_mm("0.4"), breakout_length_nm=nm_from_mm(1),
                breakout_clearance_nm=nm_from_mm("0.25"), tuning_amplitude_limit_nm=nm_from_mm("0.25"),
                tuning_style=TuningStyle.SERPENTINE)
        for rule in board.net_routing_rules),
        match_groups=(NetMatchGroup("lanes", tuple(sorted(net.name for net in board.nets)),
                                    nm_from_mm(max_skew)),) if max_skew else ())


HEMMED = ("-1.0", "0", "2.6")
# Searches use a bounded state budget to keep the tests fast.
LIMIT = 4000


def _route(board, guides):
    return route_critical_nets(board, guides, pair_state_limit=LIMIT)


def _final_pass_only(monkeypatch) -> None:
    monkeypatch.setattr(critical, "_tune_bundle_pairs", lambda *_, **__: False)


def test_a_hemmed_pair_is_tuned_while_its_neighbour_is_unrouted(monkeypatch) -> None:
    board = _bundle(HEMMED)
    guides = route_global(board)
    result = _route(board, guides)
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.skew_before_nm) == ("tuned", nm_from_mm("0.497056"))
    assert tuning.skew_after_nm <= tuning.max_skew_nm
    assert result.status is not CriticalRoutingStatus.FAILED and not _hard(result.board)
    # LANE_B was tuned as soon as the longer LANE_C was accepted, while LANE_A
    # was unrouted; LANE_A routed around its bumps and was tuned in turn.
    assert [(unit.nets, unit.stage) for unit in tuning.units] == [
        (("LANE_B_N", "LANE_B_P"), "routing"), (("LANE_A_N", "LANE_A_P"), "routing")]
    assert all(unit.bumps and not unit.legs for unit in tuning.units)
    for name in ("LANE_A", "LANE_B", "LANE_C"):
        lane = _lane(result.board.tracks, name)
        assert _length(lane, f"{name}_N") == _length(lane, f"{name}_P")
    document = json.loads(result.to_json())["match_tuning"][0]
    assert [unit["stage"] for unit in document["units"]] == ["routing", "routing"]
    assert "LANE_B_N/LANE_B_P bumps" in match_tuning_line(tuning) and "while routing" in match_tuning_line(tuning)
    # Deterministic.
    again = _route(board, guides)
    assert again == result and again.to_json() == result.to_json()

    # The final pass alone finds LANE_B hemmed in by the wall and LANE_A.
    _final_pass_only(monkeypatch)
    late = _route(board, guides)
    (tuning,) = late.match_tuning
    assert tuning.status == "failed" and tuning.reason.startswith("LANE_B_N/LANE_B_P: insufficient tuning room")
    assert not tuning.units


def test_tuning_that_blocks_a_later_pair_is_removed_before_any_rip_up(monkeypatch) -> None:
    # LANE_A has no slack toward LANE_C here: LANE_B's bumps would block it.
    board = _bundle(("-1.0", "0", "2.2"), ("-1.0", "0", "1.0"))
    guides = route_global(board)
    changed = []
    original = critical._tune_bundle_pairs
    monkeypatch.setattr(critical, "_tune_bundle_pairs", lambda *args, **kwargs: (
        changed.append(original(*args, **kwargs)), changed[-1])[1])
    result = _route(board, guides)
    # LANE_B was tuned when LANE_C was accepted; LANE_A then found no
    # candidate past it, so that tuning was removed and LANE_A routed as if
    # it had never been there.
    assert changed[:2] == [False, True] and not result.match_tuning[0].units
    _final_pass_only(monkeypatch)
    late = _route(board, guides)
    (bundle,) = result.bundles
    assert bundle.repairs == ()
    assert all(item.connected for item in result.nets)
    assert (result.locked_tracks, result.nets, result.match_tuning) == (
        late.locked_tracks, late.nets, late.match_tuning)


def test_bundles_without_an_over_skew_group_route_as_before(monkeypatch) -> None:
    called = []
    original = critical._tune_bundle_pairs
    monkeypatch.setattr(critical, "_tune_bundle_pairs", lambda *args, **kwargs: (
        called.append(args[7]), original(*args, **kwargs))[1])
    plain = _bundle(HEMMED, max_skew=None)
    guides = route_global(plain)
    baseline = _route(plain, guides)
    assert called == [] and baseline.match_tuning == ()
    # A group within its limit is checked as pairs are accepted but changes
    # nothing: the copper and results are those of no group at all.
    loose = replace(plain, match_groups=(NetMatchGroup(
        "lanes", tuple(sorted(net.name for net in plain.nets)), nm_from_mm(1)),))
    result = _route(loose, guides)
    assert called and result.match_tuning[0].status == "within_limit" and not result.match_tuning[0].units
    assert (result.locked_tracks, result.nets, result.routing_fingerprint) == (
        baseline.locked_tracks, baseline.nets, baseline.routing_fingerprint)
    assert "units" not in json.loads(result.to_json())["match_tuning"][0]
