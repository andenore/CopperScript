from pathlib import Path
import shutil

import pytest

from pcbir import (
    audit_resolved_footprints,
    FootprintResolutionError,
    FootprintResolver,
    compile_source,
    resolved_physicalize,
)


ROOT = Path(__file__).parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "footprints" / "R_0402_Test.kicad_mod"


def test_resolves_direct_path_relative_to_board(tmp_path: Path) -> None:
    destination = tmp_path / FIXTURE.name
    shutil.copyfile(FIXTURE, destination)

    result = FootprintResolver(tmp_path).resolve(FIXTURE.name)

    assert result.footprint.name == FIXTURE.name
    assert result.footprint.source_library_id == FIXTURE.name
    assert result.footprint.metadata["source_footprint_name"] == "R_0402_Test"
    assert result.footprint.metadata["resolved_reference"] == FIXTURE.name


def test_resolves_kicad_library_identifier_from_explicit_root(
    tmp_path: Path,
) -> None:
    library = tmp_path / "Resistor_SMD.pretty"
    library.mkdir()
    shutil.copyfile(FIXTURE, library / FIXTURE.name)

    result = FootprintResolver(
        tmp_path / "board", search_roots=(tmp_path,)
    ).resolve("Resistor_SMD:R_0402_Test")

    assert result.footprint.name == "Resistor_SMD:R_0402_Test"


def test_rejects_ambiguous_library_resolution(tmp_path: Path) -> None:
    roots = (tmp_path / "one", tmp_path / "two")
    for root in roots:
        library = root / "Resistor_SMD.pretty"
        library.mkdir(parents=True)
        shutil.copyfile(FIXTURE, library / FIXTURE.name)

    resolver = FootprintResolver(tmp_path / "board", search_roots=roots)

    with pytest.raises(FootprintResolutionError, match="is ambiguous"):
        resolver.resolve("Resistor_SMD:R_0402_Test")


def test_rejects_filename_and_declared_name_mismatch(tmp_path: Path) -> None:
    shutil.copyfile(FIXTURE, tmp_path / "WrongName.kicad_mod")

    with pytest.raises(FootprintResolutionError, match="declares 'R_0402_Test'"):
        FootprintResolver(tmp_path).resolve("WrongName.kicad_mod")


def test_resolved_physicalizer_uses_imported_geometry(tmp_path: Path) -> None:
    shutil.copyfile(FIXTURE, tmp_path / FIXTURE.name)
    board = compile_source(
        f'''board Resolved {{
            use library "tiny";
            component R1: RESISTOR {{
                value = 10kohm;
                footprint = "{FIXTURE.name}";
            }}
            net LEFT {{ R1.1; }}
            net RIGHT {{ R1.2; }}
        }}'''
    )

    physical = resolved_physicalize(board, FootprintResolver(tmp_path))

    footprint = physical.footprints[FIXTURE.name]
    assert len(footprint.graphics) == 6
    assert physical.metadata["resolved_footprints"] == "true"
    assert "prototype_footprints" not in physical.metadata


def test_resolved_physicalizer_uses_part_default_footprint(tmp_path: Path) -> None:
    source = FIXTURE.read_text(encoding="utf-8").replace(
        'footprint "R_0402_Test"', 'footprint "0402"', 1
    )
    (tmp_path / "0402.kicad_mod").write_text(source, encoding="utf-8")
    board = compile_source(
        '''board PartDefault {
            use library "tiny";
            component R1: RESISTOR { value = 10kohm; }
        }'''
    )

    physical = resolved_physicalize(board, FootprintResolver(tmp_path))

    assert physical.placements[0].footprint == "0402"
    assert len(physical.footprints["0402"].pads) == 2


def test_resolved_physicalizer_rejects_part_pad_mismatch(tmp_path: Path) -> None:
    shutil.copyfile(FIXTURE, tmp_path / FIXTURE.name)
    board = compile_source(
        f'''board Mismatch {{
            use library "tiny";
            component U1: REGULATOR_3V3 {{
                footprint = "{FIXTURE.name}";
            }}
        }}'''
    )

    with pytest.raises(ValueError, match="incompatible.*missing pads 3"):
        resolved_physicalize(board, FootprintResolver(tmp_path))


def test_footprint_audit_reports_all_resolution_and_pin_failures(tmp_path: Path) -> None:
    shutil.copyfile(FIXTURE, tmp_path / FIXTURE.name)
    board = compile_source(
        f'''board Audit {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "{FIXTURE.name}"; }}
            component U1: REGULATOR_3V3 {{ footprint = "{FIXTURE.name}"; }}
            component C1: CAPACITOR {{ footprint = "Missing.kicad_mod"; }}
        }}'''
    )

    audit = audit_resolved_footprints(board, FootprintResolver(tmp_path))

    assert not audit.passed
    by_reference = {entry.reference: entry for entry in audit.entries}
    assert by_reference[FIXTURE.name].source_sha256
    assert "missing pads 3" in by_reference[FIXTURE.name].errors[0]
    assert "cannot resolve footprint" in by_reference["Missing.kicad_mod"].errors[0]
