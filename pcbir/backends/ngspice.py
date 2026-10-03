"""Deterministic ngspice decks from a derived simulation circuit."""
from __future__ import annotations

import json
from dataclasses import dataclass

from .base import Artifact, ArtifactManifest
from ..model import Board
from ..simulation.model import Analysis, SimulationPlan
from ..simulation.select import SimulationCircuit, select_circuit


def si(value) -> str:
    return f"{float(value.base_value):.17e}"


def source_names(circuit: SimulationCircuit) -> dict[str, str]:
    return {s.name: f"{'v' if s.kind == 'voltage' else 'i'}s{i:06d}" for i, s in enumerate(circuit.plan.sources, 1)}


def generate_deck(circuit: SimulationCircuit, analysis: Analysis) -> str:
    plan = circuit.plan
    lines = ["CopperScript simulation", "* Generated from immutable electrical IR; values in SI units.",
             ".options filetype=ascii reltol=1e-5 abstol=1e-10 vntol=1e-7"]
    for model in circuit.models:
        lines.append(f'.include "{model.entrypoint}"')
    for i, element in enumerate(circuit.elements, 1):
        prefix = {"resistor": "r", "capacitor": "c", "inductor": "l", "subcircuit": "x", "diode": "d", "bjt": "q", "mosfet": "m"}[element.kind]
        nodes = " ".join(circuit.node(n) for n in element.nodes)
        value = element.model.name if element.model else si(element.value)
        lines.append(f"{prefix}e{i:06d} {nodes} {value}")
    names = source_names(circuit)
    for i, source in enumerate(plan.sources, 1):
        positive, negative = circuit.node(source.positive), circuit.node(source.negative)
        if source.series_resistance:
            internal = f"source_internal_{i:06d}"
            lines.append(f"rsource{i:06d} {internal} {positive} {si(source.series_resistance)}")
            positive = internal
        line = f"{names[source.name]} {positive} {negative} DC {si(source.dc)}"
        if source.ac_magnitude is not None:
            line += f" AC {si(source.ac_magnitude)} {source.ac_phase:.17e}"
        if source.waveform:
            line += " PWL(" + " ".join(f"{si(t)} {si(v)}" for t, v in source.waveform) + ")"
        lines.append(line)
    for node, voltage in sorted(plan.initial_voltages.items()):
        if circuit.node(node) != "0":
            lines.append(f".ic V({circuit.node(node)})={si(voltage)}")
    vectors = [f"v({n})" for n in circuit.nodes.values() if n != "0"]
    vectors += [f"i({names[s.name]})" for s in plan.sources if s.kind == "voltage"]
    lines.append(".save " + " ".join(vectors))
    if analysis.kind == "op":
        lines.append(".op")
    elif analysis.kind == "ac":
        sweep = {"decade": "dec", "octave": "oct", "linear": "lin"}[analysis.sweep]
        lines.append(f".ac {sweep} {analysis.points} {si(analysis.start)} {si(analysis.stop)}")
    else:
        uic = " UIC" if plan.initial_method == "uic" else ""
        lines.append(f".tran {si(analysis.max_step)} {si(analysis.stop)} 0 {si(analysis.max_step)}{uic}")
    return "\n".join([*lines, ".end", ""])


def circuit_manifest(circuit: SimulationCircuit) -> dict:
    return {
        "board": circuit.board_name,
        "nodes": dict(circuit.nodes),
        "aliases": dict(sorted(circuit.aliases.items())),
        "elements": [{"ref": e.ref, "kind": e.kind, "nodes": list(e.nodes), "model": e.model.model_id if e.model else None,
                      "value_si": str(e.value.base_value) if e.value else None} for e in circuit.elements],
        "sources": source_names(circuit),
        "boundaries": [{"endpoint": e, "net": n, "replacement": r} for e, n, r in circuit.boundaries],
        "models": [json.loads(m.manifest) for m in circuit.models],
        "numerics": {"reltol": 1e-5, "abstol_A": 1e-10, "vntol_V": 1e-7},
        "assumptions": list(circuit.plan.assumptions),
    }


@dataclass(frozen=True, slots=True)
class NgspiceBackend:
    plan: SimulationPlan

    def generate(self, board: Board) -> ArtifactManifest:
        artifacts = [Artifact("resolved-plan.json", "application/json", self.plan.document + "\n")]
        assets = {}
        for case in self.plan.cases:
            circuit = select_circuit(board, self.plan.for_case(case))
            prefix = f"cases/{case}"
            artifacts.append(Artifact(f"{prefix}/circuit.json", "application/json", json.dumps(circuit_manifest(circuit), indent=2, sort_keys=True) + "\n"))
            for analysis in circuit.plan.analyses:
                artifacts.append(Artifact(f"{prefix}/{analysis.name}/deck.cir", "text/plain", generate_deck(circuit, analysis)))
                for model in circuit.models:
                    for name, content in model.assets:
                        assets[f"{prefix}/{analysis.name}/{name}"] = content
        artifacts.extend(Artifact(name, "text/plain", content) for name, content in sorted(assets.items()))
        return ArtifactManifest("ngspice", "native", tuple(artifacts))
