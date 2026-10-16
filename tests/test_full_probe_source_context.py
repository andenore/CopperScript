"""Full pattern trials preserve source context and reject borrowed probe copper."""
from dataclasses import replace
import pytest
import pcbir.package_access as access
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling_paths import validate_crosslayer_witness
from pcbir.fanout import route_fanout
from pcbir.physical import Point, TrackSegment, CopperLayer
from test_crosslayer_partial_ownership import completed


@pytest.mark.parametrize("metadata", (
    {"critical_routing": "complete"},
    {"critical_routing": "partial", "plane_stitching": "pad-escapes-only"},
    {"plane_stitching": "partial"},
))
def test_full_trial_normalizes_metadata_before_creating_real_witnesses(metadata):
    source, link, settings, original = completed()
    track = TrackSegment("FOREIGN", Point.mm(21, 3), Point.mm(22, 3), 100000, CopperLayer.FRONT)
    probe = replace(source, tracks=(*source.tracks, track), metadata=metadata)
    assert validate_crosslayer_witness(probe, original.decoupling_witnesses[0]) == "stale_witness_context"
    proposed, reason = access._full_ordinary_pattern(source, probe, settings)
    assert reason == "proposed" and proposed.decoupling_witnesses
    assert proposed.board.metadata == source.metadata and track not in proposed.board.tracks
    assert verified_bypass_reservations(source, proposed)[2] is None
    assert all(validate_crosslayer_witness(proposed.board, w) == "" for w in proposed.decoupling_witnesses)
    assert probe.metadata == metadata and probe.tracks[-1] is track


@pytest.mark.parametrize("fault", ("rule", "pose", "name"))
def test_full_trial_rejects_changed_source_before_routing(monkeypatch, fault):
    source, _, settings, _ = completed()
    if fault == "rule": probe = replace(source, rules=replace(source.rules, minimum_clearance_nm=110000))
    elif fault == "pose": probe = replace(source, placements=(replace(source.placements[0], position=Point.mm(7, 8)), *source.placements[1:]))
    else: probe = replace(source, name="changed source")
    def forbidden(*args, **kwargs): raise AssertionError("source mismatch must not route")
    monkeypatch.setattr(access, "route_fanout", forbidden)
    assert access._full_ordinary_pattern(source, probe, settings) == (None, "stale_probe_source_intent")


def test_full_trial_rejects_real_witness_borrowing_disposable_return_via(monkeypatch):
    source, link, settings, original = completed()
    borrowed = next(v for v in original.decoupling_vias if v.net == link.return_net)
    probe = replace(source, vias=(*source.vias, borrowed), metadata={"critical_routing": "complete"})
    observed = []
    real = access.route_fanout
    def inspect(frame, options):
        result = real(frame, options)
        assert result.decoupling_witnesses
        assert any(step.edge is borrowed for w in result.decoupling_witnesses for step in w.return_path.steps)
        observed.append(result)
        return result
    monkeypatch.setattr(access, "route_fanout", inspect)
    proposed, reason = access._full_ordinary_pattern(source, probe, settings)
    assert observed and proposed is None and reason == "stale_bypass_witness"
    assert probe.vias[-1] is borrowed and source.vias == ()
    assert verified_bypass_reservations(source, original)[2] is None
