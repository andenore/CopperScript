import json
import pytest


def test_route_board_cli_reports_opt_in_plan_and_missing_native_tool(tmp_path):
    from pcbir.cli import main
    source = tmp_path/'board.copper'
    source.write_text('''board PowerStage {
        silkscreen { branding { enabled = false; } }
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component R2: RESISTOR { footprint = "0402"; }
        net POWER { R1.1; R2.1; }
        net DATA { R1.2; R2.2; }
        mechanical { outline rectangle { width = 20 mm; height = 12 mm; } }
        constraint fixed_placement(R1) { x=3 mm; y=6 mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17 mm; y=6 mm; rotation=0; side=front; }
    }''')
    config = tmp_path/'power.json'
    config.write_text(json.dumps(dict(schema='copperscript-power-area/v0.1', nets=['POWER'], candidates_per_domain=1,
        external_boundaries=[dict(net='POWER', terminal=dict(component='R1', pad='1'), role='source',
                                  provenance='Synthetic external test feed')])) )
    report = tmp_path/'route.json'
    code = main(['route-board', str(source), '--allow-proxy-footprints', '--routing-profile', 'quick',
        '--router', 'legacy', '--power-area-config', str(config), '--package-access-trials', '0', '--zone-escape-trials', '0',
        '--report', str(report), '-o', str(tmp_path/'draft.kicad_pcb')])
    assert code in (0, 1) and report.is_file()
    document = json.loads(report.read_text())
    assert document['power_area_planning']['plan']['domains'][0]['net'] == 'POWER'
    assert document['power_area_planning']['final_observation']['status'] == 'native_tool_not_requested'


def test_route_board_native_partition_verifies_source_baseline(tmp_path):
    import shutil
    from pcbir.cli import main
    cli = shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable')
    footprints = tmp_path/'P.pretty'
    footprints.mkdir()
    (footprints/'terminal.kicad_mod').write_text('''(footprint "terminal" (version 20240108) (generator pcbnew)
        (layer "F.Cu") (attr smd)
        (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
        (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask")))''')
    source = tmp_path/'board.copper'
    source.write_text('''board PowerPartition {
        silkscreen { branding { enabled = false; } }
        use library "tiny";
        component R1: RESISTOR { footprint = "P:terminal"; }
        component R2: RESISTOR { footprint = "P:terminal"; }
        net POWER { R1.1; R2.1; }
        net HOST { R1.2; R2.2; }
        supply HOST { voltage = 1.8 V; external = true; }
        mechanical { outline rectangle { width = 20 mm; height = 12 mm; } }
        constraint fixed_placement(R1) { x=3 mm; y=6 mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17 mm; y=6 mm; rotation=0; side=front; }
        constraint copper_zone(HOST) { id="host"; layers="F.Cu";
            x=1 mm; y=1 mm; width=18 mm; height=10 mm; pad_connection=solid; }
    }''')
    config = tmp_path/'power.json'
    config.write_text(json.dumps(dict(schema='copperscript-power-area/v0.1', nets=['POWER'], candidates_per_domain=1,
        partitionable_zone_ids=['host'], external_boundaries=[dict(net='POWER', terminal=dict(component='R1', pad='1'),
            role='source', provenance='Synthetic external test feed')])))
    report = tmp_path/'route.json'
    code = main(['route-board', str(source), '--footprint-root', str(tmp_path), '--routing-profile', 'quick',
        '--router', 'legacy', '--power-area-config', str(config), '--package-access-trials', '0', '--zone-escape-trials', '0',
        '--filled-copper-trials', '0', '--verify-plane-fill', cli,
        '--report', str(report), '-o', str(tmp_path/'draft.kicad_pcb')])
    assert code in (0, 1) and report.is_file()
    document = json.loads(report.read_text())
    power = document['power_area_planning']
    assert power['plan']['partitioned_zone_ids'] == ['host']
    assert power['final_observation']['partition_check']['status'] == 'preserved'
    assert power['final_observation']['status'] == 'native_connected'
