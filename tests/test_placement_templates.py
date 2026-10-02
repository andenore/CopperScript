from dataclasses import replace
from hashlib import sha256
import json

import pytest

from pcbir import (
    BoardOutline, FootprintPad, PhysicalBoard, PhysicalFootprint, Placement,
    Point, Size, PlacementPlannerOptions, placement_solution_is_legal,
)
from pcbir.clusters import footprint_geometry_digest, cluster_placements
from pcbir.placement_templates import apply_placement_templates
from pcbir.cli import main


def fixture(tmp_path):
    fp = PhysicalFootprint("test", (FootprintPad("1", Point.mm(0, 0), Size.mm(.2, .2)),), Size.mm(1, 1))
    board = PhysicalBoard("Template", BoardOutline.rectangle(30, 30), {"test": fp},
                          (Placement("U1", "test", Point.mm(10, 10)),
                           Placement("L1", "test", Point.mm(11.2, 10))), ())
    reference = {"schema": "copperlib-rigid-reference/v0.1", "production_publishable": False,
                 "source": {"url": "synthetic fixture", "entry": "fixture"},
                 "anchor": {"reference": "chip", "pad": "1"}, "pad_nets": [],
                 "members": [{"reference": "chip", "center_nm": [0, 0], "rotation_degrees": "0"},
                             {"reference": "inductor", "center_nm": [1200000, 0], "rotation_degrees": "0"}]}
    raw = json.dumps(reference).encode()
    (tmp_path / "reference.json").write_bytes(raw)
    scene = {"schema": "copperscript-placement-templates/v0.1", "clusters": [{
        "name": "unit", "reference": "reference.json", "reference_sha256": sha256(raw).hexdigest(),
        "bindings": {"chip": "U1", "inductor": "L1"}, "net_bindings": {},
        "footprint_digests": {"chip": footprint_geometry_digest(fp), "inductor": footprint_geometry_digest(fp)},
        "allowed_rotations": [0, 45, 90], "internal_clearance_nm": 0,
    }]}
    path = tmp_path / "scene.json"
    path.write_text(json.dumps(scene))
    return board, path, scene


def test_source_scene_binding_and_macro_only_courtyard_gap(tmp_path):
    board, path, scene = fixture(tmp_path)
    assert not placement_solution_is_legal(board, {p.reference: p for p in board.placements})
    adapted = apply_placement_templates(board, path)
    assert placement_solution_is_legal(adapted, {p.reference: p for p in adapted.placements})
    assert adapted.rules == board.rules
    assert adapted.nets == board.nets
    assert adapted.metadata["fabrication_ready"] == "false"
    assert adapted.rigid_clusters[0].internal_clearance_nm == 0
    assert adapted.placements == board.placements  # binding does not manually move anything
    outsider = Placement("J1", "test", Point.mm(12.4, 10))
    adapted = replace(adapted, placements=(*adapted.placements, outsider))
    assert not placement_solution_is_legal(adapted, {p.reference: p for p in adapted.placements})


@pytest.mark.parametrize("change, message", [
    ("source", "reference identity"), ("footprint", "footprint identity"),
    ("net", "pad/net"), ("member", "exactly cover"), ("unknown", "unsupported"),
])
def test_binding_failures_never_mutate_input(tmp_path, change, message):
    board, path, scene = fixture(tmp_path)
    binding = scene["clusters"][0]
    if change == "source":
        (tmp_path / "reference.json").write_text("changed")
    elif change == "footprint":
        binding["footprint_digests"]["chip"] = "0" * 64
    elif change == "net":
        source = json.loads((tmp_path / "reference.json").read_text())
        source["pad_nets"] = [["chip", "1", "ground"]]
        raw = json.dumps(source).encode()
        (tmp_path / "reference.json").write_bytes(raw)
        binding["reference_sha256"] = sha256(raw).hexdigest()
        binding["net_bindings"] = {"ground": "GND"}
    elif change == "member":
        binding["bindings"].pop("chip")
    else:
        binding["ignored_rule"] = True
    path.write_text(json.dumps(scene))
    with pytest.raises(ValueError, match=message):
        apply_placement_templates(board, path)
    assert not board.rigid_clusters


def test_asset_binding_is_independent_of_checkout_location():
    fp = PhysicalFootprint("test", (), Size.mm(1, 1), metadata={"source_path": "old", "source_sha256": "same"})
    assert footprint_geometry_digest(fp) == footprint_geometry_digest(replace(fp, metadata={
        "source_path": "new", "source_sha256": "same"}))
    assert footprint_geometry_digest(fp) != footprint_geometry_digest(replace(fp, metadata={
        "source_path": "new", "source_sha256": "changed"}))


@pytest.mark.parametrize("command", ["route-board", "route-global", "plan-layout", "export-kicad-pcb"])
def test_cli_physical_paths_apply_templates_once(command, tmp_path, monkeypatch, capsys):
    calls = []
    def probe(board, path, **options):
        calls.append(path)
        raise ValueError("template integration probe")
    monkeypatch.setattr("pcbir.cli.apply_placement_templates", probe)
    path = tmp_path / "scene.json"
    assert main([command, "examples/valid_board.copper", "--allow-proxy-footprints",
                 "--placement-templates", str(path)]) == 2
    assert calls == [path]
    assert "template integration probe" in capsys.readouterr().out


@pytest.mark.parametrize("rotation", [0, 45])
def test_source_bound_nordic_matching_routes_on_installed_footprints(rotation):
    from pathlib import Path
    from pcbir import compile_file, resolved_physicalize, FootprintResolver, PrototypePhysicalOptions
    from pcbir.physical import PhysicalNet, CopperLayer
    from pcbir.routing import route_global, GlobalRouterOptions
    from pcbir.critical import route_critical_nets
    from pcbir.drc import run_physical_drc

    root = Path(__file__).resolve().parents[1]
    footprints = Path("C:/Program Files/KiCad/10.0/share/kicad/footprints")
    library = root.parent / "CopperLib"
    if not footprints.is_dir() or not library.is_dir():
        pytest.skip("optional real-reference regression requires installed KiCad and sibling CopperLib")
    original = resolved_physicalize(compile_file(root / "examples/full_vertical_board.copper"),
        FootprintResolver(root / "examples", (footprints, library / "footprints")),
        PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer"))
    refs = {"U_NRF", "C_BT_MATCH", "L_BT_MATCH"}
    poses = tuple(item for item in original.placements if item.reference in refs)
    # This bounded matching-only fixture is not a complete operational MCU board.
    board = PhysicalBoard("NordicMatchingProbe", BoardOutline.rectangle(40, 40),
        {item.footprint: original.footprints[item.footprint] for item in poses}, poses,
        tuple(PhysicalNet(net.name, tuple(pad for pad in net.pads if pad.component in refs))
              for net in original.nets if net.name in {"NRF_RF_RAW", "NRF_RF_ANT", "GND"}),
        stackup=original.stackup, rules=original.rules,
        net_routing_rules=tuple(rule for rule in original.net_routing_rules if rule.net == "NRF_RF_RAW"))
    board = apply_placement_templates(board, root / "examples/full_vertical_placement_templates.json")
    current = {pose.reference: pose for pose in board.placements}
    current.update(cluster_placements(board, board.rigid_clusters[0], replace(
        current["U_NRF"], position=Point.mm(20, 20), rotation_degrees=rotation)))
    board = replace(board, placements=tuple(current.values()))
    assert placement_solution_is_legal(board, current)
    guides = route_global(board, GlobalRouterOptions(maximum_iterations=5, tile_size_nm=1000000))
    critical = route_critical_nets(board, guides)
    assert critical.nets[0].connected, (critical.nets[0].diagnostics, guides.metrics,
                                      [(route.net, route.diagnostics) for route in guides.routes])
    assert not critical.board.vias
    assert critical.nets[0].lengths_nm[0] < 5000000
    assert all(track.layer is CopperLayer.FRONT for track in critical.board.tracks)
    assert not any(finding.code == "DRC-OPEN-NET" and "NRF_RF_RAW" in finding.nets
                   for finding in run_physical_drc(critical.board).findings)
    assert not {finding.code for finding in run_physical_drc(critical.board).findings
                if finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}}
