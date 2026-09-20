from pathlib import Path
import hashlib

import pytest

from pcbir import CopperScriptError, check, compile_file
from pcbir.packages import read_manifest


def test_manifest_local_replacement_imports_a_part(tmp_path: Path) -> None:
    package = tmp_path / "deps" / "sensors"
    package.mkdir(parents=True)
    (package / "sensor.copper").write_text(
        """part Sensor {
    kind = sensor;
    pin VDD { number = "1"; role = digital_supply; capabilities = "power_input"; voltage_min = 1.8V; voltage_max = 3.6V; }
    pin GND { number = "2"; role = ground; capabilities = "power_input"; }
}
""",
        encoding="utf-8",
    )
    (tmp_path / "copper.mod").write_text(
        """module github.com/test/board
require github.com/vendor/parts v1.2.3
replace github.com/vendor/parts => ./deps
""",
        encoding="utf-8",
    )
    board_path = tmp_path / "board.copper"
    board_path.write_text(
        """board Demo {
    import sensors "github.com/vendor/parts/sensors";
    component U1: sensors.Sensor;
    net VDD { U1.VDD; }
    net GND { U1.GND; }
    supply VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}
""",
        encoding="utf-8",
    )

    board = compile_file(board_path)

    assert check(board) == []
    assert board.components[0].part == "sensors.Sensor"
    assert board.library["sensors.Sensor"].pins["VDD"].number == "1"
    assert board.dependencies[0].version == "v1.2.3"
    assert board.dependencies[0].checksum.startswith("sha256:")
    assert not (tmp_path / "copper.sum").exists()


def test_checksum_change_is_rejected(tmp_path: Path) -> None:
    module = "github.com/vendor/library"
    version = "v1.0.0"
    cache_key = hashlib.sha256(f"{module}@{version}".encode()).hexdigest()[:20]
    package = tmp_path / ".copper-cache" / "pkg" / cache_key / "parts"
    package.mkdir(parents=True)
    part_path = package / "part.copper"
    part_path.write_text(
        'part R { kind = resistor; pin A { number = "1"; capabilities = "passive"; } }',
        encoding="utf-8",
    )
    (tmp_path / "copper.mod").write_text(
        """module example/project
require github.com/vendor/library v1.0.0
""",
        encoding="utf-8",
    )
    board_path = tmp_path / "board.copper"
    board_path.write_text(
        'board Demo { import parts "github.com/vendor/library/parts"; component R1: parts.R; net N { R1.A; } }',
        encoding="utf-8",
    )
    compile_file(board_path)
    assert (tmp_path / "copper.sum").read_text(encoding="utf-8").startswith(
        "github.com/vendor/library v1.0.0 sha256:"
    )
    part_path.write_text(
        'part R { kind = resistor; manufacturer = "changed"; pin A { number = "1"; capabilities = "passive"; } }',
        encoding="utf-8",
    )

    with pytest.raises(CopperScriptError) as captured:
        compile_file(board_path)
    assert captured.value.code == "PKG008"


def test_manifest_rejects_replacement_without_requirement(tmp_path: Path) -> None:
    manifest_path = tmp_path / "copper.mod"
    manifest_path.write_text(
        """module example/project
replace github.com/vendor/library => ./deps
""",
        encoding="utf-8",
    )

    with pytest.raises(CopperScriptError) as captured:
        read_manifest(manifest_path)
    assert captured.value.code == "PKG009"
