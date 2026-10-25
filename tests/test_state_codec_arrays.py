"""Numeric numpy arrays round-trip exactly through the shared state codec."""
from __future__ import annotations

import numpy as np
import pytest

from pcbir.state_codec import StateError, decode, encode, load_state, save_state


@pytest.mark.parametrize("array", [
    np.arange(12, dtype=np.float64).reshape(3, 4) / 7,
    np.array([[1, -2], [3, 4]], dtype=np.int32),
    np.array([True, False, True]),
    np.zeros((0, 5), dtype=np.uint8),
    np.arange(6, dtype=">i8").reshape(2, 3),       # big-endian input is stored little-endian
    np.asfortranarray(np.arange(6.0).reshape(2, 3)),
])
def test_numeric_arrays_round_trip_exactly(array):
    back = decode(encode(array))
    assert back.shape == array.shape and back.dtype.kind == array.dtype.kind
    assert back.dtype.itemsize == array.dtype.itemsize
    assert np.array_equal(back, array)
    back[...] = back  # decoded arrays are writable copies, not views of the file


def test_numpy_scalars_encode_as_plain_numbers():
    assert encode(np.int64(7)) == 7 and encode(np.float64(0.5)) == 0.5
    assert encode(np.bool_(True)) is True


def test_object_and_string_arrays_are_refused():
    for array in (np.array([object()]), np.array(["a", "b"])):
        with pytest.raises(StateError, match="cannot encode"):
            encode(array)


def test_corrupted_array_payload_is_refused():
    spec = encode(np.arange(4, dtype=np.int32))
    spec["$ndarray"]["shape"] = [5]
    with pytest.raises(StateError, match="does not match"):
        decode(spec)
    spec = encode(np.arange(4, dtype=np.int32))
    spec["$ndarray"]["dtype"] = "|O"
    with pytest.raises(StateError, match="unsupported array dtype"):
        decode(spec)


def test_unified_result_with_congestion_map_saves_and_loads(tmp_path):
    from test_unified_congestion_export import _crossing_route
    result = _crossing_route()[0]
    assert result.congestion is not None
    path = save_state(tmp_path / "result.state.json", result)
    back = load_state(path)
    assert back == result
    assert back.congestion == result.congestion
    assert np.array_equal(back.congestion.demand, result.congestion.demand)
    # Same value, same bytes: the content digest stays deterministic.
    again = save_state(tmp_path / "again.state.json", back)
    assert again.read_bytes() == path.read_bytes()


def test_unknown_dtype_string_is_a_state_error():
    spec = encode(np.arange(2, dtype=np.int32))
    spec["$ndarray"]["dtype"] = "not-a-dtype"
    with pytest.raises(StateError, match="unsupported array dtype"):
        decode(spec)
