from dataclasses import replace
import json
from pathlib import Path

from pcbir.bench import _top_level_spans, net_codes, score_drc_report, summarize, write_back_routing
from pcbir.importers.kicad_mod import _parse_sexpr
from pcbir.importers.kicad_pcb import parse_kicad_pcb
from pcbir.physical import CopperLayer, Point, TrackSegment, Via, nm_from_mm

from test_kicad_pcb_import import LEGACY


def _routed(text):
    board = parse_kicad_pcb(text, source="legacy", name="legacy", project=None).board
    track = TrackSegment("SIG", Point(nm_from_mm(10), nm_from_mm(10)), Point(nm_from_mm("12.5"), nm_from_mm(10)),
                         nm_from_mm("0.2"), CopperLayer.BACK)
    via = Via("GND", Point(nm_from_mm(14), nm_from_mm(11)), nm_from_mm("0.6"), nm_from_mm("0.3"))
    return replace(board, tracks=(track,), vias=(via,))


def test_write_back_replaces_routing_and_keeps_everything_else():
    written = write_back_routing(LEGACY, _routed(LEGACY))
    root = _parse_sexpr(written, "written")
    tags = [tag for _, _, tag in _top_level_spans(written)]
    assert tags.count("segment") == 1 and tags.count("via") == 1
    segment = next(item for item in root if isinstance(item, list) and item[0] == "segment")
    via = next(item for item in root if isinstance(item, list) and item[0] == "via")
    # KiCad 5 custom layer names and the board's own net numbers are used.
    assert ["layer", "Bottom"] in segment and ["net", "2"] in segment and ["end", "12.5", "10"] in segment
    assert ["layers", "Top", "Bottom"] in via and ["net", "1"] in via
    # Non-routing items survive byte for byte.
    for start, end, tag in _top_level_spans(LEGACY):
        if tag not in {"segment", "via"}:
            assert LEGACY[start:end] in written
    assert written.rstrip().endswith(")")
    assert net_codes(written) == {"": 0, "GND": 1, "SIG": 2}


def test_write_back_rejects_unknown_nets():
    stripped = LEGACY.replace('(net 0 "") (net 1 GND) (net 2 SIG)', '(net 0 "") (net 2 SIG)')
    try:
        write_back_routing(stripped, _routed(LEGACY))
    except ValueError as exc:
        assert "GND" in str(exc)
    else:
        raise AssertionError("unknown net accepted")


def test_clean_pass_needs_no_errors_and_nothing_unconnected():
    clean = {"violations": [{"severity": "warning", "type": "silk_overlap"}], "unconnected_items": []}
    assert score_drc_report(clean)["clean_pass"] is True
    dirty = {"violations": [{"severity": "error", "type": "clearance"}], "unconnected_items": []}
    assert score_drc_report(dirty)["clean_pass"] is False
    assert score_drc_report(dirty)["drc_error_types"] == {"clearance": 1}
    assert score_drc_report({"violations": [], "unconnected_items": [{}]})["clean_pass"] is False


def test_summary_counts_failures_against_the_clean_pass_rate(tmp_path):
    for name, record in {
        "a": {"status": "routed", "scored": True, "clean_pass": True, "routable_nets": 4, "unrouted_nets": [],
              "drc_errors": 0, "unconnected_items": 0, "vias": 2, "wirelength_mm": 10, "seconds": 1},
        "b": {"status": "routed", "scored": True, "clean_pass": False, "routable_nets": 4, "unrouted_nets": ["X"],
              "drc_errors": 2, "unconnected_items": 1, "vias": 4, "wirelength_mm": 20, "seconds": 3},
        "c": {"status": "import_failed", "seconds": 0},
    }.items():
        (tmp_path / name).mkdir()
        (tmp_path / name / "result.json").write_text(json.dumps(record))
    summary = summarize(tmp_path)
    assert summary["boards"] == 3 and summary["clean_pass_rate"] == 0.333
    assert summary["net_completion_mean"] == 0.875
    assert summary["statuses"] == {"import_failed": 1, "routed": 2}


def test_annotation_chunks_fit_and_keep_every_character():
    from pcbir.bench import annotation_chunks
    text = "".join(f"line {i} " + "x" * (i * 37 % 300) + "\n" for i in range(400)) + "y" * 9000 + "\n"
    chunks = annotation_chunks(text, limit=1000)
    assert all(len(chunk) <= 1000 for chunk in chunks)
    assert "".join(chunks) == text


def test_failure_report_lists_only_unclean_boards_with_their_causes(tmp_path):
    from pcbir.bench import failure_report
    for name, record in {
        "ok": {"board": "ok", "clean_pass": True},
        "bad": {"board": "bad", "clean_pass": False, "status": "routed",
                "unrouted_details": {"N1": ["detailed search cannot reach U1.2"]},
                "drc_findings": [{"kind": "violation", "type": "clearance", "description": "Clearance violation",
                                  "items": [{"description": "Track [N1]", "pos": {"x": 1, "y": 2}}]}]},
    }.items():
        (tmp_path / name).mkdir()
        (tmp_path / name / "result.json").write_text(json.dumps(record))
    report = failure_report(tmp_path)
    assert "#### bad" in report and "#### ok" not in report
    assert "unrouted N1: detailed search cannot reach U1.2" in report
    assert "KiCad violation clearance" in report and "Track [N1]" in report


def _baseline(**boards):
    return {"schema": "copperscript-bench-baseline/v0.1", "boards": boards}


def test_compare_reports_gained_and_lost_clean_boards():
    from pcbir.bench import compare_to_baseline
    before = _baseline(a={"clean_pass": True, "vias": 2, "wirelength_mm": 10.0},
                       b={"clean_pass": False, "unrouted": 1}, c={"clean_pass": True, "vias": 1, "wirelength_mm": 5.0},
                       d={"clean_pass": False, "unrouted": 3, "unconnected_items": 6})
    after = _baseline(a={"clean_pass": True, "vias": 3, "wirelength_mm": 11.0},
                      b={"clean_pass": True}, c={"clean_pass": False, "unrouted": 1},
                      d={"clean_pass": False, "unrouted": 1, "unconnected_items": 2})
    text, lost = compare_to_baseline(before, after)
    assert lost == ["c"]
    assert "Clean boards: 2 → 2 (gained 1, lost 1)" in text and "Now clean: b" in text
    assert "Vias on the 1 boards clean in both: 2 → 3" in text
    assert "- d: unrouted 3 → 1, unconnected_items 6 → 2" in text


def test_committed_baseline_matches_the_published_result():
    baseline = json.loads((Path(__file__).parents[1] / ".github/routing-benchmark-baseline.json").read_text())
    assert len(baseline["boards"]) == 99
    assert sum(item["clean_pass"] for item in baseline["boards"].values()) == 92


def test_baseline_round_trips_through_the_published_record():
    from pcbir.bench import baseline_from_record
    record = ("board|status|clean|unrouted|drc_errors|unconnected|vias|wirelength_mm\n"
              "a|routed|1|0|0|0|3|12.5\nb|import_failed|0|None|None|None|None|None\n")
    boards = baseline_from_record(record)["boards"]
    assert boards["a"] == {"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0,
                           "unconnected_items": 0, "vias": 3, "wirelength_mm": 12.5}
    assert boards["b"]["clean_pass"] is False and boards["b"]["vias"] is None
