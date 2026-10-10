import pytest

from pcbir.pair_geometry import differential_pair_inventory


def track(net, start, end, layer="F.Cu", width=200):
    return dict(net=net, start=start, end=end, layer=layer, width_nm=width)


def board(*tracks):
    return dict(layers=["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"], tracks=list(tracks), vias=[], fills=[])


def screen(native, **kwargs):
    return differential_pair_inventory(native, {}, identifier="pair", positive_net="P", negative_net="N",
                                        maximum_search_gap_nm=kwargs.get("gap", 400))


def test_horizontal_reversed_partial_overlap_uses_edge_gap():
    result = screen(board(track("P", [0, 0], [1000, 0]), track("N", [1500, 400], [500, 400], width=100)))
    assert result["status"] == "incomplete"
    section = result["metrics"]["parallel_sections"][0]
    assert section["edge_gap_nm"] == 250
    assert section["parallel_overlap_length_nm"] == 500
    for member in result["metrics"]["members"].values():
        assert member["layers"]["F.Cu"]["length_without_parallel_candidate_nm"] == 500


@pytest.mark.parametrize("end,offset,distance", [([0, 1000], [400, 0], 400),
    ([1000, 1000], [0, 400], 400 / 2**.5), ([1000, -1000], [0, 400], 400 / 2**.5)])
def test_vertical_and_diagonal_segments(end, offset, distance):
    other_end = [end[i] + offset[i] for i in (0, 1)]
    r = screen(board(track("P", [0, 0], end), track("N", offset, other_end)))
    assert r["metrics"]["parallel_sections"][0]["centre_distance_nm"] == pytest.approx(distance)


@pytest.mark.parametrize("other", [track("N", [0, 400], [1000, 401]),
    track("N", [2000, 400], [3000, 400]), track("N", [0, 400], [1000, 400], layer="In2.Cu"),
    track("N", [0, 1000], [1000, 1000])])
def test_cross_layer_distant_nonparallel_and_disjoint_not_coupled(other):
    assert screen(board(track("P", [0, 0], [1000, 0]), other))["metrics"]["parallel_sections"] == []


def test_interval_union_avoids_multiple_candidate_double_counting():
    r = screen(board(track("P", [0, 0], [1000, 0]), track("N", [0, 400], [750, 400]),
                     track("N", [500, -400], [1000, -400])))
    assert len(r["metrics"]["parallel_sections"]) == 2
    assert r["metrics"]["members"]["P"]["layers"]["F.Cu"]["parallel_candidate_length_nm"] == 1000


def test_overlapping_conductors_fail_screen():
    assert screen(board(track("P", [0, 0], [1000, 0]), track("N", [0, 100], [1000, 100])))["status"] == "fail"


@pytest.mark.parametrize("width", [0, -1, True, 1.5])
def test_invalid_width_rejected(width):
    with pytest.raises(ValueError):
        screen(board(track("P", [0, 0], [1000, 0], width=width), track("N", [0, 400], [1000, 400])))


def test_missing_member_and_native_remain_incomplete():
    assert screen(None)["status"] == "incomplete"
    assert screen(board(track("P", [0, 0], [1000, 0])))["status"] == "incomplete"


def test_adjacent_power_plane_and_vias_are_reported_not_assumed_ground():
    native = board(track("P", [0, 0], [1000, 0], layer="In2.Cu"),
                   track("N", [0, 400], [1000, 400], layer="In2.Cu"))
    native["vias"] = [dict(net="P", position=[0, 0], from_layer="F.Cu", to_layer="B.Cu")]
    native["fills"] = [dict(net="V3V3", layer="B.Cu", outer=[[-500,-500],[1500,-500],[1500,900],[-500,900]], holes=[])]
    physical = []
    for i, layer in enumerate(native["layers"]):
        if i:
            physical.append(dict(kind="dielectric", thickness_nm=100 if i == 3 else 550))
        physical.append(dict(kind="copper", name=layer, thickness_nm=35))
    r = differential_pair_inventory(native, dict(physical_layers=physical, copper_layers=native["layers"]),
        identifier="pair", positive_net="P", negative_net="N", maximum_search_gap_nm=400)
    assert r["metrics"]["members"]["P"]["via_count"] == 1
    planes = r["metrics"]["adjacent_copper"]
    assert [p["surface_separation_nm"] for p in planes] == [550, 100]
    assert planes[1]["filled_nets"][0]["net"] == "V3V3"
    assert planes[1]["filled_nets"][0]["member_coverage"]["P"]["status"] == "pass"
    assert r["status"] == "incomplete"
