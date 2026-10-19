"""Explicit no_connect intent and the unmarked-pin completeness warning."""

import json

import pytest

from pcbir.compiler import compile_source
from pcbir.elaborate import elaborate
from pcbir.erc import Severity, check
from pcbir.parser import parse
from pcbir.serializer import board_to_dict, board_to_json
from pcbir.syntax import CopperScriptError, NoConnectDecl


def board(body: str) -> str:
    return "board T {\n    use library \"tiny\";\n" + body + "\n}\n"


DIVIDER = """
    component R1: RESISTOR { value = 10kohm; }
    component R2: RESISTOR { value = 10kohm; }
    component R3: RESISTOR { value = 10kohm; }
    net VIN { R1.1; }
    net MID { R1.2; R2.1; }
    net GND { R2.2; }
    supply VIN { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
"""


def codes(diagnostics):
    return [d.code for d in diagnostics]


def test_parser_keeps_endpoints_reason_and_locations():
    document = parse(board(DIVIDER + '    no_connect R3.1, R3.2 { reason = "spare footprint"; }'), "t.copper")
    declaration = next(d for d in document.declarations if isinstance(d, NoConnectDecl))
    assert declaration.endpoints == ("R3.1", "R3.2")
    assert declaration.attributes == {"reason": "spare footprint"}
    assert [loc.column for loc in declaration.endpoint_locations] == [16, 22]


def test_semicolon_form_and_lowering():
    compiled = compile_source(board(DIVIDER + "    no_connect R3.1;\n    no_connect R3.2;"), "t.copper")
    assert [(str(item.endpoint), item.reason) for item in compiled.no_connects] == [("R3.1", None), ("R3.2", None)]
    assert check(compiled, unmarked_pins=True) == []


def test_unknown_property_and_empty_reason_are_compile_errors():
    with pytest.raises(CopperScriptError) as excinfo:
        compile_source(board(DIVIDER + '    no_connect R3.1 { why = "x"; }'), "t.copper")
    assert excinfo.value.code == "CMP018" and excinfo.value.hint == "allowed: reason"
    with pytest.raises(CopperScriptError) as excinfo:
        compile_source(board(DIVIDER + '    no_connect R3.1 { reason = ""; }'), "t.copper")
    assert excinfo.value.code == "CMP124"


def test_unmarked_pins_warn_once_per_component_and_only_when_requested():
    compiled = compile_source(board(DIVIDER), "t.copper")
    assert check(compiled) == []  # API default is unchanged
    findings = check(compiled, unmarked_pins=True)
    assert codes(findings) == ["UNMARKED_PIN"]
    assert findings[0].severity is Severity.WARNING
    assert findings[0].subject == "R3"
    assert "2 pins" in findings[0].message and "no_connect R3.1, R3.2" in findings[0].hint


def test_connected_no_connect_unknown_pin_and_duplicates():
    compiled = compile_source(board(DIVIDER + """
    no_connect R2.2 { reason = "wrong"; }
    no_connect R3.1, R3.1;
    no_connect R3.9;
    no_connect R9.1;"""), "t.copper")
    findings = check(compiled, unmarked_pins=True)
    by_code = {d.code: d for d in findings}
    assert by_code["NO_CONNECT_CONNECTED"].subject == "R2.2"
    assert "GND" in by_code["NO_CONNECT_CONNECTED"].message
    assert by_code["DUPLICATE_NO_CONNECT"].severity is Severity.WARNING
    assert by_code["UNKNOWN_PIN"].subject == "R3.9"
    assert by_code["UNKNOWN_COMPONENT"].subject == "R9.1"
    assert by_code["UNMARKED_PIN"].message.endswith(": 2")


def test_module_no_connects_are_qualified_by_instance():
    from pcbir.model import Board, ComponentInstance, Endpoint, ModuleDefinition, ModuleInstance, Net, NoConnect, PinType
    from copperscript.library_data import tiny_library
    library = tiny_library()
    module = ModuleDefinition(
        name="Spare", ports={"A": PinType.PASSIVE}, library=library,
        components=(ComponentInstance("R1", "RESISTOR"),), module_instances=(),
        nets=(Net("A", (Endpoint("port", "A"), Endpoint("R1", "1"))),),
        no_connects=(NoConnect(Endpoint("R1", "2"), "test pad"),),
    )
    top = Board(name="Top", library=library, components=(), nets=(Net("X", (Endpoint("S1", "A"),)),),
                module_instances=(ModuleInstance("S1", "Spare"),), module_definitions={"Spare": module})
    flat = elaborate(top)
    assert [(str(item.endpoint), item.reason) for item in flat.no_connects] == [("S1/R1.2", "test pad")]
    assert "UNMARKED_PIN" not in codes(check(top, unmarked_pins=True))


def test_ir_only_mentions_no_connects_when_declared():
    plain = compile_source(board(DIVIDER), "t.copper")
    assert "no_connects" not in board_to_dict(plain)
    marked = compile_source(board(DIVIDER + '    no_connect R3.1 { reason = "spare"; }'), "t.copper")
    assert json.loads(board_to_json(marked))["no_connects"] == [{"endpoint": "R3.1", "reason": "spare"}]


def test_no_connect_rejects_module_boundary_ports():
    from pcbir.compiler import _no_connects
    from pcbir.syntax import SourceLocation
    location = SourceLocation("m.copper", 0, 1, 1)
    with pytest.raises(CopperScriptError) as excinfo:
        _no_connects(NoConnectDecl(location, ("port.A",), {}, (location,)))
    assert excinfo.value.code == "CMP125"
