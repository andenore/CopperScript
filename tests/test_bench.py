from dataclasses import replace
import json
from pathlib import Path

import pytest

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


def test_track_bends_count_turns_between_two_tracks_of_one_net():
    from pcbir.bench import track_bends
    top = CopperLayer.FRONT
    p = lambda x, y: Point(nm_from_mm(str(x)), nm_from_mm(str(y)))
    width = nm_from_mm("0.2")
    tracks = [TrackSegment("A", p(0, 0), p(2, 0), width, top),   # straight run: no bend at (2, 0)
              TrackSegment("A", p(2, 0), p(4, 0), width, top),
              TrackSegment("A", p(4, 0), p(5, 1), width, top),   # 45-degree bend at (4, 0)
              TrackSegment("A", p(5, 1), p(5, 3), width, top),   # 45-degree bend at (5, 1)
              TrackSegment("A", p(5, 3), p(7, 3), width, top),   # 90-degree bend at (5, 3)
              TrackSegment("B", p(7, 3), p(7, 5), width, top)]   # another net meeting there is no bend
    assert track_bends(tracks) == {"bends": 3, "bends_90": 1}


def test_run_needs_prepared_boards_and_a_fresh_results_directory(tmp_path):
    from pcbir.bench import PREPARED_NAME, run_boards
    boards = tmp_path / "boards"
    (boards / "0001_empty").mkdir(parents=True)
    with pytest.raises(ValueError, match="no prepared boards"):
        run_boards(boards, tmp_path / "out")
    (boards / "0001_empty" / PREPARED_NAME).write_text("")
    used = tmp_path / "used"
    used.mkdir()
    (used / "old.txt").write_text("")
    with pytest.raises(ValueError, match="not empty"):
        run_boards(boards, used)
    with pytest.raises(ValueError, match="no prepared boards"):
        run_boards(boards, tmp_path / "out", only=("0002",))


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
    assert len(baseline["boards"]) == 109  # D3-A 99, D3-B 10
    assert sum(item["clean_pass"] for item in baseline["boards"].values()) == 100  # D3-A 93, D3-B 7


def test_baseline_round_trips_through_the_published_record():
    from pcbir.bench import baseline_from_record
    record = ("board|status|clean|unrouted|drc_errors|unconnected|vias|wirelength_mm\n"
              "a|routed|1|0|0|0|3|12.5\nb|import_failed|0|None|None|None|None|None\n")
    boards = baseline_from_record(record)["boards"]
    # Records published before the routable/seconds columns still parse.
    assert boards["a"] == {"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0,
                           "unconnected_items": 0, "vias": 3, "wirelength_mm": 12.5,
                           "routable_nets": None, "seconds": None, "bends": None, "bends_90": None}
    assert boards["b"]["clean_pass"] is False and boards["b"]["vias"] is None


def test_record_text_round_trips_with_routable_nets_and_seconds():
    from pcbir.bench import baseline_from_record, record_text
    baseline = _baseline(a={"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0,
                            "unconnected_items": 0, "vias": 3, "wirelength_mm": 12.5,
                            "routable_nets": 20, "seconds": 4.25})
    boards = baseline_from_record(record_text(baseline))["boards"]
    assert boards["a"]["routable_nets"] == 20 and boards["a"]["seconds"] == 4.25


def test_versus_compares_two_runs_on_every_measure():
    from pcbir.bench import versus
    legacy = _baseline(
        a={"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0, "unconnected_items": 0,
           "vias": 4, "wirelength_mm": 20.0, "routable_nets": 10, "seconds": 3.0},
        b={"status": "routed", "clean_pass": False, "unrouted": 2, "drc_errors": 1, "unconnected_items": 3,
           "vias": 1, "wirelength_mm": 5.0, "routable_nets": 10, "seconds": 9.0},
        c={"status": "route_crashed", "clean_pass": False, "unrouted": None, "drc_errors": None,
           "unconnected_items": None, "vias": None, "wirelength_mm": None, "routable_nets": 10, "seconds": 1.0})
    unified = _baseline(
        a={"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0, "unconnected_items": 0,
           "vias": 2, "wirelength_mm": 18.0, "routable_nets": 10, "seconds": 2.0},
        b={"status": "routed", "clean_pass": True, "unrouted": 0, "drc_errors": 0, "unconnected_items": 0,
           "vias": 3, "wirelength_mm": 7.0, "routable_nets": 10, "seconds": 4.0},
        c={"status": "routed", "clean_pass": False, "unrouted": 1, "drc_errors": 0, "unconnected_items": 1,
           "vias": 0, "wirelength_mm": 1.0, "routable_nets": 10, "seconds": 1.5})
    text = versus(legacy, unified, ("legacy", "unified"))
    assert "| Clean (0 KiCad errors, 0 unconnected) | 1 | 2 |" in text
    assert "| Fully routed (0 unrouted nets) | 1 | 2 |" in text
    # Legacy's crashed board has no unrouted count: completion uses the boards that report one.
    assert "| Net completion (all boards) | 0.9 | 0.9667 |" in text
    # Copper is compared only where both runs are clean (board a).
    assert "| Vias, 1 boards clean in both | 4 | 2 |" in text
    assert "| Not routed (crash, timeout, missing) | 1 | 0 |" in text
    assert "| Unrouted nets | 1 | 1 | 0 |" in text and "| Routing seconds | 2 | 0 | 1 |" in text
    assert "Clean only with unified (1): b" in text and "Clean only with legacy (0): none" in text

def test_unrouted_nets_get_a_primary_cause_class():
    from pcbir.bench import unrouted_causes
    record = {
        "unrouted_nets": ["CE", "MISO", "PWR", "SIG", "LOST", "ODD"],
        "unrouted_details": {
            "CE": ["detailed search cannot reach P1.6",
                   "search: 4 calls, 0 out of budget, 4 no path, 900 states"],
            "MISO": ["search: 3 calls, 3 out of budget, 0 no path, 60000 states"],
            "PWR": ["area routing blocked: package-access preflight incomplete"],
            "SIG": ["search: 2 calls, 2 out of budget, 0 no path, 40000 states"],
            "ODD": ["rip-up limit reached"],
        },
        "escape_causes": {"CE": ["two_end_crossing"], "MISO": ["two_end_crossing"]},
        "package_pending_nets": ["LOST"],
    }
    assert unrouted_causes(record) == {
        "CE": ["two_end_crossing", "no_path"], "MISO": ["two_end_crossing", "search_budget"],
        "PWR": ["package_access"], "SIG": ["search_budget"], "LOST": ["package_access", "unlocated"],
        "ODD": ["other"]}
    # An escape cause outranks the pending exit it explains.
    record["escape_causes"]["LOST"] = ["channel_overflow"]
    assert unrouted_causes(record)["LOST"] == ["channel_overflow", "package_access", "unlocated"]


def test_cause_table_counts_primary_causes_and_marks_kicad_only_failures(tmp_path):
    from pcbir.bench import cause_table
    records = {
        "clean": {"board": "clean", "clean_pass": True, "status": "routed", "unrouted_nets": []},
        "header": {"board": "header", "clean_pass": False, "status": "routed", "unrouted_nets": ["CE", "SCK"],
                   "unrouted_causes": {"CE": ["two_end_crossing", "no_path"], "SCK": ["no_path"]},
                   "escape": {"pairs": [{"components": ["P1", "U1"], "crossing_pairs": 1,
                                         "layer_moves": ["MISO"], "analysed": True}], "clusters": []}},
        "kicad": {"board": "kicad", "clean_pass": False, "status": "routed", "unrouted_nets": [],
                  "unconnected_items": 2, "scored": True},
        "unscored": {"board": "unscored", "status": "routed", "unrouted_nets": []},
    }
    for name, record in records.items():
        (tmp_path / name).mkdir()
        (tmp_path / name / "result.json").write_text(json.dumps(record))
    table = cause_table(tmp_path)
    assert "| clean |" not in table and "| unscored |" not in table
    assert "| header | 2 | 1 | 0 | 0 | 0 | 1 | 0 | 0 | P1-U1 crossings 1, move MISO |" in table
    assert "| kicad | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | KiCad only: no unrouted net |" in table
    assert "| **Total (primary cause)** | 2 | 1 | 0 | 0 | 0 | 1 | 0 | 0 | |" in table


def test_tiers_split_the_summary_and_the_comparison(tmp_path):
    from pcbir.bench import compare_to_baseline, main
    for name, clean in (("0001_a", True), ("0002_b", False), ("0113_c", True)):
        (tmp_path / name).mkdir()
        (tmp_path / name / "result.json").write_text(json.dumps({"board": name, "clean_pass": clean,
                                                                 "status": "routed", "seconds": 1}))
    tiers = {"0001_a": "D3-A", "0002_b": "D3-A", "0113_c": "D3-B"}
    assert summarize(tmp_path, {"0001_a", "0002_b"})["clean_pass_rate"] == 0.5
    before = _baseline(**{"0001_a": {"clean_pass": True}, "0002_b": {"clean_pass": False}})
    after = _baseline(**{"0001_a": {"clean_pass": True}, "0002_b": {"clean_pass": True},
                         "0113_c": {"clean_pass": True}})
    text, lost = compare_to_baseline(before, after, tiers)
    assert not lost
    assert "- D3-A: 1 of 2 → 2 of 2" in text
    assert "- D3-B: 0 of 0 → 1 of 1 (1 board(s) not in the baseline)" in text
    tiers_file = tmp_path / "tiers.json"
    tiers_file.write_text(json.dumps(tiers))
    assert main(["summarize", str(tmp_path), "--markdown", "--tiers", str(tiers_file)]) == 0


def test_write_back_names_nets_on_a_kicad10_board_without_a_net_table():
    text = ('(kicad_pcb (version 20260206) (generator "pcbnew")\n'
            '  (layers (0 "F.Cu" signal) (2 "B.Cu" signal))\n'
            '  (segment (start 0 0) (end 1 0) (width 0.2) (layer "F.Cu") (net "OLD"))\n)\n')
    track = TrackSegment("SIG", Point(nm_from_mm(1), nm_from_mm(2)), Point(nm_from_mm(3), nm_from_mm(2)),
                         nm_from_mm("0.2"), CopperLayer.FRONT)
    via = Via("GND", Point(nm_from_mm(4), nm_from_mm(2)), nm_from_mm("0.6"), nm_from_mm("0.3"))
    routed = replace(_routed(LEGACY), tracks=(track,), vias=(via,))
    written = write_back_routing(text, routed)
    assert '(net "OLD")' not in written
    assert '(layer "F.Cu") (net "SIG"))' in written and '(net "GND"))' in written
