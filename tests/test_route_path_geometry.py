import pytest

from pcbir.route_path_geometry import routed_path_inventory


def track(a, b, layer="F.Cu"):
    return dict(net="S", start=a, end=b, width_nm=100, layer=layer)


def contact(component, xy, layer="F.Cu"):
    return dict(component=component, pad="1", position=xy, layers=[layer], net="S")


def board(tracks=None):
    return dict(layers=["F.Cu", "In1.Cu", "B.Cu"],
                tracks=tracks or [track([0,0], [1000,0])], vias=[],
                contacts=[contact("A", [0,0]), contact("B", [1000,0])])


def screen(native, context=None, terminals=None):
    return routed_path_inventory(native, context or {}, identifier="path", net="S", return_net="GND",
        terminals=terminals or [dict(component="A", pad="1", layer="F.Cu"), dict(component="B", pad="1", layer="F.Cu")])


def test_unique_path_is_geometry_not_approval():
    r = screen(board())
    assert r["status"] == "incomplete"
    assert r["metrics"]["centreline_path_found"]
    assert r["metrics"]["path_track_length_nm"] == 1000
    assert r["metrics"]["off_path_connected_edge_count"] == 0


def test_reversed_diagonal_segments_work():
    b = board([track([500,500], [0,0]), track([1000,0], [500,500])])
    r = screen(b)["metrics"]
    assert r["path_track_length_nm"] == pytest.approx(1000 * 2**.5)
    assert r["path"][0]["start"] == [0,0,"F.Cu"]
    assert r["path"][0]["end"] == r["path"][1]["start"]
    assert r["path"][-1]["end"] == [1000,0,"F.Cu"]


def test_midtrack_pad_centres_split_track_without_counting_overhang():
    b = board([track([-200,0], [1200,0])])
    r = screen(b)["metrics"]
    assert r["path_track_length_nm"] == 1000
    assert r["off_path_connected_edge_count"] == 2


def test_t_branch_is_separate_from_unique_tree_path():
    b = board([track([0,0], [1000,0]), track([500,0], [500,100])])
    r = screen(b)["metrics"]
    assert r["path_track_length_nm"] == 1000
    assert r["branch_nodes"] == [[500,0,"F.Cu"]]
    assert r["off_path_connected_edge_count"] == 1


def test_disconnected_copper_is_not_added_to_path():
    b = board([track([0,0], [1000,0]), track([2000,0], [3000,0])])
    r = screen(b)["metrics"]
    assert r["path_track_length_nm"] == 1000
    assert r["off_component_edge_count"] == 1


def test_cycle_has_no_shortest_path_guess():
    b = board([track([0,0], [1000,0]), track([0,0], [500,500]), track([500,500], [1000,0])])
    r = screen(b)
    assert "cycle" in r["detail"]
    assert r["metrics"]["path_track_length_nm"] is None


def test_crossing_centrelines_require_explicit_handling():
    b = board([track([0,0], [1000,0]), track([500,-100], [500,100])])
    assert "crossing unsupported" in screen(b)["detail"]


@pytest.mark.parametrize("tracks", [[track([0,0], [1000,0]), track([1000,0], [0,0])],
                                   [track([0,0], [1000,0]), track([500,0], [1500,0])]])
def test_duplicate_and_overlapping_edges_rejected(tracks):
    with pytest.raises(ValueError, match="overlapping/duplicate"):
        screen(board(tracks))


@pytest.mark.parametrize("mutation", ["missing_layer", "wrong_layer", "wrong_net", "duplicate_land"])
def test_ambiguous_or_unverified_terminal_is_incomplete(mutation):
    b = board()
    if mutation == "missing_layer":
        b["contacts"][0].pop("layers")
    elif mutation == "wrong_layer":
        b["contacts"][0]["layers"] = ["B.Cu"]
    elif mutation == "wrong_net":
        b["contacts"][0]["net"] = "OTHER"
    else:
        b["contacts"].append(b["contacts"][0])
    assert screen(b)["status"] == "incomplete"
    assert "identity unestablished" in screen(b)["detail"]


def test_different_layers_do_not_join_without_supported_via():
    b = board([track([0,0], [500,0]), track([500,0], [1000,0], "In1.Cu"), track([1000,0], [1000,100])])
    assert not screen(b)["metrics"]["centreline_path_found"]


def test_via_span_stub_and_return_distance_are_geometry_only():
    b = board([track([0,0], [500,0]), track([500,0], [1000,0], "In1.Cu")])
    b["contacts"][1]["layers"] = ["In1.Cu"]
    b["vias"] = [dict(net="S", position=[500,0], from_layer="F.Cu", to_layer="B.Cu", via_type="through"),
                 dict(net="GND", position=[500,300], from_layer="F.Cu", to_layer="B.Cu", via_type="through")]
    context = dict(copper_layers=b["layers"], physical_layers=[
        dict(kind="copper", name="F.Cu", thickness_nm=20), dict(kind="dielectric", thickness_nm=90),
        dict(kind="copper", name="In1.Cu", thickness_nm=20), dict(kind="dielectric", thickness_nm=180),
        dict(kind="copper", name="B.Cu", thickness_nm=20)])
    r = screen(b, context, [dict(component="A", pad="1", layer="F.Cu"), dict(component="B", pad="1", layer="In1.Cu")])
    assert r["status"] == "incomplete"
    m = r["metrics"]
    assert m["path_track_length_nm"] == 1000
    assert m["path_via_transition_count"] == 1
    assert m["vias"][0]["construction_span_between_used_layer_centres_nm"] == 110
    assert m["vias"][0]["construction_unused_barrel_below_nm"] == 200
    assert m["vias"][0]["nearest_return_via"]["centre_distance_nm"] == 300


@pytest.mark.parametrize("via_type", ["non-through", None])
def test_unverified_via_type_is_not_assumed_through(via_type):
    b = board()
    b["vias"] = [dict(net="S", position=[500,0], from_layer="F.Cu", to_layer="B.Cu", via_type=via_type)]
    assert "Non-through" in screen(b)["detail"]


def test_no_native_probe_is_incomplete():
    assert screen(None)["status"] == "incomplete"
