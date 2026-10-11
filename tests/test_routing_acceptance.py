from dataclasses import replace

import pytest

from pcbir.physical import CopperLayer, Point, TrackSegment, Via, nm_from_mm
from pcbir.routing_acceptance import acceptance_cases, assess_case, route_case, run_acceptance
from pcbir.routing_metrics import routing_quality


@pytest.mark.parametrize("case", acceptance_cases(), ids=lambda case: case["id"])
def test_diverse_bounded_shared_pipeline(case):
    result = route_case(case["board"])
    quality = routing_quality(result.board)
    assessed = assess_case(case, result, quality)
    assert assessed["expectations_passed"], assessed
    assert quality["native_fill_current"] is False


def test_bypass_interior_junction_and_no_via_or_internal_shortcut():
    case = acceptance_cases()[4]
    board = case["board"]
    # The capacitor joins the middle of the longer same-net segment.
    board = replace(board, tracks=(TrackSegment("POWER", Point.mm(5, 7), Point.mm(16, 7),
        nm_from_mm("0.25"), CopperLayer.FRONT),))
    metrics = routing_quality(board)
    assert metrics["decoupling"][0]["surface_connected"]
    assert metrics["decoupling"][0]["surface_component_track_length_nm"] == 11_000_000
    assert metrics["decoupling"][0]["pad_distance_nm"] == 3_000_000
    assert not routing_quality(replace(board, tracks=()))["decoupling"][0]["surface_connected"]
    bottom = replace(board, tracks=(replace(board.tracks[0], layer=CopperLayer.BACK),),
        vias=(Via("POWER", Point.mm(5, 7), 600_000, 300_000, CopperLayer.FRONT, CopperLayer.BACK),
              Via("POWER", Point.mm(8, 7), 600_000, 300_000, CopperLayer.FRONT, CopperLayer.BACK)))
    assert not routing_quality(bottom)["decoupling"][0]["surface_connected"]


def test_added_copper_changes_actual_quality_not_pipeline_counters():
    board = acceptance_cases()[0]["board"]
    original = routing_quality(board)
    changed = replace(board, tracks=(TrackSegment("SIGNAL", Point.mm(4, 7), Point.mm(6, 7),
        250_000, CopperLayer.FRONT),), vias=(Via("SIGNAL", Point.mm(6, 7),
        600_000, 300_000, CopperLayer.FRONT, CopperLayer.BACK),))
    metrics = routing_quality(changed)
    assert metrics["via_count"] == original["via_count"] + 1
    assert metrics["track_length_nm"] == 2_000_000
    assert metrics["board_digest"] != original["board_digest"]


def test_stale_native_reference_evidence_remains_incomplete():
    from pcbir.plane_verify import PlaneVerification
    from pcbir.physical import NetRoutingRule, RouteKind
    board = acceptance_cases()[0]["board"]
    board = replace(board, net_routing_rules=(NetRoutingRule("SIGNAL", kind=RouteKind.CLOCK),))
    stale = PlaneVerification(True, "different-geometry", "different-export", "filled", "report", "10.0.6", 0, 0, 0, ())
    metrics = routing_quality(board, stale)
    assert not metrics["native_fill_current"]
    assert metrics["native_report_digest"] is None
    assert metrics["reference_coverage"][0]["status"] == "incomplete"


def test_faster_geometry_cannot_silently_exceed_fixture_bounds():
    case = acceptance_cases()[0]
    result = route_case(case["board"])
    quality = routing_quality(result.board)
    assert not assess_case({**case, "maximum_length_nm": 1}, result, quality)["expectations_passed"]


def test_fresh_output_and_missing_native_remain_incomplete(tmp_path):
    directory = tmp_path / "acceptance"
    report = run_acceptance(directory)
    assert report["status"] == "incomplete"
    assert report["software_expectations_passed"]
    assert report["implementation_stable"]
    assert len(report["cases"]) == 8
    assert sum(row["expect_connected"] for row in report["cases"]) == 6
    assert (directory / "acceptance.json").is_file()
    with pytest.raises(FileExistsError):
        run_acceptance(directory)
