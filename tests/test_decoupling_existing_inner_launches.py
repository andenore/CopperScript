"""Reuse exact existing feeds at both ends without allocating another via."""
from dataclasses import replace
import pytest
from pcbir import decoupling_paths as paths
from pcbir.physical import CopperLayer
from test_decoupling_inner_bridges import blocked_surfaces

@pytest.mark.parametrize('renamed', [False, True])
def test_both_existing_launches_join_when_new_sites_are_unavailable(monkeypatch, renamed):
    board, link = blocked_surfaces(renamed)
    initial = paths.propose_crosslayer_decoupling(board, link)
    assert initial.witness is not None, initial.reason
    # Retain the independently proven surface feeds and all four original vias.
    partial = replace(initial.board, tracks=tuple(t for t in initial.board.tracks
        if t.layer in (CopperLayer.FRONT, CopperLayer.BACK)))
    assert paths.verify_crosslayer_decoupling(partial, link).witness is None
    monkeypatch.setattr(paths, '_package_candidates', lambda *a, **k: iter(()))
    monkeypatch.setattr(paths, '_two_leg_candidates', lambda *a, **k: iter(()))
    repaired = paths.propose_crosslayer_decoupling(partial, link)
    assert repaired.witness is not None, repaired.reason
    assert paths.validate_crosslayer_witness(repaired.board, repaired.witness) == ''
    assert repaired.created_vias == ()
    assert repaired.created_tracks and all(t.layer not in (CopperLayer.FRONT, CopperLayer.BACK)
                                          for t in repaired.created_tracks)
    assert all(any(t is old for t in repaired.board.tracks) for old in partial.tracks)
    assert all(any(v is old for v in repaired.board.vias) for old in partial.vias)

def test_unconnected_same_net_vias_cannot_prove_a_destination_launch(monkeypatch):
    board, link = blocked_surfaces()
    initial = paths.propose_crosslayer_decoupling(board, link)
    assert initial.witness is not None
    partial = replace(initial.board, tracks=tuple(t for t in initial.board.tracks
        if t.layer is CopperLayer.FRONT))
    monkeypatch.setattr(paths, '_package_candidates', lambda *a, **k: iter(()))
    monkeypatch.setattr(paths, '_two_leg_candidates', lambda *a, **k: iter(()))
    repaired = paths.propose_crosslayer_decoupling(partial, link)
    assert repaired.witness is None and repaired.board is partial
    assert not repaired.created_tracks and not repaired.created_vias
