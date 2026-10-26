"""Small replayable routing corpus with explicit expected failures and quality bounds.

Authored test lands are finite geometry, not reviewed manufacturer components.
All algorithmic work runs through the shared routing pipeline.
"""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
from hashlib import sha256
import json
from pathlib import Path
import platform
import sys

from .physical import (BoardOutline, CopperKeepout, CopperLayer,
    DecouplingLink, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles,
    Size, Stackup, nm_from_mm)
from .drc import physical_board_digest
from .routing_metrics import routing_quality


def acceptance_cases():
    terminal = PhysicalFootprint("acceptance/terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    cap = PhysicalFootprint("acceptance/bypass", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),
        # Keep the authored body/silk envelope clear of both mask openings.
        FootprintPad("2", Point.mm(0, "1.2"), Size.mm("0.6", "0.6"))), Size.mm(1, 4))
    def pose(ref, x, y, footprint=terminal):
        return Placement(ref, footprint.name, Point.mm(x, y))
    def net(name, *refs):
        return PhysicalNet(name, tuple(PadReference(ref, "1") for ref in refs))
    def box(x0, y0, x1, y1):
        return PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
            ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))))
    common = dict(outline=BoardOutline.rectangle(20, 14), footprints={terminal.name: terminal},
                  metadata={"geometry_scope": "authored finite test lands; not qualified manufacturer footprints"})
    detour = PhysicalBoard("AcceptanceDetour", placements=(pose("A", 4, 7), pose("B", 16, 7)),
        nets=(net("SIGNAL", "A", "B"),),
        net_routing_rules=(NetRoutingRule("SIGNAL", allowed_layers=(CopperLayer.FRONT,), max_vias=0),),
        copper_keepouts=(CopperKeepout("obstacle", (CopperLayer.FRONT,), box(9, 5, 11, 9)),), **common)
    crossing = PhysicalBoard("AcceptanceCrossing", placements=(pose("A", 4, 7), pose("B", 16, 7),
        pose("C", 10, 3), pose("D", 10, 11)), nets=(net("H", "A", "B"), net("V", "C", "D")),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)), **common)
    bypass = PhysicalBoard("AcceptanceBypass", outline=common["outline"],
        footprints={terminal.name: terminal, cap.name: cap},
        placements=(pose("U", 5, 7), pose("C", 8, 7, cap), pose("J", 16, 7)),
        nets=(net("POWER", "U", "C", "J"), PhysicalNet("RETURN", (PadReference("C", "2"),))),
        decoupling_links=(DecouplingLink(PadReference("C", "1"), PadReference("U", "1"),
            PadReference("C", "2"), "POWER", "RETURN"),), metadata=common["metadata"])
    blocked = replace(detour, name="AcceptanceBlocked", copper_keepouts=(
        CopperKeepout("blocked-terminal", (CopperLayer.FRONT, CopperLayer.BACK), box(3, 6, 5, 8)),))
    bases = (("two-layer-detour", detour, True, 0, 25_000_000),
             ("four-layer-crossing", crossing, True, 8, 40_000_000),
             ("two-layer-branched-bypass", bypass, True, 4, 30_000_000),
             ("two-layer-blocked-access", blocked, False, None, None))
    cases = []
    for name, board, connected, max_vias, max_length in bases:
        for rotated in (False, True):
            candidate = _renamed_rotated(board) if rotated else board
            cases.append({"id": name + ("-renamed-rotated" if rotated else ""), "board": candidate,
                "expect_connected": connected, "maximum_vias": max_vias, "maximum_length_nm": max_length})
    return tuple(cases)


def _renamed_rotated(board):
    height = max(p.y_nm for p in board.outline.vertices)
    def point(p):
        return Point(height-p.y_nm, p.x_nm)
    refs = {p.reference: "ALT_" + p.reference for p in board.placements}
    nets = {n.name: "ALT_" + n.name for n in board.nets}
    def pad(p):
        return PadReference(refs[p.component], p.pad)
    def polygon(p):
        return PolygonWithHoles(PolygonRing(tuple(point(v) for v in p.outer.vertices)),
            tuple(PolygonRing(tuple(point(v) for v in ring.vertices)) for ring in p.holes))
    return replace(board, name=board.name + "RenamedRotated",
        outline=BoardOutline.rectangle(14, 20),
        placements=tuple(replace(p, reference=refs[p.reference], position=point(p.position),
                                 rotation_degrees=Decimal(270)) for p in board.placements),
        nets=tuple(replace(n, name=nets[n.name], pads=tuple(pad(p) for p in n.pads)) for n in board.nets),
        net_routing_rules=tuple(replace(r, net=nets[r.net]) for r in board.net_routing_rules),
        copper_keepouts=tuple(replace(k, outline=polygon(k.outline)) for k in board.copper_keepouts),
        decoupling_links=tuple(DecouplingLink(pad(d.capacitor), pad(d.target), pad(d.return_pad),
            nets[d.net], nets[d.return_net]) for d in board.decoupling_links))


def route_case(board):
    from .flow import run_routing_pipeline
    from .placement import PlacementPlannerOptions
    from .routing import GlobalRouterOptions
    from .fanout import FanoutOptions
    from .routeflow import PlacementRoutingFeedbackOptions
    return run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        fanout_options=FanoutOptions(minimum_component_pads=2, two_leg_escapes=False))


def assess_case(case, result, quality):
    """Expectations distinguish a truthful failure from a successful board."""
    from .drc import explicit_copper_connectivity
    graph = explicit_copper_connectivity(result.board)
    connected = all(graph.net_connected(n) for n in result.board.nets)
    hard = [f.code for f in result.drc.findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]
    source_poses = {p.reference: p for p in case["board"].placements}
    checks = {"expected_connectivity": connected == case["expect_connected"],
        "no_new_hard_findings": not hard,
        "fixed_poses_preserved": source_poses == {p.reference: p for p in result.board.placements},
        "source_rules_preserved": case["board"].net_routing_rules == result.board.net_routing_rules,
        "source_keepouts_preserved": case["board"].copper_keepouts == result.board.copper_keepouts,
        "truthful_pipeline_status": (result.status.value == "pass") == connected}
    if case["expect_connected"]:
        checks.update({"via_bound": quality["via_count"] <= case["maximum_vias"],
            "length_bound": quality["track_length_nm"] <= case["maximum_length_nm"],
            "surface_bypasses": all(d["surface_connected"] for d in quality["decoupling"])})
    return {"connected": connected, "checks": checks, "hard_findings": hard,
            "expectations_passed": all(checks.values())}


def run_acceptance(output, *, kicad_cli=None):
    from .backends import KiCadPcbBackend
    from .backends.kicad_project import write_kicad_project
    from .plane_verify import verify_filled_planes
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema": "copperscript-routing-acceptance/v0.1", "status": "incomplete",
        "scope": "bounded software acceptance; authored geometry; not an LLM success-rate or hardware qualification",
        "python": sys.version, "platform": platform.platform(),
        "settings": {"pitch_nm": 500_000, "area_passes": 2, "search_states": 10_000,
                     "placement_candidates": 1, "placement_feedback_iterations": 1, "package_trials": 0},
        "cases": [], "fabrication_ready": False}
    report["implementation_digest"] = _implementation_digest()
    for case in acceptance_cases():
        directory = output / case["id"]
        directory.mkdir()
        result = route_case(case["board"])
        quality = routing_quality(result.board)
        row = {"id": case["id"], "layers": len(result.board.stackup.copper_layers),
            "input_digest": physical_board_digest(case["board"]),
            "output_digest": physical_board_digest(result.board), "pipeline_status": result.status.value,
            "expect_connected": case["expect_connected"], "maximum_vias": case["maximum_vias"],
            "maximum_length_nm": case["maximum_length_nm"],
            **assess_case(case, result, quality), "quality": quality, "native_status": "incomplete"}
        write_kicad_project(KiCadPcbBackend().generate(result.board), directory / "candidate.kicad_pcb")
        (directory / "physical-drc.json").write_text(result.drc.to_json(), encoding="utf-8")
        if kicad_cli is not None:
            try:
                native = verify_filled_planes(result.board, kicad_cli=Path(kicad_cli),
                                             artifact_directory=directory / "native")
                row["native"] = {"passed": native.passed, "version": native.kicad_version,
                    "unconnected": native.unconnected_count, "violations": native.other_violation_count,
                    "report_digest": native.report_digest, "filled_board_digest": native.filled_board_digest}
                # Expected failures must have native opens; unrelated violations
                # remain regressions, even in a deliberately blocked fixture.
                expected_native = (native.passed if case["expect_connected"] else
                    native.unconnected_count is not None and native.unconnected_count > 0
                    and native.other_violation_count == 0)
                row["native_status"] = "pass" if expected_native else "fail"
            except (OSError, ValueError, RuntimeError) as exc:
                row["native_reason"] = str(exc)
        report["cases"].append(row)
        # Retain completed cases if a later process is interrupted.
        (output / "acceptance.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    report["software_expectations_passed"] = all(r["expectations_passed"] for r in report["cases"])
    report["implementation_digest_after"] = _implementation_digest()
    report["implementation_stable"] = report["implementation_digest"] == report["implementation_digest_after"]
    report["status"] = ("fail" if not report["software_expectations_passed"] or
        any(r["native_status"] == "fail" for r in report["cases"]) else
        "pass" if all(r["native_status"] == "pass" for r in report["cases"]) else "incomplete")
    if not report["implementation_stable"] and report["status"] == "pass":
        report["status"] = "incomplete"
    (output / "acceptance.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return report


def _implementation_digest():
    implementation = sha256()
    package = Path(__file__).parent
    for path in sorted(package.rglob("*.py")):
        implementation.update(path.relative_to(package).as_posix().encode("utf-8") + b"\0")
        implementation.update(path.read_bytes() + b"\0")
    return implementation.hexdigest()
