"""route-board names the legacy-only flags that a --router unified run ignores."""
import json

import pcbir.cli as cli


def _board(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text('''board RouterFlags {
        silkscreen { branding { enabled = false; } }
        use library "tiny";
        component R1: RESISTOR { footprint="Fixture:Two"; }
        component R2: RESISTOR { footprint="Fixture:Two"; }
        net POWER { R1.1; R2.1; }
        net DATA { R1.2; R2.2; }
        mechanical { outline rectangle { width=20mm; height=12mm; } }
        constraint fixed_placement(R1) { x=3mm; y=6mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17mm; y=6mm; rotation=0; side=front; }
    }''', encoding="utf-8")
    library = tmp_path / "footprints" / "Fixture.pretty"
    library.mkdir(parents=True)
    (library / "Two.kicad_mod").write_text('''(footprint "Two" (layer "F.Cu")
        (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
        (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask")))''', encoding="utf-8")
    return source, library.parent


def _route(tmp_path, source, footprints, name, *extra):
    report = tmp_path / f"{name}.json"
    code = cli.main(["route-board", str(source), "--footprint-root", str(footprints),
        "--router", "unified", "--unified-time-budget-s", "5",
        "--candidates", "1", "--feedback-iterations", "1", "--router-iterations", "1",
        "--progress", "--report", str(report), *extra])
    return code, json.loads(report.read_text())


def _router_events(output):
    return [json.loads(line[len("PROGRESS "):]) for line in output.splitlines()
            if line.startswith("PROGRESS ") and '"router_options"' in line]


def test_unified_route_board_reports_ignored_legacy_flags(tmp_path, capsys):
    source, footprints = _board(tmp_path)
    code, data = _route(tmp_path, source, footprints, "legacy-flags", "--passes", "7", "--search-budget", "123")
    assert code == 0
    assert data["work_limits"]["unified_ignored_flags"] == ["--passes", "--search-budget"]
    assert data["work_limits"]["unified_skipped_stage_flags"] == []
    events = _router_events(capsys.readouterr().out)
    assert [(e["event"], e["details"]["ignored_flags"]) for e in events] == [
        ("ignored", ["--passes", "--search-budget"])]

    code, data = _route(tmp_path, source, footprints, "unified-only", "--unified-iterations", "20",
                        "--no-route-smoothing")
    assert code == 0
    assert data["work_limits"]["unified_ignored_flags"] == []
    assert data["work_limits"]["unified_skipped_stage_flags"] == []
    assert _router_events(capsys.readouterr().out) == []


def test_unified_flag_notes_name_skipped_stage_flags_and_respect_scope(tmp_path):
    def notes(*extra):
        args = cli._parser().parse_args(["route-board", "board.copper", *extra])
        cli._apply_routing_profile(args, list(extra))
        return cli._unified_flag_notes(args)

    assert notes("--passes", "7") is None  # legacy router: every flag applies
    assert notes("--router", "unified") == {"ignored_flags": [], "skipped_stage_flags": []}
    assert notes("--router", "unified", "--fanout", "--no-route-smoothing",
                 "--layer-preference-cost", "2") == {"ignored_flags": [], "skipped_stage_flags": []}
    # Disabling a skipped stage is consistent with skipping it; enabling it is not.
    assert notes("--router", "unified", "--package-access-trials", "0", "--zone-escape-trials", "2",
                 "--zone-local-ripup-trials", "0", "--plan-power-areas", "--soft-ripup") == {
        "ignored_flags": ["--soft-ripup"],
        "skipped_stage_flags": ["--plan-power-areas", "--zone-escape-trials"]}
    # --pitch-mm still sets global local-demand lanes when that cost is enabled.
    assert notes("--router", "unified", "--pitch-mm", "0.5")["ignored_flags"] == ["--pitch-mm"]
    assert notes("--router", "unified", "--pitch-mm", "0.5", "--local-demand-cost", "3")["ignored_flags"] == []
    # Profile budgets that differ from the defaults are reported as well (quick keeps --passes 1).
    assert notes("--router", "unified", "--routing-profile", "quick") == {
        "ignored_flags": ["--search-budget"],
        "skipped_stage_flags": ["--package-access-trials", "--package-placement-probes"]}
