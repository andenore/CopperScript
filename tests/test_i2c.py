from dataclasses import replace

from pcbir import Interface, InterfaceKind, check

from test_erc import load_python_fixture


def test_each_i2c_signal_requires_its_own_pullup() -> None:
    board = load_python_fixture("valid_board.py")
    board = replace(
        board,
        components=tuple(component for component in board.components if component.ref != "R2"),
        nets=tuple(
            replace(net, endpoints=tuple(ep_ for ep_ in net.endpoints if ep_.component != "R2"))
            for net in board.nets
        ),
    )
    missing = [item for item in check(board) if item.code == "I2C_MISSING_PULLUP"]
    assert len(missing) == 1
    assert "SCL" in missing[0].message


def test_i2c_binding_must_be_on_declared_signal_net() -> None:
    board = load_python_fixture("valid_board.py")
    broken_interface = Interface(
        name="SENSOR_I2C",
        kind=InterfaceKind.I2C,
        signals={"sda": "I2C_SDA", "scl": "I2C_SCL"},
        bindings={
            "U2": {"sda": "PA0", "scl": "PB6"},
            "U3": {"sda": "SDA", "scl": "SCL"},
        },
        pullup_supply="V3V3",
    )
    board = replace(board, interfaces=(broken_interface,))
    assert "I2C_BINDING_NOT_ON_NET" in {item.code for item in check(board)}
