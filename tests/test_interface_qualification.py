import json

import pytest

from pcbir.engineering_qualification import REQUIRED, load_contract, assess_engineering
from pcbir.qualification import aggregate
from pcbir.qualification_gate import qualification_gate
from test_qualification_gate import setup_reports


def contract():
    return dict(schema="copperscript-engineering-contract/v0.1", id="usb", kind="interface", protocol="USB 2.0",
        sources=[], requirements=[dict(id=name, stage="bench" if name.startswith("prototype-") else
            "simulation" if name in {"impedance", "signal-integrity"} else "design",
            algorithm="external", description="Whole channel including inline components and applicable mode")
            for name in sorted(REQUIRED["interface"])])


@pytest.mark.parametrize("name", sorted(REQUIRED["interface"]))
@pytest.mark.parametrize("mutation", ["omit", "calculation", "wrong-stage"])
def test_interface_requirements_cannot_be_removed_or_weakened(tmp_path, name, mutation):
    c = contract()
    r = next(r for r in c["requirements"] if r["id"] == name)
    if mutation == "omit":
        c["requirements"].remove(r)
    elif mutation == "calculation":
        r["algorithm"] = "calculation"
    else:
        r["stage"] = "design" if r["stage"] != "design" else "simulation"
    p = tmp_path / "interface.json"
    p.write_text(json.dumps(c))
    with pytest.raises(ValueError):
        load_contract(p)


def test_missing_interface_protocol_is_rejected(tmp_path):
    c = contract()
    c.pop("protocol")
    p = tmp_path / "interface.json"
    p.write_text(json.dumps(c))
    with pytest.raises(ValueError, match="identify its protocol"):
        load_contract(p)


def test_missing_usb_bench_check_cannot_be_hidden_in_report(tmp_path):
    root, plan, cam, engineering = setup_reports(tmp_path)
    (tmp_path / "contract.json").write_text(json.dumps(contract()))
    r = assess_engineering(root / "round.kicad_pcb", plan)
    assert {f"fixture.{name}" for name in REQUIRED["interface"]} <= {c["id"] for c in r["checks"]}
    r["checks"] = [c for c in r["checks"] if c["id"] != "fixture.prototype-interface"]
    r["status"] = aggregate(r["checks"])
    engineering.write_text(json.dumps(r))
    gate = qualification_gate(root, plan, cam, engineering)
    coverage = next(c for c in gate["checks"] if c["id"] == "rf-power.coverage")
    assert coverage["status"] == "incomplete"
    assert coverage["metrics"]["missing"] == ["fixture.prototype-interface"]
    assert gate["qualified_release"] is False
