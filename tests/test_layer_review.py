import importlib.util
from pathlib import Path

import pytest


spec = importlib.util.spec_from_file_location("review", Path(__file__).resolve().parents[1]/"scripts/review_routing_layers.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


@pytest.mark.parametrize("a,b,expected", [((-1, 0), (1, 0), 0),
    ((-1, 0), (1, 1), 45), ((-1, 0), (0, 1), 90),
    ((-1, 0), (-1, 1), 135), ((-1, 0), (-1, 0), 180)])
def test_turn_angle_uses_outward_vectors_and_distinguishes_sharp_bends(a, b, expected):
    assert module.turn_degrees(a, b) == expected


def test_zero_length_segment_is_not_a_bend():
    assert module.turn_degrees((0, 0), (1, 1)) is None
