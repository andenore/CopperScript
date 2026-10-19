from pathlib import Path

import pytest

from pcbir.cam_geometry_adapter import (
    _arc_poly_shape,
    extract_positive_copper_artwork,
    subtract_drill_geometry,
)
from pcbir.cam_qualification import DrillHit, DrillSlot, NormalizedDrillProgram


def _gerber(*body: str) -> str:
    return "".join((
        "G04 CAM vector fixture*\n%FSLAX26Y26*%\n%MOMM*%\n"
        "%TF.FileFunction,Copper,L1,Top*%\n%TF.FilePolarity,Positive*%\n"
        "%ADD10C,0.1*%\nD10*\n",
        *body,
        "M02*\n",
    ))


def test_gerber_region_cut_in_becomes_a_valid_polygon_hole(tmp_path: Path) -> None:
    path = tmp_path / "region-with-hole.gbr"
    path.write_text(_gerber(
        "G36*\nX0Y5000000D02*\nG01*\n"
        "Y10000000D01*\nX10000000D01*\nY0D01*\nX0D01*\nY5000000D01*\n"
        "X1000000D01*\nX5000000Y1000000D01*\nX9000000Y5000000D01*\n"
        "X5000000Y9000000D01*\nX1000000Y5000000D01*\nX0D01*\nG37*\n"
    ), encoding="ascii")

    function, shape = extract_positive_copper_artwork(path)

    assert function == "Copper,L1,Top"
    assert shape.is_valid
    assert shape.geom_type == "Polygon"
    assert len(shape.interiors) == 1
    assert shape.area == pytest.approx(68.0)


def test_cut_in_rejects_diagonal_coincident_edges() -> None:
    from gerbonara import graphic_primitives as gp

    contour = gp.ArcPoly(
        outline=[(0, 0), (10, 0), (10, 10), (0, 10), (0, 0),
                 (1, 1), (5, 3), (9, 1), (1, 1), (0, 0)],
        arc_centers=[None] * 9,
    )

    with pytest.raises(ValueError, match="horizontal or vertical"):
        _arc_poly_shape(contour, max_curve_error_nm=100, allow_cutins=True)


def test_curved_region_contour_keeps_the_true_circle_bounds_and_area() -> None:
    from gerbonara import graphic_primitives as gp

    contour = gp.ArcPoly(
        outline=[(1, 0), (0, 1), (-1, 0), (0, -1)],
        arc_centers=[(False, (0, 0))] * 4,
    )

    shape = _arc_poly_shape(contour, max_curve_error_nm=100, allow_cutins=True)

    assert shape.is_valid
    assert shape.bounds == pytest.approx((-1, -1, 1, 1), abs=0.0001)
    assert shape.area == pytest.approx(3.141592653589793, abs=0.001)


def test_gerber_vector_extractor_requires_final_m02(tmp_path: Path) -> None:
    path = tmp_path / "truncated.gbr"
    path.write_text(_gerber("D10*\nX0Y0D03*\n").removesuffix("M02*\n"), encoding="ascii")

    with pytest.raises(ValueError, match="mandatory final M02"):
        extract_positive_copper_artwork(path)


def test_drill_subtraction_removes_plated_and_nonplated_holes() -> None:
    from shapely.geometry import Point, box

    positive = box(0, 0, 10, 10)
    programs = (
        NormalizedDrillProgram(
            (DrillHit("T1", 2_000_000, 3_000_000, 5_000_000),),
            "mm", True,
        ),
        NormalizedDrillProgram(
            (DrillHit("T2", 2_000_000, 7_000_000, 5_000_000),),
            "mm", False,
        ),
    )

    final = subtract_drill_geometry(positive, programs)

    assert final.is_valid
    assert not final.covers(Point(3, 5))
    assert not final.covers(Point(7, 5))
    assert final.area == pytest.approx(100 - 2 * 3.141592653589793, abs=0.0005)


def test_drill_subtraction_removes_round_ended_slots() -> None:
    from shapely.geometry import Point, box

    program = NormalizedDrillProgram(
        (), "mm", False,
        (DrillSlot("T1", 2_000_000, (2_000_000, 5_000_000),
                   (8_000_000, 5_000_000)),),
    )

    final = subtract_drill_geometry(box(0, 0, 10, 10), (program,))

    assert final.is_valid
    assert not final.covers(Point(5, 5))
    assert not final.covers(Point(2, 5))
    assert final.area == pytest.approx(100 - (12 + 3.141592653589793), abs=0.0002)


def test_drill_subtraction_requires_explicit_plating_classification() -> None:
    from shapely.geometry import box

    unclassified = NormalizedDrillProgram(
        (DrillHit("T1", 400_000, 1_000_000, 1_000_000),), "mm", None
    )

    with pytest.raises(ValueError, match="explicit plated/non-plated"):
        subtract_drill_geometry(box(0, 0, 2, 2), (unclassified,))
