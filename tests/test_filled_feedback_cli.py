"""Source-to-native CLI integration using explicitly synthetic test lands."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from pcbir.cli import _apply_routing_profile, _parser


def test_profiles_are_bounded_and_explicit_options_win():
    assert _parser().parse_args(["route-board", "board.copper"]).stitch_surface_zones
    assert not _parser().parse_args(["route-board", "board.copper",
                                    "--no-stitch-surface-zones"]).stitch_surface_zones
    for name, limits in (("quick", (1,10000,2,4,2,2000)),
                         ("balanced", (2,30000,8,16,8,4000)),
                         ("thorough", (4,100000,16,32,16,12000))):
        argv = ["route-board", "board.copper", "--routing-profile", name]
        args = _parser().parse_args(argv)
        _apply_routing_profile(args, argv)
        assert tuple(getattr(args, field) for field in (
            "passes", "search_budget", "package_access_trials", "package_placement_probes",
            "filled_copper_trials", "filled_copper_search_states")) == limits
        argv += ["--package-access-trials=0", "--filled-copper-trials", "0", "--passes", "7"]
        args = _parser().parse_args(argv)
        _apply_routing_profile(args, argv)
        assert args.package_access_trials == args.filled_copper_trials == 0 and args.passes == 7


@pytest.mark.parametrize("missing_tool", (False, True))
def test_standard_cli_refills_repairs_or_saves_explicit_tool_failure(tmp_path, missing_tool):
    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file() and not missing_tool:
        pytest.skip("native CLI required")
    source = tmp_path / "board.copper"
    source.write_text('''board NativeFeedback {
        use library "tiny";
        component R1: RESISTOR { footprint="Fixture:Divider"; }
        component R2: RESISTOR { footprint="Fixture:Terminal"; }
        net POWER { R1.1; R2.1; }
        net DATA { R1.2; R2.2; }
        mechanical { outline rectangle { width=20mm; height=12mm; } }
        // This compact routing fixture has no artwork clearance reservation.
        silkscreen { branding { enabled = false; } }
        constraint fixed_placement(R1) { x=3mm; y=6mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17mm; y=6mm; rotation=0; side=front; }
        constraint copper_zone(POWER) { layers="F.Cu"; x=1mm; y=1mm; width=18mm; height=10mm;
            pad_connection=solid; island_policy=remove_all; }
    }''', encoding="utf-8")
    root = tmp_path / "footprints" / "Fixture.pretty"
    root.mkdir(parents=True)
    for name in ("Divider", "Terminal"):
        divider = '''(zone (net 0) (net_name "") (layer "F.Cu")
            (hatch edge 0.5) (connect_pads (clearance 0)) (min_thickness 0.25)
            (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed))
            (polygon (pts (xy 6.5 -6) (xy 7.5 -6) (xy 7.5 6) (xy 6.5 6))))''' if name == "Divider" else ""
        (root / (name + ".kicad_mod")).write_text(f'''(footprint "{name}" (layer "F.Cu")
            (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
            (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
            (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
            {divider})''', encoding="utf-8")
    report, pcb = tmp_path / "reports" / "route.json", tmp_path / "candidate.kicad_pcb"
    result = subprocess.run([sys.executable, "-m", "copperscript", "route-board", str(source),
        "--footprint-root", str(root.parent),
        "--routing-profile", "quick", "--pitch-mm", "0.5", "--progress",
        "--verify-plane-fill", str(tmp_path / "missing.exe" if missing_tool else cli),
        "--report", str(report), "-o", str(pcb)], capture_output=True, text=True,
        cwd=Path(__file__).parents[1], timeout=120)
    assert report.is_file(), result.stdout + result.stderr
    document = json.loads(report.read_text())
    assert document["fabrication_ready"] is False and pcb.is_file()
    assert document["routing_profile"] == "quick" and document["work_limits"]["filled_copper_trials"] == 2
    feedback = document["filled_copper_feedback"]
    if missing_tool:
        assert result.returncode != 0 and document["status"] == "fail"
        assert feedback["stop_reason"] == "missing_tool" and feedback["verification"] is None
    else:
        assert result.returncode == 0, result.stdout + result.stderr + json.dumps(feedback)
        assert document["plane_verification"]["passed"]
        assert feedback["stop_reason"] == "verified"
        assert any(item["outcome"] == "accepted" for item in feedback["trials"])
        assert Path(feedback["verification"]["artifact_directory"]).is_dir()
