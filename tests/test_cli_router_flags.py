"""route-board has one interface per setting and explains removed options."""
import json

import pytest

import pcbir.cli as cli
from pcbir.route_options import RETIRED_OPTIONS


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
        "--unified-time-budget-s", "5",
        "--candidates", "1", "--feedback-iterations", "1", "--router-iterations", "1",
        "--progress", "--report", str(report), *extra])
    return code, json.loads(report.read_text())


def _router_events(output):
    return [json.loads(line[len("PROGRESS "):]) for line in output.splitlines()
            if line.startswith("PROGRESS ") and '"router_options"' in line]


@pytest.mark.parametrize("flag", RETIRED_OPTIONS)
def test_retired_options_explain_replacement_before_loading_source(flag, capsys):
    assert cli.main(["route-board", "missing.copper", flag]) == 2
    assert capsys.readouterr().err.strip() == f"copper route-board: error: {flag} was removed; {RETIRED_OPTIONS[flag]}"


def test_local_demand_pitch_requires_enabled_demand(capsys):
    with pytest.raises(SystemExit) as error:
        cli.main(["route-board", "missing.copper", "--local-demand-pitch-mm", "0.5"])
    assert error.value.code == 2
    assert "requires --local-demand-cost greater than 0" in capsys.readouterr().err


def test_expert_options_reach_effective_report(tmp_path, capsys):
    source, footprints = _board(tmp_path)
    code, data = _route(tmp_path, source, footprints, "expert", "--unified-set", "maximum_iterations=20",
                        "--unified-set", "route_smoothing=false", "--local-demand-pitch-mm", "0.5",
                        "--local-demand-cost", "3", "--layer-assignment-passes", "1")
    assert code == 0
    assert data["work_limits"]["unified_iterations"] == 20
    assert data["work_limits"]["ignored_flags"] == []
    assert data["work_limits"]["mapped_flags"] == []
    assert _router_events(capsys.readouterr().out) == []


@pytest.mark.parametrize("flag", (["--router", "unified"], ["--passes", "2"], ["--soft-ripup"],
                                  ["--package-access-trials", "0"], ["--plan-power-areas"],
                                  ["--zone-escape-trials", "0"]))
def test_removed_router_flags_are_refused(flag, capsys):
    with pytest.raises(SystemExit) as exit:
        cli._parser().parse_args(["route-board", "board.copper", *flag])
    assert exit.value.code == 2 and "unrecognized arguments" in capsys.readouterr().err


def test_one_congestion_budget_and_direct_mode_overrides():
    def parsed(*extra):
        return cli._parser().parse_args(["route-board", "board.copper", *extra])

    assert cli._congestion_feedback_budget(parsed()) == 4
    assert cli._congestion_feedback_budget(parsed("--congestion-feedback-trials", "2")) == 2
    assert cli._congestion_feedback_budget(parsed("--congestion-feedback-trials", "0")) == 0
    options = cli._unified_options(parsed("--unified-set", "direct=true"))
    assert options.direct and not options.pre_route_decouplers
    options = cli._unified_options(parsed("--unified-set", "direct=true", "--unified-set", "pre_route_decouplers=true"))
    assert options.direct and options.pre_route_decouplers


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


def test_congestion_budget_reaches_the_loop(tmp_path, capsys):
    source = tmp_path / "bypass.copper"
    source.write_text(BYPASS_SOURCE, encoding="utf-8")
    report = tmp_path / "mapped.json"
    code = cli.main(["route-board", str(source), "--allow-proxy-footprints",
        "--unified-time-budget-s", "5", "--candidates", "1", "--feedback-iterations", "1",
        "--router-iterations", "1", "--progress", "--congestion-feedback-trials", "2", "--report", str(report)])
    assert code == 0
    data = json.loads(report.read_text())
    assert data["work_limits"]["mapped_flags"] == []
    assert data["work_limits"]["ignored_flags"] == []
    assert data["work_limits"]["congestion_feedback_trials"] == 2
    assert "critical_placement_feedback" not in data
    feedback = data["placement_feedback"]
    assert feedback["stop_reason"] == "no_failed_nets" and feedback["trials"] == []
    events = _router_events(capsys.readouterr().out)
    assert events == []


def test_fine_regions_flag_reaches_the_unified_router_and_the_report(tmp_path, capsys):
    def parsed(*extra):
        args = cli._parser().parse_args(["route-board", "board.copper", *extra])
        cli._apply_routing_profile(args, list(extra))
        return args

    assert cli._unified_options(parsed()).fine_regions == "arrays"
    for mode in ("off", "arrays", "dense"):
        args = parsed("--unified-set", f"fine_regions={mode}")
        assert cli._unified_options(args).fine_regions == mode

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

    code, data = _route(tmp_path, source, footprints, "off-fine-regions", "--unified-set", "fine_regions=off")
    assert code == 0
    assert data["work_limits"]["fine_regions"] == "off" and data["fine_regions"] == {"mode": "off"}
