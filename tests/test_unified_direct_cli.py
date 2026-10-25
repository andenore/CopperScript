"""Direct unified mode must stay direct through the complete CLI/export path."""
import json
import pytest
from pathlib import Path

import pcbir.cli as cli
import pcbir.fanout as fanout
import pcbir.flow as flow


@pytest.mark.parametrize('search_states', (1, 2_000_000))
def test_direct_cli_has_no_legacy_contact_or_native_repair_routing(tmp_path, monkeypatch, search_states):
    source = tmp_path / "board.copper"
    source.write_text('''board DirectCli {
        silkscreen { branding { enabled = false; } }
        use library "tiny";
        component R1: RESISTOR { footprint="Fixture:Two"; }
        component R2: RESISTOR { footprint="Fixture:Two"; }
        net POWER { R1.1; R2.1; }
        net DATA { R1.2; R2.2; }
        mechanical { outline rectangle { width=20mm; height=12mm; } }
        constraint fixed_placement(R1) { x=3mm; y=6mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17mm; y=6mm; rotation=0; side=front; }
        constraint copper_zone(POWER) { layers="F.Cu"; x=1mm; y=1mm; width=18mm; height=10mm;
            pad_connection=solid; island_policy=remove_all; }
    }''', encoding="utf-8")
    library = tmp_path / "footprints" / "Fixture.pretty"
    library.mkdir(parents=True)
    (library / "Two.kicad_mod").write_text('''(footprint "Two" (layer "F.Cu")
        (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
        (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask")))''', encoding="utf-8")
    def forbidden(*args, **kwargs):
        raise AssertionError("direct CLI invoked legacy routing")
    monkeypatch.setattr(cli, "stitch_zone_pads", forbidden)
    monkeypatch.setattr(flow, "route_critical_nets", forbidden)
    monkeypatch.setattr(flow, "_plan_decouplers", forbidden)
    monkeypatch.setattr(fanout, "route_fanout", forbidden)
    actual_verify = cli.improve_filled_copper
    observed = []
    def verify_only(*args, **kwargs):
        observed.append(kwargs["options"].maximum_trials)
        assert observed[-1] == 0
        return actual_verify(*args, **kwargs)
    monkeypatch.setattr(cli, "improve_filled_copper", verify_only)
    report = tmp_path / "route.json"
    code = cli.main(["route-board", str(source), "--footprint-root", str(library.parent),
        "--router", "unified", "--unified-direct", "--unified-time-budget-s", "10",
        "--unified-search-states", str(search_states),
        "--unified-save-inputs", str(tmp_path / "inputs.state.json"),
        "--checkpoint-dir", str(tmp_path / "checkpoint"),
        "--candidates", "1", "--feedback-iterations", "1", "--router-iterations", "1",
        "--verify-plane-fill", str(tmp_path / "missing-kicad.exe"),
        "--report", str(report), "-o", str(tmp_path / "board.kicad_pcb")])
    assert code != 0  # A missing verifier remains a failure, never a routing success.
    data = json.loads(report.read_text())
    assert observed == [0]
    assert data["work_limits"]["router"] == "unified"
    assert data["work_limits"]["unified_direct"] is True
    assert data["work_limits"]["filled_copper_trials"] == 0
    assert data["filled_copper_feedback"]["stop_reason"] == "missing_tool"

    from pcbir.workbench import load_state
    saved = load_state(tmp_path / "inputs.state.json")
    final = load_state(tmp_path / "checkpoint" / "final-board.state.json")
    from pcbir.route_summary import connection_summary
    from pcbir.drc import run_physical_drc
    assert connection_summary(final, run_physical_drc(final))["signal"] == data["summary"]["signal"]
    assert saved['options'].maximum_expansions == search_states
    assert data['work_limits']['unified_search_states'] == search_states
    assert (data['summary']['signal']['opens'] == 0) == (search_states > 1)


def test_unified_search_state_budget_must_be_positive():
    from pcbir.unified_router import UnifiedRouterOptions
    with pytest.raises(ValueError, match='search states'):
        UnifiedRouterOptions(maximum_expansions=0)
