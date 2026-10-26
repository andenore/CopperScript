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

