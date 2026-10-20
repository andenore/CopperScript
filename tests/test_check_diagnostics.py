"""Located diagnostics, did-you-mean hints, JSON fixes and `copper explain`."""

import json
import subprocess
import sys
from pathlib import Path

from pcbir.compiler import compile_source
from pcbir.diagnostics import EXPLANATIONS, SourceLocator, explain, locate
from pcbir.erc import check

ROOT = Path(__file__).parents[1]

TYPO = """board Typo {
    use library "tiny";
    component PS1: VOLTAGE_SOURCE;
    component U1: STM32_LIKE;
    component R1: RESISTOR { value = 10kohm; }
    net V3V3 { PS1.OUT; U1.VDD; R1.1; }
    net GND { PS1.GND; U1.VSS; }
    net SDA { U1.PB77; R1.2; }
    supply V3V3 { voltage = 3.3V; source = PS1.OUT; }
    supply GND { voltage = 0V; external = true; }
}
"""


def run(*args, cwd=ROOT):
    return subprocess.run([sys.executable, "-m", "copperscript", *args], cwd=cwd, text=True,
                          capture_output=True, check=False)


def test_unknown_pin_has_hint_location_and_fix(tmp_path):
    source = tmp_path / "typo.copper"
    source.write_text(TYPO, encoding="utf-8")
    result = run("check", "typo.copper", "--json", cwd=tmp_path)
    assert result.returncode == 1
    document = json.loads(result.stdout)
    assert document["schema"] == "copperscript.check/v1" and not document["passed"]
    finding = next(item for item in document["diagnostics"] if item["code"] == "UNKNOWN_PIN")
    assert finding["hint"] == "did you mean 'PB7'?"
    assert finding["location"] == {"file": "typo.copper", "line": 8, "column": 15, "end_line": 8, "end_column": 22}
    assert finding["fix"]["replacement"] == "U1.PB7"
    line = TYPO.splitlines()[7]
    span = finding["fix"]["range"]
    assert line[span["column"] - 1:span["end_column"] - 1] == "U1.PB77"


def test_text_output_is_located_and_summarized(tmp_path):
    (tmp_path / "typo.copper").write_text(TYPO, encoding="utf-8")
    result = run("check", "typo.copper", cwd=tmp_path)
    assert "typo.copper:8:15: ERROR UNKNOWN_PIN [U1.PB77]" in result.stdout
    assert "(did you mean 'PB7'?)" in result.stdout
    assert "FAILED: 1 error" in result.stdout


def test_pins_option_controls_unmarked_severity(tmp_path):
    fixed = TYPO.replace("U1.PB77", "U1.PB7")
    (tmp_path / "b.copper").write_text(fixed, encoding="utf-8")
    warn = run("check", "b.copper", cwd=tmp_path)
    assert warn.returncode == 0 and "WARNING UNMARKED_PIN [U1]" in warn.stdout
    assert "passed ERC with 1 warning" in warn.stdout
    assert run("check", "b.copper", "--pins", "error", cwd=tmp_path).returncode == 1
    off = run("check", "b.copper", "--pins", "off", cwd=tmp_path)
    assert off.returncode == 0 and "UNMARKED_PIN" not in off.stdout


def test_compile_error_json_has_token_span_and_keyword_fix(tmp_path):
    (tmp_path / "kw.copper").write_text('board T {\n    use library "tiny";\n    componet R1: RESISTOR;\n}\n')
    result = run("check", "kw.copper", "--json", cwd=tmp_path)
    assert result.returncode == 2
    finding = json.loads(result.stdout)["diagnostics"][0]
    assert finding["code"] == "PAR002"
    assert finding["location"] == {"file": "kw.copper", "line": 3, "column": 5, "end_line": 3, "end_column": 13}
    assert finding["fix"]["replacement"] == "component"


def test_unknown_property_hint_names_close_property(tmp_path):
    (tmp_path / "p.copper").write_text(
        'board T {\n    use library "tiny";\n    component R1: RESISTOR;\n    supply V { voltag = 3V; }\n}\n')
    result = run("check", "p.copper", cwd=tmp_path)
    assert "CMP018" in result.stdout and "did you mean 'voltage'?" in result.stdout


def test_duplicates_point_at_the_second_declaration():
    source = 'board T {\n    use library "tiny";\n    component R1: RESISTOR;\n    component R1: RESISTOR;\n}\n'
    locator = SourceLocator(source, "d.copper")
    assert locator.span("R1", code="DUPLICATE_COMPONENT")[0].start.line == 4
    assert locator.span("R1", code="UNKNOWN_PART")[0].start.line == 3


def test_supply_subjects_prefer_supply_declarations():
    board = compile_source(TYPO.replace("net V3V3", "net V3V4"), "t.copper")
    located = locate(check(board), TYPO.replace("net V3V3", "net V3V4"), "t.copper")
    unknown = next(d for d, _ in located if d.code == "UNKNOWN_NET")
    assert unknown.location.line == 9  # `supply V3V3`, not an endpoint
    assert unknown.hint == "did you mean 'V3V4'?"


def test_hierarchical_subject_falls_back_to_instance():
    locator = SourceLocator("board B {\n    module PWR: power.Supply;\n}\n", "b.copper")
    span, exact = locator.span("PWR/U1.VDD")
    assert span.start.line == 2 and not exact


def test_every_erc_code_is_explained():
    import re
    source = (ROOT / "pcbir" / "erc.py").read_text(encoding="utf-8")
    emitted = set(re.findall(r'"([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)"', source))
    assert emitted - set(EXPLANATIONS) == set()


def test_explain_cli_and_families():
    assert explain("unmarked_pin")["code"] == "UNMARKED_PIN"
    assert explain("PAR002")["summary"].startswith("Syntax error")
    assert explain("NOT_A_CODE") is None
    result = run("explain", "UNKNOWN_PINN")
    assert result.returncode == 2 and "did you mean UNKNOWN_PIN?" in result.stdout
    assert run("explain", "--list").stdout.splitlines()[0] in EXPLANATIONS


def _located(source):
    board = compile_source(source, "t.copper")
    return [(d.code, d.subject, d.location.line if d.location else None, d.replacement)
            for d, _ in locate(check(board), source, "t.copper")]


def test_locator_never_matches_a_prefix_of_a_longer_name():
    locator = SourceLocator("board T {\n    net A { U1.CORE.VDD; }\n    net B { U1.CORE; }\n}\n", "t.copper")
    span, exact = locator.span("U1.CORE", origin="net B")
    assert span.start.line == 3 and exact
    assert locator.span("U1.CORE", origin="net A") == (locator.span("U1.CORE", origin="net B")[0], False)


def test_repeated_typos_get_distinct_ranges():
    source = TYPO.replace("net SDA { U1.PB77; R1.2; }", "net SDA { U1.PB77; R1.2; }\n    net SCL { U1.PB77; }")
    rows = [row for row in _located(source) if row[0] == "UNKNOWN_PIN"]
    assert [row[2] for row in rows] == [8, 9] and all(row[3] == "U1.PB7" for row in rows)


def test_duplicate_no_connects_point_at_each_repeat():
    source = TYPO.replace("    supply V3V3", '    no_connect U1.PA0;\n    no_connect U1.PA0;\n'
                          '    no_connect U1.PA0;\n    supply V3V3')
    assert [row[2] for row in _located(source) if row[0] == "DUPLICATE_NO_CONNECT"] == [10, 11]


def test_hierarchical_names_are_never_offered_as_fixes():
    path = ROOT / "examples" / "hierarchical_board" / "board.copper"
    text = path.read_text(encoding="utf-8")
    first_net = text.index("    net ")
    text = text[:first_net] + "    no_connect PWR.VIN;\n" + text[first_net:].replace("PS1.OUT;", "PS11.OUT;", 1)
    board = compile_source(text, str(path), locked=True, offline=True)
    located = [d for d, _ in locate(check(board, unmarked_pins=True), text, "board.copper")]
    port = next(d for d in located if d.subject == "PWR.VIN")
    assert port.code == "NO_CONNECT_MODULE_PORT" and port.replacement is None
    assert all("/" not in (d.replacement or "") for d in located)
    typo = next(d for d in located if d.subject == "PS11.OUT")
    assert typo.replacement == "PS1.OUT"
    assert not any(d.code == "UNMARKED_PIN" and "/" in d.subject for d in located)


def test_plain_string_form_has_no_hint_suffix():
    board = compile_source(TYPO, "t.copper")
    unknown = next(d for d in check(board) if d.code == "UNKNOWN_PIN")
    assert str(unknown) == "ERROR UNKNOWN_PIN [U1.PB77]: part STM32_LIKE has no pin 'PB77'"


def test_constraints_naming_unknown_components_or_pins_are_errors():
    source = TYPO.replace("    net SDA { U1.PB77; R1.2; }\n", "    net SDA { U1.VDD; R1.2; }\n").replace(
        "    supply GND", "    constraint max_distance(R1, GHOST) { distance = 3mm; }\n"
                         "    constraint align(R1, U1.VDDD) { axis = \"x\"; }\n"
                         "    constraint max_distance(R1, U1) { distance = 5mm; }\n    supply GND")
    findings = [d for d in check(compile_source(source, "constraints.copper"))
                if d.code == "UNKNOWN_CONSTRAINT_TARGET"]
    assert [d.subject for d in findings] == ["GHOST", "U1.VDDD"]
    assert findings[1].replacement == "U1.VDD" and "did you mean 'VDD'" in findings[1].hint
    located = dict((d.subject, span) for d, span in locate(findings, source, "constraints.copper"))
    line = source.splitlines()[located["GHOST"].start.line - 1]
    assert "max_distance(R1, GHOST)" in line
    assert explain("UNKNOWN_CONSTRAINT_TARGET")
