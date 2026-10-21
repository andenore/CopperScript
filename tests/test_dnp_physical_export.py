"""Language non-assembly intent survives native export without losing copper."""
import csv
from pathlib import Path
import shutil
import subprocess
from pcbir import compile_source, prototype_physicalize, KiCadPcbBackend
from pcbir.assembly import snapshot
from pcbir.importers.kicad_mod import _children, _first, _parse_sexpr


def test_fitted_and_dnp_using_same_geometry_remain_distinct(tmp_path):
    (tmp_path / 'copper.mod').write_text('module dnp-test\n')
    (tmp_path / 'parts').mkdir()
    pins = 'pin A { number = "1"; domains = "analog"; directions = "passive"; } pin B { number = "2"; domains = "analog"; directions = "passive"; }'
    (tmp_path / 'parts/fitted.copper').write_text('part Fitted { footprint = "same"; ' + pins + '}\n')
    (tmp_path / 'parts/dnp.copper').write_text('part DNP { footprint = "same"; assembled = false; ' + pins + '}\n')
    electrical = compile_source('''board Test {
        import p "./parts";
        component R: p.Fitted; component C: p.DNP;
        net SIGNAL { R.A; C.A; } net GND { R.B; C.B; }
    }''', str(tmp_path / 'board.copper'), offline=True)
    assert {s.reference for s in snapshot(electrical).selections} == {'R'}
    physical = prototype_physicalize(electrical)
    placements = {p.reference: p for p in physical.placements}
    fitted = physical.footprints[placements['R'].footprint]
    dnp = physical.footprints[placements['C'].footprint]
    assert fitted.name != dnp.name
    assert fitted.pads == dnp.pads
    assert not fitted.exclude_from_bom and not fitted.exclude_from_pos_files
    assert dnp.exclude_from_bom and dnp.exclude_from_pos_files
    assert len(physical.nets[0].pads) == len(physical.nets[1].pads) == 2
    pcb = KiCadPcbBackend().generate(physical).artifacts[0].content
    tree = _parse_sexpr(pcb, 'generated')
    attributes = {}
    for fp in _children(tree, 'footprint'):
        ref = next(p[2] for p in _children(fp, 'property') if p[1] == 'Reference')
        attributes[ref] = _first(fp, 'attr')
    assert 'exclude_from_bom' in attributes['C']
    assert 'exclude_from_pos_files' in attributes['C']
    assert 'exclude_from_bom' not in attributes['R']
    # When available, check actual native CPL behavior, not just our attr text.
    cli = shutil.which('kicad-cli') or 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe'
    if Path(cli).is_file():
        from pcbir.backends.kicad_project import write_kicad_project
        board_path, cpl = tmp_path / 'dnp.kicad_pcb', tmp_path / 'dnp.csv'
        write_kicad_project(KiCadPcbBackend().generate(physical), board_path)
        subprocess.run([cli, 'pcb', 'export', 'pos', '--format', 'csv',
            '--side', 'front', '-o', str(cpl), str(board_path)],
            check=True, capture_output=True, text=True, timeout=30)
        with cpl.open(newline='', encoding='utf-8-sig') as stream:
            assert {row['Ref'] for row in csv.DictReader(stream)} == {'R'}
