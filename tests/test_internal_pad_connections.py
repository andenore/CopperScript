"""Internal component paths are explicit, scoped, and independently exported."""
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (InternalPadGroup, PhysicalBoard, PhysicalFootprint, FootprintPad,
                   Point, Size, Placement, BoardOutline, PadReference, PhysicalNet,
                   TrackSegment, CopperLayer, nm_from_mm, parse_kicad_mod)
from pcbir.physical import FootprintLine, FootprintLayer
from pcbir.drc import explicit_copper_connectivity, physical_board_digest
from pcbir.pad_stitch import stitch_duplicate_pads
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir import compile_source, check, CopperScriptError
from pcbir.serializer import board_to_dict


def native_cli():
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("requires KiCad 10")
    version = subprocess.run([cli, "version"], check=True, capture_output=True, text=True).stdout.strip()
    if int(version.split(".", 1)[0]) < 10:
        pytest.skip("jumper groups require KiCad 10")
    return cli


def compile_contact_fixture(tmp_path, groups="1; 2", nets="net V { S.A; } net GND { S.B; }", assembled=True, b_extra=""):
    tmp_path = tmp_path / f"fixture-{len(tuple(tmp_path.iterdir()))}"
    tmp_path.mkdir()
    (tmp_path / "copper.mod").write_text("module contact-test\nrequire github.com/test/contacts v1.0.0\nreplace github.com/test/contacts => .\n", encoding="utf-8")
    (tmp_path / "contacts.copper").write_text(f'''part Contact {{
        footprint = "contacts"; assembled = {str(assembled).lower()};
        internal_pad_groups = "{groups}";
        pin A {{ number = "1"; domains = "digital"; directions = "passive"; }}
        pin B {{ number = "2"; domains = "digital"; directions = "passive"; {b_extra} }}
    }}''', encoding="utf-8")
    return compile_source(f'''board ContactTest {{
        import c "github.com/test/contacts";
        component S: c.Contact; {nets}
    }}''', str(tmp_path / "board.copper"))


def test_language_and_erc_preserve_separate_groups(tmp_path):
    board = compile_contact_fixture(tmp_path)
    assert not check(board)
    part = board.library["c.Contact"]
    assert part.internal_pad_groups == (InternalPadGroup(("1",)), InternalPadGroup(("2",)))
    assert board_to_dict(board)["parts"][0]["internal_pad_groups"] == [["1"], ["2"]]
    conflicting = compile_contact_fixture(tmp_path, "1, 2")
    assert "INTERNAL_PAD_NET_CONFLICT" in {d.code for d in check(conflicting)}


def test_internal_alias_preserves_voltage_checks_without_mutating_source(tmp_path):
    board = compile_contact_fixture(tmp_path, "1, 2",
        "net V { S.A; } supply V { voltage = 3.3V; external = true; }",
        b_extra="voltage_max = 1.8V;")
    assert "SUPPLY_VOLTAGE_HIGH" in {d.code for d in check(board)}
    assert len(board.nets[0].endpoints) == 1


@pytest.mark.parametrize("groups", ["3", "1; 1", "1,1", "1;", "1, 2; 2"])
def test_language_rejects_bad_groups(tmp_path, groups):
    with pytest.raises(CopperScriptError, match="CMP088"):
        compile_contact_fixture(tmp_path, groups)


def test_physical_lowering_expands_aliases_and_removes_unassembled_paths(tmp_path):
    from pcbir import prototype_physicalize
    board = compile_contact_fixture(tmp_path, "1, 2", "net V { S.A; }")
    physical = prototype_physicalize(board)
    assert set(physical.nets[0].pads) == {PadReference("S", "1"), PadReference("S", "2")}
    unassembled = prototype_physicalize(compile_contact_fixture(tmp_path, "1, 2", assembled=False))
    assert not next(iter(unassembled.footprints.values())).internal_pad_groups
    # Singleton duplicate-land facts need no invented lands in proxy geometry.
    assert not next(iter(prototype_physicalize(compile_contact_fixture(tmp_path)).footprints.values())).internal_pad_groups


def board_with_groups(groups=(InternalPadGroup(("1",)),)):
    footprint = PhysicalFootprint("clip", (
        FootprintPad("1", Point.mm(-2, 0), Size.mm(1, 1)),
        FootprintPad("1", Point.mm(2, 0), Size.mm(1, 1)),
        FootprintPad("2", Point.mm(0, 2), Size.mm(1, 1)),
        FootprintPad("2", Point.mm(0, 4), Size.mm(1, 1)),
    ), Size.mm(5, 5), internal_pad_groups=groups,
        graphics=(FootprintLine(Point.mm(-2, -2), Point.mm(2, -2), nm_from_mm(.1), FootprintLayer.FABRICATION),))
    numbers = next((g.numbers for g in groups if "1" in g.numbers), ("1",))
    return PhysicalBoard("Internal", BoardOutline.rectangle(20, 20), {"clip": footprint},
        (Placement("BT1", "clip", Point.mm(5, 5)),),
        (PhysicalNet("V", tuple(PadReference("BT1", n) for n in numbers)),))


def test_declared_group_connects_assembled_but_not_bare_copper():
    board = board_with_groups()
    pad = PadReference("BT1", "1")
    assert explicit_copper_connectivity(board).pad_connected(pad)
    assert not explicit_copper_connectivity(board, include_internal_connections=False).pad_connected(pad)
    assert explicit_copper_connectivity(board).internal_connections
    closure = stitch_duplicate_pads(board)
    assert closure.added_track_count == 0 and closure.already_connected == (pad,)
    undeclared = replace(board, footprints={"clip": replace(board.footprints["clip"], internal_pad_groups=())})
    assert not explicit_copper_connectivity(undeclared).pad_connected(pad)
    assert physical_board_digest(board) != physical_board_digest(undeclared)


def test_no_short_between_button_sides():
    board = board_with_groups((InternalPadGroup(("1",)), InternalPadGroup(("2",))))
    board = replace(board, nets=(*board.nets, PhysicalNet("GND", (PadReference("BT1", "2"),))))
    graph = explicit_copper_connectivity(board)
    assert graph.pad_connected(PadReference("BT1", "2"))
    assert graph.roots[graph.pad_nodes[PadReference("BT1", "1")][0]] != graph.roots[graph.pad_nodes[PadReference("BT1", "2")][0]]


def test_group_validation():
    with pytest.raises(ValueError, match="different nets"):
        replace(board_with_groups(), footprints={"clip": replace(board_with_groups().footprints["clip"],
                    internal_pad_groups=(InternalPadGroup(("1", "2")),))},
                nets=(PhysicalNet("V", (PadReference("BT1", "1"),)),
                      PhysicalNet("GND", (PadReference("BT1", "2"),))))
    for groups in ((InternalPadGroup(("3",)),), (InternalPadGroup(("1",)), InternalPadGroup(("1",)))):
        with pytest.raises(ValueError):
            board_with_groups(groups)


def test_import_export_roundtrip_and_group_scope():
    board = board_with_groups()
    manifest = KiCadPcbBackend().generate(board)
    assert manifest.target_version == "10.0"
    assert '(jumper_pad_groups ("1"))' in manifest.artifacts[0].content
    library = next(a for a in manifest.artifacts if a.name.endswith(".kicad_mod"))
    footprint = parse_kicad_mod(library.content).footprint
    assert footprint.internal_pad_groups == board.footprints["clip"].internal_pad_groups
    global_jumpers = library.content.replace('(duplicate_pad_numbers_are_jumpers no)', '(duplicate_pad_numbers_are_jumpers yes)').replace('(jumper_pad_groups ("1"))', '')
    assert parse_kicad_mod(global_jumpers).footprint.internal_pad_groups == (InternalPadGroup(("1",)), InternalPadGroup(("2",)))


def test_plane_stitch_needs_one_contact_per_internal_group(monkeypatch):
    from pcbir.physical import CopperZone, Stackup, PolygonRing, PolygonWithHoles, Via
    from pcbir import plane
    board = board_with_groups((InternalPadGroup(("1", "2")),))
    board = replace(board, nets=(PhysicalNet("V", (PadReference("BT1", "1"), PadReference("BT1", "2"))),),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        zones=(CopperZone("plane", "V", (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(board.outline.vertices))),))
    attempts = []
    def choice(*args):
        attempts.append(args[4].position)
        if len(attempts) == 1:
            return None  # First physical land is blocked, another is usable.
        return (), Via("V", Point.mm(10, 10), nm_from_mm(.8), nm_from_mm(.4), CopperLayer.FRONT, CopperLayer.BACK)
    monkeypatch.setattr(plane, "_stitch_land", choice)
    result = plane.stitch_zone_pads(board)
    assert result.complete and result.added_via_count == 1 and len(attempts) == 2
    monkeypatch.setattr(plane, "_stitch_land", lambda *args: None)
    result = plane.stitch_zone_pads(board, plane.PlaneStitchOptions(maximum_contact_radius_nm=0))
    assert set(result.pending_pads) == set(board.nets[0].pads)


def board_with_external_terminal():
    board = board_with_groups()
    terminal = PhysicalFootprint("terminal", (FootprintPad("1", Point(0, 0), Size.mm(.6, .6)),), Size.mm(1, 1))
    return replace(board, footprints={**board.footprints, "terminal": terminal},
        placements=(*board.placements, Placement("J1", "terminal", Point.mm(16, 5))),
        nets=(PhysicalNet("V", (PadReference("BT1", "1"), PadReference("J1", "1"))),))


def test_detailed_access_can_use_alternate_internal_land(monkeypatch):
    import pcbir.detailed as module
    from pcbir import route_global, route_detailed, GlobalRouterOptions, DetailedRouterOptions
    board = board_with_external_terminal()
    real_access = module._access_candidates
    def access(*args, **kwargs):
        if args[3] == Point.mm(3, 5):
            return ()
        return real_access(*args, **kwargs)
    monkeypatch.setattr(module, "_access_candidates", access)
    result = route_detailed(board, route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))),
                            DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=1))
    assert result.metrics.routed_net_count == 1
    assert explicit_copper_connectivity(result.board).net_connected(board.nets[0])
    assert any(Point.mm(7, 5) in (t.start, t.end) for t in result.board.tracks)
    assert not any(Point.mm(3, 5) in (t.start, t.end) for t in result.board.tracks)


def test_fanout_chooses_one_alternate_internal_land_and_verifies_it(monkeypatch):
    from pcbir import fanout
    from pcbir.pin_escape import verified_fanout_path
    from pcbir.routing_clearance import RoutingClearanceIndex
    board = board_with_external_terminal()
    legal = fanout._legal_choices
    def choices(*args, **kwargs):
        return () if args[3] == Point.mm(3, 5) else legal(*args, **kwargs)
    monkeypatch.setattr(fanout, "_legal_choices", choices)
    result = fanout.route_fanout(board, fanout.FanoutOptions(minimum_component_pads=1,
        maximum_neighbor_distance_nm=nm_from_mm(5)))
    assert result.added_via_count == 1 and not result.pending_pads
    reference = PadReference("BT1", "1")
    path = verified_fanout_path(result.board, reference, "V", result.accesses[reference],
                               RoutingClearanceIndex(result.board))
    assert path is not None and Point.mm(7, 5) in (path[0].start, path[0].end)


def test_native_schematic_retains_groups(tmp_path):
    from pcbir import KiCadSchematicBackend
    cli = native_cli()
    manifest = KiCadSchematicBackend().generate(compile_contact_fixture(tmp_path))
    assert manifest.target_version == "10.0"
    source = tmp_path / "contacts.kicad_sch"
    source.write_text(manifest.artifacts[0].content, encoding="utf-8")
    netlist = tmp_path / "contacts.xml"
    subprocess.run([cli, "sch", "export", "netlist", "--format", "kicadxml", "-o", str(netlist), str(source)],
                   check=True, capture_output=True)
    assert "jumper_pin_groups" in netlist.read_text(encoding="utf-8")


@pytest.mark.parametrize("groups", [(), (InternalPadGroup(("1",)),), (InternalPadGroup(("1", "2")),)])
def test_native_kicad_agrees(groups, tmp_path):
    cli = native_cli()
    board = board_with_groups(groups)
    path = tmp_path / "internal.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), path)
    report_path = tmp_path / "drc.json"
    subprocess.run([cli, "pcb", "drc", "--format", "json", "-o", str(report_path), str(path)], check=True, capture_output=True)
    report = json.loads(report_path.read_text())
    assert bool(report["unconnected_items"]) == (not bool(groups))
    assert not report["violations"]
