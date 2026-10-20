"""Shared KiCad reader conformance fixture (identical copy in CopperReview)."""

import hashlib
import json
import math
from pathlib import Path

import pytest

from pcbir.scene import scene_from_kicad_pcb

FIXTURE = Path(__file__).parent / "fixtures" / "kicad-conformance"
EXPECTED = json.loads((FIXTURE / "expected.json").read_text(encoding="utf-8"))
TOL = EXPECTED["tolerance_mm"]


@pytest.fixture(scope="module")
def scene():
    return scene_from_kicad_pcb(FIXTURE / EXPECTED["board"])


def _close(a, b):
    assert len(a) == len(b) and all(abs(x - y) <= TOL for x, y in zip(a, b)), (a, b)


def test_fixture_is_the_shared_copy():
    assert hashlib.sha256((FIXTURE / EXPECTED["board"]).read_bytes()).hexdigest() == EXPECTED["board_sha256"]


def test_footprints_in_file_order_have_unique_names_and_poses(scene):
    footprints = list(scene.footprints.values())
    assert len(footprints) == len(EXPECTED["footprints"])
    assert [f.ref for f in footprints] == ["R1", "U2", "U3", "FP#1", "FP#2", "TP1", "TP1#2"]
    for got, want in zip(footprints, EXPECTED["footprints"]):
        _close(got.center, want["position_mm"])
        assert got.rotation == want["rotation_deg"] and got.side == want["side"]


def test_pad_centres_and_outlines(scene):
    refs = list(scene.footprints)
    for ref, want in zip(refs, EXPECTED["footprints"]):
        pads = [p for p in scene.pads if p.ref == ref]
        assert [p.pad for p in pads] == [p["number"] for p in want["pads"]]
        for pad, expected in zip(pads, want["pads"]):
            _close(pad.center, expected["center_mm"])
            assert list(pad.layers) == expected["layers"]
            if expected["shape"] == "rect":
                got = sorted(pad.polygon)
                assert len(got) == 4
                for g, w in zip(got, sorted(map(tuple, expected["corners_mm"]))):
                    _close(g, w)


def test_copper_zones_including_footprint_zones_use_board_coordinates(scene):
    # Keepouts are rule areas, not copper, so this reader leaves them out, and it shows
    # a zone's outline only on layers where the zone has no stored fill.
    fills = {(z["footprint_index"], z["layer"]) for z in EXPECTED["zones"] if z["kind"] == "fill"}
    copper = [z for z in EXPECTED["zones"] if z["kind"] == "fill"
              or z["kind"] == "outline" and (z["footprint_index"], z["layer"]) not in fills]
    assert any(z["footprint_index"] is not None for z in copper)
    kinds = {"fill": "fill", "outline": "zone"}
    assert len(scene.areas) == len(copper)
    for want in copper:
        match = [a for a in scene.areas if a.kind == kinds[want["kind"]] and a.layer == want["layer"]
                 and a.net == want["net"] and abs(a.outer[0][0] - want["points_mm"][0][0]) <= TOL]
        assert len(match) == 1, want
        for g, w in zip(match[0].outer, want["points_mm"]):
            _close(g, w)


def test_arcs_and_vias(scene):
    for want in EXPECTED["arcs"]:
        segments = [t for t in scene.tracks if t.net == want["net"] and t.start != (20.0, 10.8)]
        points = [segments[0].start] + [t.end for t in segments]
        _close(points[0], want["start_mm"])
        _close(points[-1], want["end_mm"])
        assert all(abs(math.dist(p, want["center_mm"]) - want["radius_mm"]) <= 1e-6 for p in points)
    for want in EXPECTED["vias"]:
        via = next(v for v in scene.vias if v.net == want["net"])
        _close(via.center, want["position_mm"])
        assert list(via.layers) == want["layers"]
