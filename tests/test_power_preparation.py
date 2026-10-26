"""Native plane returns remain a separate, fresh, owned continuity ledger."""
from pathlib import Path
import shutil
import pytest

from pcbir.plane_verify import verify_filled_planes


def verifier(tmp_path):
    executable = shutil.which('kicad-cli')
    if executable is None:
        pytest.skip('KiCad CLI unavailable')
    calls = []
    def verify(board, stage):
        calls.append(stage)
        return verify_filled_planes(board, kicad_cli=Path(executable),
            artifact_directory=tmp_path / f'{len(calls)}-{stage}')
    return verify, calls


def test_route_board_native_return_policy_reports_final_certificate(tmp_path):
    import json
    from pcbir.cli import main
    cli = shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable')
    footprints = tmp_path / 'P.pretty'
    footprints.mkdir()
    for name, pads in (('two', [('1', -1, 0), ('2', 1, 0)]),
                       ('three', [('1', -1, 0), ('2', 1, 0), ('3', 0, -2)])):
        lines = [f'(footprint "{name}" (version 20240108) (generator pcbnew) (layer "F.Cu") (attr smd)']
        # Real fixture graphics prevent an inferred silk box along pad edges.
        lines += ['(fp_rect (start -1.5 -2.5) (end 1.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.Fab"))']
        lines += [f'(pad "{n}" smd rect (at {x} {y}) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))'
                  for n, x, y in pads]
        (footprints / f'{name}.kicad_mod').write_text('\n'.join((*lines, ')')), encoding='utf-8')
    source = tmp_path / 'board.copper'
    source.write_text('''board ReturnsCLI {
      silkscreen { branding { enabled = false; } }
      use library "tiny";
      component U: REGULATOR_3V3 { footprint = "P:three"; }
      component J: RESISTOR { footprint = "P:two"; }
      component C: CAPACITOR { footprint = "P:two"; role = decoupling; decouples = "U.IN"; }
      net RAIL { J.1; U.IN; C.1; }
      net GND { J.2; U.GND; C.2; }
      net OUT { U.OUT; }
      supply RAIL { voltage = 5 V; external = true; }
      supply GND { voltage = 0 V; external = true; }
      mechanical { outline rectangle { width = 30 mm; height = 20 mm; } }
      constraint fixed_placement(J) { x=4 mm; y=10 mm; rotation=0; side=front; }
      constraint fixed_placement(U) { x=15 mm; y=10 mm; rotation=0; side=front; }
      constraint fixed_placement(C) { x=15 mm; y=14 mm; rotation=0; side=back; }
      constraint copper_zone(GND) { id="ground"; layers="In1.Cu";
          x=1 mm; y=1 mm; width=28 mm; height=18 mm; pad_connection=solid; }
    }''', encoding='utf-8')
    config = tmp_path / 'power.json'
    config.write_text(json.dumps(dict(schema='copperscript-power-area/v0.1', nets=['RAIL'],
        plane_return_nets=['RAIL'], candidates_per_domain=1, maximum_layer_alternatives=1,
        external_boundaries=[dict(net='RAIL', terminal=dict(component='J', pad='1'), role='source',
                                  provenance='Synthetic external fixture source') ])), encoding='utf-8')
    report = tmp_path / 'route.json'
    code = main(['route-board', str(source), '--layers', '4', '--footprint-root', str(tmp_path),
        '--router', 'legacy', '--power-area-config', str(config), '--routing-profile', 'quick', '--package-access-trials', '0',
        '--zone-escape-trials', '0', '--zone-local-ripup-trials', '0', '--filled-copper-trials', '0',
        '--verify-plane-fill', cli, '--report', str(report), '-o', str(tmp_path/'draft.kicad_pcb')])
    assert code in (0, 1) and report.is_file()
    document = json.loads(report.read_text(encoding='utf-8'))
    assert document['power_area_planning']['return_preparation']['status'] == 'native_connected'
    assert document['power_area_planning']['return_preservation']['status'] == 'native_connected'
