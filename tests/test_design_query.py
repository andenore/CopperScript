"""Compact design queries over the compiled flat view."""

import json
import subprocess
import sys

import pytest

from pcbir.compiler import compile_source
from pcbir.design_query import QueryError, render_query, run_query

SOURCE = """board Q {
    use library "tiny";
    component PS1: VOLTAGE_SOURCE;
    component U1: STM32_LIKE;
    component C1: CAPACITOR { value = 100nF; role = decoupling; decouples = "U1.VDD"; }
    component R1: RESISTOR { value = 10kohm; }
    net V3V3 { PS1.OUT; U1.VDD; C1.1; R1.1; }
    net GND { PS1.GND; U1.VSS; C1.2; }
    net SDA { U1.PB7; R1.2; }
    no_connect U1.PB6 { reason = "reserved for SCL"; }
    supply V3V3 { voltage = 3.3V; source = PS1.OUT; }
    supply GND { voltage = 0V; external = true; }
}
"""


@pytest.fixture(scope="module")
def board():
    return compile_source(SOURCE, "q.copper")


def pin(result, name):
    return next(row for row in result["pins"] if row["pin"] == name)


def test_pins_report_nets_status_reason_and_decoupling(board):
    result = run_query(board, "pins", "U1")
    assert result["schema"] == "copperscript.query/v1"
    assert pin(result, "VDD")["nets"] == ["V3V3"] and pin(result, "VDD")["decoupled_by"] == ["C1"]
    assert pin(result, "PB6")["status"] == "no_connect" and pin(result, "PB6")["reason"] == "reserved for SCL"
    assert pin(result, "PA0")["status"] == "unmarked"
    assert "reason: reserved for SCL" in render_query(result)


def test_net_detail_lists_members_supply_and_caps(board):
    net = run_query(board, "net", "V3V3")["net"]
    assert net["supply"]["voltage"] == "3.3 V" and net["supply"]["source"] == "PS1.OUT"
    assert [m["endpoint"] for m in net["members"]] == ["PS1.OUT", "U1.VDD", "C1.1", "R1.1"]
    assert net["decoupling_capacitors"] == ["C1"]


def test_unconnected_groups_by_status(board):
    rows = {row["ref"]: row for row in run_query(board, "unconnected")["components"]}
    assert rows["U1"]["no_connect"] == [{"pin": "PB6", "reason": "reserved for SCL"}]
    assert "PA0" in rows["U1"]["unmarked"]
    assert "R1" not in rows


def test_component_and_overview_queries(board):
    components = {row["ref"]: row for row in run_query(board, "components")["components"]}
    assert components["C1"]["value"] == "100 nF" and components["C1"]["pins"]["connected"] == 2
    detail = run_query(board, "component", "C1")["component"]
    assert detail["properties"]["decouples"] == "U1.VDD"
    nets = [row["net"] for row in run_query(board, "nets")["nets"]]
    assert nets == ["V3V3", "GND", "SDA"]


def test_unknown_names_suggest_close_matches(board):
    with pytest.raises(QueryError, match="did you mean 'U1'"):
        run_query(board, "pins", "U11")
    with pytest.raises(QueryError, match="did you mean 'V3V3'"):
        run_query(board, "net", "V3V")
    with pytest.raises(QueryError, match="needs a component reference"):
        run_query(board, "pins")


def test_query_cli_json_is_compact_and_deterministic(tmp_path):
    (tmp_path / "q.copper").write_text(SOURCE, encoding="utf-8")
    command = [sys.executable, "-m", "copperscript", "query", "q.copper", "pins", "U1", "--json"]
    first = subprocess.run(command, cwd=tmp_path, text=True, capture_output=True, check=True).stdout
    second = subprocess.run(command, cwd=tmp_path, text=True, capture_output=True, check=True).stdout
    assert first == second and "\n" not in first.strip()
    assert json.loads(first)["component"] == "U1"
    missing = subprocess.run([*command[:6], "U9"], cwd=tmp_path, text=True, capture_output=True)
    assert missing.returncode == 2 and "QUERY ERROR" in missing.stdout
