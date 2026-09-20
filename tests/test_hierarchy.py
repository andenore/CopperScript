from pathlib import Path

import pytest

from pcbir import CopperScriptError, check, compile_file, elaborate


ROOT = Path(__file__).parents[1]


def test_hierarchical_board_elaborates_and_passes_erc() -> None:
    board = compile_file(ROOT / "examples" / "hierarchical_board.copper")

    assert check(board) == []
    assert [(instance.ref, instance.module) for instance in board.module_instances] == [
        ("PWR", "power.Buck5VTo3V3")
    ]
    assert "power.Buck5VTo3V3" in board.module_definitions
    definition = board.module_definitions["power.Buck5VTo3V3"]
    assert set(definition.ports) == {"VIN", "VOUT", "GND"}
    assert {component.ref for component in definition.components} >= {"U1", "L1", "C_IN", "C_OUT"}

    # The authoritative parent net still terminates at the module boundary.
    vbus = next(net for net in board.nets if net.name == "VBUS")
    assert "PWR.VIN" in {str(endpoint) for endpoint in vbus.endpoints}

    # Flattening is an explicit, derived view used by ERC and flat backends.
    flat = elaborate(board)
    component_refs = {component.ref for component in flat.components}
    assert {"PWR/U1", "PWR/L1", "PWR/C_IN", "PWR/C_OUT"} <= component_refs
    flat_vbus = next(net for net in flat.nets if net.name == "VBUS")
    assert "PWR/U1.VIN" in {str(endpoint) for endpoint in flat_vbus.endpoints}
    assert all(endpoint.component not in {"PWR", "port"} for net in flat.nets for endpoint in net.endpoints)


def test_module_constraints_are_qualified() -> None:
    board = compile_file(ROOT / "examples" / "hierarchical_board.copper")
    definition = board.module_definitions["power.Buck5VTo3V3"]
    local_targets = {
        target for constraint in definition.constraints for target in constraint.targets
    }
    assert "C_IN" in local_targets
    assert "U1.VIN" in local_targets

    flat = elaborate(board)
    targets = {target for constraint in flat.constraints for target in constraint.targets}
    assert "PWR/C_IN" in targets
    assert "PWR/U1.VIN" in targets


def test_unknown_module_port_is_a_compile_error(tmp_path: Path) -> None:
    package = tmp_path / "packages" / "supply"
    package.mkdir(parents=True)
    module = package / "supply.copper"
    module.write_text(
        """module Supply {
    port VIN: power_in;
    net VIN { port.VIN; }
}
""",
        encoding="utf-8",
    )
    board = tmp_path / "board.copper"
    board.write_text(
        """board Demo {
    import supply "github.com/test/modules/supply";
    module PWR: supply.Supply;
    net INPUT { PWR.DOES_NOT_EXIST; }
}
""",
        encoding="utf-8",
    )
    _write_manifest(tmp_path)

    with pytest.raises(CopperScriptError) as captured:
        compile_file(board)
    assert captured.value.code == "CMP034"


def test_module_import_cycles_are_rejected(tmp_path: Path) -> None:
    first_dir = tmp_path / "packages" / "first"
    second_dir = tmp_path / "packages" / "second"
    first_dir.mkdir(parents=True)
    second_dir.mkdir(parents=True)
    first = first_dir / "first.copper"
    second = second_dir / "second.copper"
    first.write_text(
        'module First { import second "github.com/test/modules/second"; module CHILD: second.Second; }',
        encoding="utf-8",
    )
    second.write_text(
        'module Second { import first "github.com/test/modules/first"; module CHILD: first.First; }',
        encoding="utf-8",
    )
    board = tmp_path / "board.copper"
    board.write_text(
        'board Demo { import first "github.com/test/modules/first"; module TOP: first.First; }',
        encoding="utf-8",
    )
    _write_manifest(tmp_path)

    with pytest.raises(CopperScriptError) as captured:
        compile_file(board)
    assert captured.value.code == "CMP021"


def test_all_module_ports_must_be_connected(tmp_path: Path) -> None:
    package = tmp_path / "packages" / "supply"
    package.mkdir(parents=True)
    module = package / "supply.copper"
    module.write_text(
        """module Supply {
    port VIN: power_in;
    port VOUT: power_out;
    net VIN { port.VIN; }
    net VOUT { port.VOUT; }
}
""",
        encoding="utf-8",
    )
    board = tmp_path / "board.copper"
    board.write_text(
        """board Demo {
    import supply "github.com/test/modules/supply";
    module PWR: supply.Supply;
    net INPUT { PWR.VIN; }
}
""",
        encoding="utf-8",
    )
    _write_manifest(tmp_path)

    with pytest.raises(CopperScriptError) as captured:
        compile_file(board)
    assert captured.value.code == "CMP038"


def test_modules_can_nest_recursively(tmp_path: Path) -> None:
    leaf_dir = tmp_path / "packages" / "leaf"
    branch_dir = tmp_path / "packages" / "branch"
    leaf_dir.mkdir(parents=True)
    branch_dir.mkdir(parents=True)
    (leaf_dir / "leaf.copper").write_text(
        """module Leaf {
    use library "tiny";
    port IN: passive;
    port OUT: passive;
    component R1: RESISTOR { value = 1kohm; }
    net IN { port.IN; R1.1; }
    net OUT { port.OUT; R1.2; }
}
""",
        encoding="utf-8",
    )
    (branch_dir / "branch.copper").write_text(
        """module Branch {
    import leaf "github.com/test/modules/leaf";
    port IN: passive;
    port OUT: passive;
    module FILTER: leaf.Leaf;
    net IN { port.IN; FILTER.IN; }
    net OUT { port.OUT; FILTER.OUT; }
}
""",
        encoding="utf-8",
    )
    board_path = tmp_path / "board.copper"
    board_path.write_text(
        """board Demo {
    import branch "github.com/test/modules/branch";
    module SIGNAL_PATH: branch.Branch;
    net INPUT { SIGNAL_PATH.IN; }
    net OUTPUT { SIGNAL_PATH.OUT; }
}
""",
        encoding="utf-8",
    )
    _write_manifest(tmp_path)

    board = compile_file(board_path)
    assert {component.ref for component in board.components} == set()
    assert board.module_instances[0].ref == "SIGNAL_PATH"
    assert board.module_definitions["branch.Branch"].module_instances[0].ref == "FILTER"

    flat = elaborate(board)
    assert {component.ref for component in flat.components} == {"SIGNAL_PATH/FILTER/R1"}
    assert [instance.path for instance in flat.module_instances] == [
        "SIGNAL_PATH",
        "SIGNAL_PATH/FILTER",
    ]


def _write_manifest(root: Path) -> None:
    (root / "copper.mod").write_text(
        """module github.com/test/project
require github.com/test/modules v0.1.0
replace github.com/test/modules => ./packages
""",
        encoding="utf-8",
    )
