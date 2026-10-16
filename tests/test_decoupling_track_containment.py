"""Narrow exact containment avoids redundant producer edges, never waives proof."""
from dataclasses import replace
from itertools import islice

import pytest

from pcbir.decoupling_paths import _connections, propose_crosslayer_decoupling, verify_crosslayer_decoupling
from pcbir.fanout import FanoutOptions
from pcbir.physical import CopperLayer, Point, TrackSegment
from pcbir.track_containment import track_is_contained
from test_decoupling_crosslayer_paths import fixture


def segment(a, b, *, net='P', width=150_000, layer=CopperLayer.BACK):
    return TrackSegment(net, Point(*a), Point(*b), width, layer)


@pytest.mark.parametrize('held_a,held_b,first,last', (
    ((0, 0), (10, 0), (2, 0), (8, 0)),
    ((10, 0), (0, 0), (8, 0), (2, 0)),
    ((0, 10), (0, 0), (0, 2), (0, 8)),
    ((0, 0), (10, 10), (8, 8), (2, 2)),
    ((-10, 10), (10, -10), (-4, 4), (8, -8)),
    ((0, 0), (10, 0), (0, 0), (10, 0)),
))
def test_true_subsegment_and_reversed_edges_use_exact_existing_geometry(held_a, held_b, first, last):
    held = segment(held_a, held_b)
    candidate = segment(first, last)
    unrelated = segment((20, 20), (30, 30))
    assert track_is_contained(candidate, (held, unrelated))
    assert track_is_contained(candidate, (unrelated, held))
    assert held.start == Point(*held_a) and held.end == Point(*held_b)


@pytest.mark.parametrize('candidate', (
    segment((-1, 0), (8, 0)),  # Partial overlap is still new copper.
    segment((-1, 0), (11, 0)),
    segment((2, 1), (8, 1)),  # Parallel within physical widths is not this proof.
    segment((2, 0), (8, 1)),
    segment((2, 0), (8, 0), net='OTHER'),
    segment((2, 0), (8, 0), layer=CopperLayer.FRONT),
    segment((2, 0), (8, 0), width=100_000),
    segment((2, 0), (8, 0), width=200_000),
))
def test_partial_overlap_other_net_layer_or_either_width_difference_stays_new(candidate):
    assert not track_is_contained(candidate, (segment((0, 0), (10, 0)),))


def test_multiple_segments_do_not_masquerade_as_one_containing_source_edge():
    assert not track_is_contained(segment((1, 0), (9, 0)),
        (segment((0, 0), (5, 0)), segment((5, 0), (10, 0))))


def test_large_fixed_point_coordinates_keep_one_nanometre_noncollinearity():
    n = 2**60
    held = segment((n, n), (n + 100, n + 100))
    assert track_is_contained(segment((n + 1, n + 1), (n + 99, n + 99)), (held,))
    assert not track_is_contained(segment((n + 1, n + 2), (n + 99, n + 99)), (held,))


@pytest.mark.parametrize('renamed', (False, True))
def test_real_connections_preserve_containing_source_edge_and_add_no_redundant_tail(renamed):
    source, link = fixture(renamed=renamed)
    held = TrackSegment(link.net, Point.mm(9.5, 8), Point.mm(9.5, 10),
                        150_000, CopperLayer.BACK)
    source = replace(source, tracks=(held,))
    candidates = tuple(islice(_connections(source, link.target, link.capacitor,
        link.net, FanoutOptions(), 8, 64), 16))
    assert candidates
    seen_supply = False
    for candidate in candidates:
        assert candidate.tracks[0] is held
        additions = candidate.tracks[len(source.tracks):]
        assert not any(track_is_contained(t, source.tracks) for t in additions)
        seen_supply |= verify_crosslayer_decoupling(candidate, link).supply_path is not None
    assert seen_supply
    proposed = propose_crosslayer_decoupling(source, link)
    assert proposed.witness is not None, proposed.reason
    assert proposed.board.tracks[0] is held
    assert all(any(t is original for original in proposed.board.tracks)
               for path in (proposed.witness.supply_path, proposed.witness.return_path)
               for t in path.tracks)
    assert source.tracks == (held,) and not source.vias
