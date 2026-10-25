"""route-board names the legacy-only flags that a unified-router run (the default) ignores."""
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


def _route(tmp_path, source, footprints, name, *extra, router=("--router", "unified")):
    report = tmp_path / f"{name}.json"
    code = cli.main(["route-board", str(source), "--footprint-root", str(footprints),
        *router, "--unified-time-budget-s", "5",
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

    # Without --router, route-board runs (and records) the unified router.
    code, data = _route(tmp_path, source, footprints, "default-router", router=())
    assert code == 0
    assert data["work_limits"]["router"] == "unified"


def test_unified_flag_notes_name_skipped_stage_flags_and_respect_scope(tmp_path):
    def notes(*extra):
        args = cli._parser().parse_args(["route-board", "board.copper", *extra])
        cli._apply_routing_profile(args, list(extra))
        return cli._unified_flag_notes(args)

    assert notes("--router", "legacy", "--passes", "7") is None  # legacy router: every flag applies
    # The unified router is the default.
    assert notes("--passes", "7") == {"ignored_flags": ["--passes"], "skipped_stage_flags": [],
                                     "mapped_flags": []}
    assert notes("--router", "unified") == {"ignored_flags": [], "skipped_stage_flags": [], "mapped_flags": []}
    assert notes("--router", "unified", "--fanout", "--no-route-smoothing",
                 "--layer-preference-cost", "2") == {"ignored_flags": [], "skipped_stage_flags": [],
                                                       "mapped_flags": []}
    # Disabling a skipped stage is consistent with skipping it; enabling it is not.
    assert notes("--router", "unified", "--package-access-trials", "0", "--zone-escape-trials", "2",
                 "--zone-local-ripup-trials", "0", "--plan-power-areas", "--soft-ripup") == {
        "ignored_flags": ["--soft-ripup"],
        "skipped_stage_flags": ["--plan-power-areas", "--zone-escape-trials"], "mapped_flags": []}
    # --pitch-mm still sets global local-demand lanes when that cost is enabled.
    assert notes("--router", "unified", "--pitch-mm", "0.5")["ignored_flags"] == ["--pitch-mm"]
    assert notes("--router", "unified", "--pitch-mm", "0.5", "--local-demand-cost", "3")["ignored_flags"] == []
    # Profile budgets that differ from the defaults are reported as well (quick keeps --passes 1).
    assert notes("--router", "unified", "--routing-profile", "quick") == {
        "ignored_flags": ["--search-budget"],
        "skipped_stage_flags": ["--package-access-trials", "--package-placement-probes"], "mapped_flags": []}
    # The legacy placement controllers' budgets map onto the congestion feedback loop.
    assert notes("--router", "unified", "--critical-feedback-trials", "2", "--detailed-feedback-trials", "0") == {
        "ignored_flags": [], "skipped_stage_flags": [], "mapped_flags": ["--critical-feedback-trials"]}
    args = cli._parser().parse_args(["route-board", "board.copper", "--critical-feedback-trials", "2",
                                     "--detailed-feedback-trials", "3", "--congestion-feedback-trials", "4"])
    assert cli._congestion_feedback_budget(args) == 5
    args.router = "legacy"
    assert cli._congestion_feedback_budget(args) == 0


BYPASS_SOURCE = '''board MappedFlags {
    silkscreen { branding { enabled = false; } }
    use library "tiny";
    component U: REGULATOR_3V3 { footprint = "SOT-23-3"; }
    component C: CAPACITOR { footprint = "0402"; role = decoupling; decouples = "U.IN"; }
    component R: RESISTOR { footprint = "0402"; }
    net P { U.IN; C.1; R.1; } net G { U.GND; C.2; } net O { U.OUT; R.2; }
    supply GROUND { net = G; voltage = 0V; external = true; }
    supply VIN { net = P; voltage = 5V; external = true; }
    mechanical { outline rectangle { width=20mm; height=12mm; } }
}'''


def test_unified_critical_feedback_trials_map_onto_the_congestion_loop(tmp_path, capsys):
    # Before the congestion loop, the unified path handed its bypass-reserved
    # critical board to the legacy critical controller, whose baseline check
    # stopped the run ("stale or inconsistent" / "cannot rebuild package access").
    source = tmp_path / "bypass.copper"
    source.write_text(BYPASS_SOURCE, encoding="utf-8")
    report = tmp_path / "mapped.json"
    code = cli.main(["route-board", str(source), "--allow-proxy-footprints", "--router", "unified",
        "--unified-time-budget-s", "5", "--candidates", "1", "--feedback-iterations", "1",
        "--router-iterations", "1", "--progress", "--critical-feedback-trials", "2", "--report", str(report)])
    assert code == 0
    data = json.loads(report.read_text())
    assert data["work_limits"]["unified_mapped_flags"] == ["--critical-feedback-trials"]
    assert data["work_limits"]["unified_ignored_flags"] == []
    assert data["work_limits"]["congestion_feedback_trials"] == 4  # max(default 4, mapped 2)
    assert "critical_placement_feedback" not in data
    feedback = data["placement_feedback"]
    assert feedback["stop_reason"] == "no_failed_nets" and feedback["trials"] == []
    events = _router_events(capsys.readouterr().out)
    assert [(e["event"], e["details"]["mapped_flags"]) for e in events] == [
        ("mapped", ["--critical-feedback-trials"])]

    # The legacy router keeps its own controllers and has no congestion loop.
    source, footprints = _board(tmp_path)
    code, data = _route(tmp_path, source, footprints, "legacy-mapped", "--critical-feedback-trials", "2",
                        router=("--router", "legacy"))
    assert code == 0
    assert data["work_limits"]["congestion_feedback_trials"] == 0
    assert "placement_feedback" not in data and "unified_mapped_flags" not in data["work_limits"]


def test_fine_regions_flag_reaches_the_unified_router_and_the_report(tmp_path, capsys):
    def parsed(*extra):
        args = cli._parser().parse_args(["route-board", "board.copper", *extra])
        cli._apply_routing_profile(args, list(extra))
        return args

    assert parsed().fine_regions == "arrays"
    assert cli._unified_options(parsed()).fine_regions == "arrays"
    for mode in ("off", "arrays", "dense"):
        args = parsed("--fine-regions", mode)
        assert cli._unified_options(args).fine_regions == mode
        # A unified-router flag: never reported as ignored, skipped or mapped.
        assert cli._unified_flag_notes(args) == {"ignored_flags": [], "skipped_stage_flags": [], "mapped_flags": []}
    assert cli._unified_options(parsed("--router", "legacy", "--fine-regions", "dense")) is None

    source, footprints = _board(tmp_path)
    code, data = _route(tmp_path, source, footprints, "default-fine-regions")
    assert code == 0
    assert data["work_limits"]["fine_regions"] == "arrays"
    # No ball array: the plan has no patch, and the setup event has no patch fields.
    block = data["fine_regions"]
    assert block["mode"] == "arrays" and block["patches"] == [] and block["base_pitch_mm"] == 0.25
    assert block["total_nodes"] == block["base_nodes"] == block["uniform_nodes_at_finest"]
    setups = [json.loads(line[len("PROGRESS "):]) for line in capsys.readouterr().out.splitlines()
              if line.startswith("PROGRESS ") and '"unified_setup"' in line and '"finished"' in line]
    assert setups and all(set(event["details"]) == {"nets", "grid", "seconds"} for event in setups)

    code, data = _route(tmp_path, source, footprints, "off-fine-regions", "--fine-regions", "off")
    assert code == 0
    assert data["work_limits"]["fine_regions"] == "off" and data["fine_regions"] == {"mode": "off"}
