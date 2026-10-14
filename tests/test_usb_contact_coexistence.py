"""Local contact reports retain failed work without certifying partial copper."""
from dataclasses import FrozenInstanceError, replace
import pytest

from pcbir.pair_terminals import prepare_pair_terminals, PairTerminalPreparationStats
from pcbir.physical import Point, TrackSegment, CopperLayer
from pcbir.routing_clearance import RoutingClearanceIndex
from test_local_pair_branches import fixture


def test_local_contact_telemetry_is_observational_and_source_is_unchanged():
    board = fixture()
    expected = prepare_pair_terminals(board, *board.net_routing_rules, {})
    report = PairTerminalPreparationStats()
    actual = prepare_pair_terminals(board, *board.net_routing_rules, {}, report=report)
    assert actual == expected
    assert report.status == "matched_contacts"
    assert report.paired_topology_supported
    assert report.elapsed_seconds > 0
    assert report.searches > 0 and report.expanded_states > 0
    assert report.candidates > 0
    assert report.track_count == len(actual[1]) + len(actual[2])
    assert report.via_count == 2 * len(actual[3]) + len(actual[4])
    assert board.tracks == board.vias == ()


def test_supported_but_unavailable_local_contacts_report_bounded_failure():
    board = fixture()
    board = replace(board, net_routing_rules=tuple(replace(rule, max_vias=0)
                    for rule in board.net_routing_rules))
    report = PairTerminalPreparationStats()
    routes = {"sentinel": object()}
    actual = prepare_pair_terminals(board, *board.net_routing_rules, routes, report=report)
    assert actual == (routes, (), (), (), ())
    assert report.status == "blocked" and report.paired_topology_supported
    assert "no accepted candidate" in report.reason
    assert report.elapsed_seconds > 0
    assert report.track_count == report.via_count == 0
    assert report.expanded_states <= 16000
    assert board.tracks == board.vias == ()


def test_blocker_report_contains_exact_owner_identity_and_remains_bounded():
    board = fixture()
    owner = TrackSegment("P", Point.mm(2, 2), Point.mm(3, 2), 100000, CopperLayer.FRONT)
    # Reporting invokes the same exact owner query as routing, retaining its
    # geometry identity rather than assigning an inferred pad/designator owner.
    board = replace(board, tracks=(owner,))
    query = replace(owner, net="N")
    report = PairTerminalPreparationStats()
    index = RoutingClearanceIndex(board)
    report.record_blockers(index, (query,), ())
    assert report.blocking_objects == index.track_blocker_records(query)
    assert report.blocking_objects[0].net == "P"
    report.record_blockers(index, (query,) * 40, ())
    assert len(report.blocking_objects) == 1
    assert board.tracks == (owner,)



def test_local_preparation_snapshot_is_immutable_and_elapsed_is_observational():
    report = PairTerminalPreparationStats(status="blocked", reason="no accepted candidate",
                                          elapsed_seconds=1.0, expanded_states=17)
    snapshot = report.snapshot()
    report.expanded_states = 18
    assert snapshot.expanded_states == 17
    assert replace(snapshot, elapsed_seconds=99.0) == snapshot
    with pytest.raises(FrozenInstanceError):
        snapshot.status = "matched_contacts"


# Consumer-owned resolved fixtures keep device assets and source facts out of
# this generic runner. It calls the shared routing APIs with fixture-supplied
# owner identities; names never select a routing or repair policy.
def encode_resolved_fixture(value):
    from collections.abc import Mapping
    from dataclasses import fields, is_dataclass
    from decimal import Decimal
    from enum import Enum
    from fractions import Fraction
    if isinstance(value, Enum):
        return {"enum": type(value).__module__ + "." + type(value).__name__, "value": value.value}
    if isinstance(value, Decimal):
        return {"decimal": str(value)}
    if isinstance(value, Fraction):
        return {"fraction": [value.numerator, value.denominator]}
    if is_dataclass(value):
        return {"type": type(value).__module__ + "." + type(value).__name__,
                "fields": {f.name: encode_resolved_fixture(getattr(value, f.name))
                           for f in fields(value) if f.init}}
    if isinstance(value, Mapping):
        return {"mapping": [[encode_resolved_fixture(k), encode_resolved_fixture(v)]
                            for k, v in value.items()]}
    if isinstance(value, tuple):
        return {"tuple": [encode_resolved_fixture(v) for v in value]}
    if isinstance(value, frozenset):
        return {"frozenset": [encode_resolved_fixture(v) for v in sorted(value, key=repr)]}
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    raise TypeError("unsupported resolved fixture value: " + type(value).__name__)


def decode_resolved_fixture(value):
    from dataclasses import is_dataclass
    from decimal import Decimal
    from enum import Enum
    from fractions import Fraction
    from importlib import import_module
    if not isinstance(value, dict):
        return value
    if "decimal" in value:
        return Decimal(value["decimal"])
    if "fraction" in value:
        return Fraction(*value["fraction"])
    if "tuple" in value:
        return tuple(decode_resolved_fixture(v) for v in value["tuple"])
    if "frozenset" in value:
        return frozenset(decode_resolved_fixture(v) for v in value["frozenset"])
    if "mapping" in value:
        return {decode_resolved_fixture(k): decode_resolved_fixture(v) for k, v in value["mapping"]}
    tag = value.get("type", value.get("enum", ""))
    module, _, name = tag.rpartition(".")
    if module not in {"pcbir.physical", "pcbir.mechanical_references", "pcbir.silkscreen"}:
        raise ValueError("unsupported resolved geometry type: " + tag)
    cls = getattr(import_module(module), name)
    if "enum" in value and isinstance(cls, type) and issubclass(cls, Enum):
        return cls(value["value"])
    if "type" in value and is_dataclass(cls):
        return cls(**{k: decode_resolved_fixture(v) for k, v in value["fields"].items()})
    raise ValueError("unsupported resolved geometry type: " + tag)


def test_resolved_fixture_codec_preserves_geometry_and_exact_fixed_point_values():
    from pcbir.drc import physical_board_digest
    board = fixture()
    restored = decode_resolved_fixture(encode_resolved_fixture(board))
    assert restored == board
    assert physical_board_digest(restored) == physical_board_digest(board)
    with pytest.raises(ValueError):
        decode_resolved_fixture({"type": "os.system", "fields": {}})


def run_resolved_fixture(path, output, kicad_cli):
    """Run a bounded scope; unrelated coarse/native failures remain explicit."""
    from collections import Counter
    from dataclasses import asdict
    from hashlib import sha256
    import json
    from pathlib import Path
    from time import perf_counter
    from pcbir import route_global, GlobalRouterOptions
    from pcbir.boundary_access import analyze_boundary_access, reserve_boundary_access, BoundaryAccessOptions
    from pcbir.critical import _route_pair_group
    from pcbir.detailed import route_detailed, DetailedRouterOptions
    from pcbir.drc import explicit_copper_connectivity, physical_board_digest, run_physical_drc, PhysicalDrcPolicy
    from pcbir.fanout import route_fanout, FanoutOptions
    from pcbir.plane_verify import verify_filled_planes, native_object_map
    path, output = Path(path), Path(output)
    output.mkdir(parents=True, exist_ok=False)
    data = json.loads(path.read_text())
    board = decode_resolved_fixture(data["board"])
    rules = {r.net: r for r in board.net_routing_rules}
    first, second = (rules[n] for n in data["critical_pair"])
    ordinary_names = frozenset(data["ordinary_nets"])
    intended = ordinary_names | {first.net, second.net}
    settings = FanoutOptions(**data["fanout_options"])
    started = perf_counter()
    summary = {"fixture_sha256": sha256(path.read_bytes()).hexdigest(), "provenance": data["provenance"],
               "source_digest": physical_board_digest(board), "fabrication_ready": False,
               "compiler_modules": {name: sha256(Path(__import__(name, fromlist=["x"]).__file__).read_bytes()).hexdigest()
                                    for name in ("pcbir.pair_terminals", "pcbir.critical", "pcbir.fanout", "pcbir.boundary_access", "pcbir.detailed")}}
    def save(stage):
        summary.update(stage=stage, elapsed_seconds=perf_counter()-started)
        (output/"result.json").write_text(json.dumps(summary, indent=2, default=str))
        print(stage, flush=True)
    initial = route_fanout(board, settings, only_nets=ordinary_names)
    initial_report = PairTerminalPreparationStats()
    prepare_pair_terminals(initial.board, first, second, {}, report=initial_report)
    summary["initial_local_preparation"] = asdict(initial_report.snapshot())
    report = PairTerminalPreparationStats()
    _, a, b, pairs, returns = prepare_pair_terminals(board, first, second, {}, report=report)
    summary["clean_local_preparation"] = asdict(report.snapshot())
    if not a and not b:
        save("clean_contact_failure")
        return False
    probe = replace(board, tracks=(*board.tracks, *a, *b),
                    vias=(*board.vias, *[v for pair in pairs for v in pair], *returns))
    fanout = route_fanout(probe, settings, only_nets=ordinary_names)
    summary["escaped_pads_preserved"] = initial.escaped_pads <= fanout.escaped_pads
    if fanout.pending_pads or not summary["escaped_pads_preserved"]:
        save("ordinary_identity_failure")
        return False
    rebased = replace(board, tracks=(*board.tracks, *fanout.created_tracks),
                      vias=(*board.vias, *fanout.created_vias))
    summary["reassigned_digest"] = physical_board_digest(rebased)
    guides = route_global(board, GlobalRouterOptions(**data["global_options"]))
    summary["global_status"] = guides.status.value
    metrics, additions, transitions = _route_pair_group(board, first, second,
        {r.net: r for r in guides.routes}, rules, list(rebased.tracks), list(rebased.vias),
        data["pair_state_limit"])
    summary["critical"] = asdict(metrics)
    if not metrics.connected:
        save("full_pair_failure")
        return False
    merged = replace(board, tracks=(*rebased.tracks, *additions), vias=(*rebased.vias, *transitions))
    boundary = analyze_boundary_access(merged, fanout, BoundaryAccessOptions(**data["boundary_options"]))
    summary["boundary"] = asdict(boundary)
    if not boundary.ready:
        save("boundary_failure")
        return False
    reserved = reserve_boundary_access(merged, fanout, boundary)
    result = route_detailed(reserved.board, guides, DetailedRouterOptions(**data["detailed_options"]),
        fanout_accesses=reserved.routing_accesses,
        fanout_created_vias=frozenset((v.net, v.position) for v in reserved.created_vias),
        fanout_created_tracks=reserved.created_tracks, only_nets=ordinary_names)
    final = result.board
    summary["detailed"] = json.loads(result.to_json())
    graph = explicit_copper_connectivity(final)
    summary["intended_connectivity"] = {n.name: graph.net_connected(n) for n in board.nets if n.name in intended}
    summary["preservation"] = {"source_tracks": Counter(board.tracks) <= Counter(final.tracks),
        "source_vias": Counter(board.vias) <= Counter(final.vias),
        "critical_tracks": Counter(additions) <= Counter(final.tracks),
        "critical_vias": Counter(transitions) <= Counter(final.vias),
        "reserved_tracks": Counter(reserved.board.tracks) <= Counter(final.tracks),
        "fixed_poses": board.placements == final.placements,
        "macros": board.hard_macros == final.hard_macros,
        "arrays": board.via_in_pad_rules == final.via_in_pad_rules}
    summary["released_generated_vias"] = [asdict(v) for v in
        (Counter(reserved.created_vias)-Counter(final.vias)).elements()]
    summary["physical_findings"] = [asdict(f) for f in run_physical_drc(final,
        policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings]
    save("geometry_complete")
    for label, candidate in (("source", board), ("final", final)):
        native = verify_filled_planes(candidate, kicad_cli=Path(kicad_cli), artifact_directory=output/("native-"+label))
        summary["native_"+label] = asdict(native)
    summary["selected_native_objects"] = {uuid: asdict(item) for uuid, item in native_object_map(final).items()
                                          if item.net in intended}
    selected_findings = [f for f in summary["native_final"]["native_findings"]
                         if any(item["net"] in intended for item in f["items"])]
    summary["selected_native_findings"] = selected_findings
    def native_identity(finding):
        return json.dumps(finding, sort_keys=True)
    original_hard = Counter(native_identity(f) for f in summary["native_source"]["native_findings"]
                            if f["category"] != "unconnected")
    current_hard = Counter(native_identity(f) for f in summary["native_final"]["native_findings"]
                           if f["category"] != "unconnected")
    summary["new_native_hard_findings"] = list((current_hard-original_hard).elements())
    summary["source_unchanged"] = physical_board_digest(board) == summary["source_digest"]
    summary["scope_complete"] = (all(summary["intended_connectivity"].values())
        and all(summary["preservation"].values()) and not selected_findings
        and not summary["new_native_hard_findings"] and summary["source_unchanged"]
        and summary["native_final"]["island_count"] <= summary["native_source"]["island_count"])
    save("native_complete")
    return summary["scope_complete"]


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description="Generic bounded resolved owner-coexistence fixture")
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--kicad-cli", required=True)
    args = parser.parse_args()
    raise SystemExit(0 if run_resolved_fixture(args.fixture, args.output, args.kicad_cli) else 1)
