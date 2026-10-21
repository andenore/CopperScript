"""Fail-closed Gerber vector extraction for CAM geometry checks.

Gerber regions may contain valid cut-in contours to encode holes. Such contours
are weakly-simple and appear self-touching to ordinary polygon libraries. This
adapter decomposes only spec-valid, fully coincident linear cut-in pairs; it
rejects partial overlaps, non-axis-aligned cut-ins, and other invalid geometry.
No geometry repair is performed.
"""
from __future__ import annotations

from collections import Counter, defaultdict
import math
from pathlib import Path
import re


def _nm_point(point: tuple[float, float]) -> tuple[int, int]:
    return round(point[0] * 1_000_000), round(point[1] * 1_000_000)


def _arc_poly_shape(arc_poly, *, max_curve_error_nm: int, allow_cutins: bool):
    from gerbonara import graphic_primitives as gp
    from shapely.geometry import Polygon

    outline = list(arc_poly.outline)
    if len(outline) < 3:
        raise ValueError("Gerber contour has fewer than three vertices")
    vertices = [_nm_point(point) for point in outline]
    closed = vertices[0] == vertices[-1]
    if closed:
        vertices.pop()
    if len(set(vertices)) < 3:
        raise ValueError("Gerber contour has fewer than three distinct vertices")

    centers = list(arc_poly.arc_centers)
    if len(centers) < len(vertices):
        centers.extend([None] * (len(vertices) - len(centers)))
    if len(centers) != len(vertices):
        raise ValueError("unsupported Gerber arc-center inventory")

    edges = []
    for index, start in enumerate(vertices):
        end = vertices[(index + 1) % len(vertices)]
        center = centers[index]
        arc = None if center is None else (
            bool(center[0]), _nm_point(center[1])
        )
        if start == end:
            raise ValueError("zero-length Gerber contour segment")
        edges.append((start, end, arc))

    straight_groups: dict[tuple, list[int]] = defaultdict(list)
    for index, (start, end, arc) in enumerate(edges):
        if arc is None:
            straight_groups[tuple(sorted((start, end)))].append(index)
    cutins = [indices for indices in straight_groups.values() if len(indices) > 1]
    if cutins and not allow_cutins:
        raise ValueError("self-touching non-region aperture contour")

    removed: set[int] = set()
    cutin_axes: set[str] = set()
    for indices in cutins:
        if len(indices) != 2:
            raise ValueError("Gerber cut-in edge is not an exact pair")
        first, second = (edges[index] for index in indices)
        if first[0] != second[1] or first[1] != second[0]:
            raise ValueError("coincident Gerber cut-in segments must be traversed in opposite directions")
        (x1, y1), (x2, y2) = first[0], first[1]
        if y1 == y2 and x1 != x2:
            cutin_axes.add("horizontal")
        elif x1 == x2 and y1 != y2:
            cutin_axes.add("vertical")
        else:
            raise ValueError("Gerber cut-in must be horizontal or vertical")
        removed.update(indices)
    if len(cutin_axes) > 1:
        raise ValueError("all cut-ins in a Gerber contour must share one axis")

    adjacency: dict[tuple[int, int], list[int]] = defaultdict(list)
    for index, (start, end, _) in enumerate(edges):
        if index not in removed:
            adjacency[start].append(index)
            adjacency[end].append(index)
    if not adjacency or any(len(incident) != 2 for incident in adjacency.values()):
        raise ValueError("Gerber contour does not decompose into closed simple loops")

    loops = []
    unused = {index for index in range(len(edges)) if index not in removed}
    while unused:
        first_edge = min(unused)
        start, current, first_arc = edges[first_edge]
        previous_edge = first_edge
        loop_vertices = [start]
        loop_centers = [first_arc]
        unused.remove(first_edge)
        if current != start:
            loop_vertices.append(current)
        while True:
            choices = [index for index in adjacency[current]
                       if index != previous_edge and index in unused]
            if current == start:
                break
            if len(choices) != 1:
                raise ValueError("Gerber cut-in decomposition is ambiguous")
            edge_index = choices[0]
            unused.remove(edge_index)
            edge_start, edge_end, arc = edges[edge_index]
            if current == edge_start:
                following = edge_end
                directed_arc = arc
            elif current == edge_end:
                following = edge_start
                directed_arc = None if arc is None else (not arc[0], arc[1])
            else:
                raise ValueError("disconnected Gerber contour edge")
            loop_centers.append(directed_arc)
            previous_edge, current = edge_index, following
            if current == start:
                break
            if current in loop_vertices:
                raise ValueError("Gerber contour loop self-touches")
            loop_vertices.append(current)
        if len(loop_vertices) != len(loop_centers):
            raise ValueError("Gerber contour loop is not explicitly closed")

        mm_outline = [(x / 1_000_000, y / 1_000_000) for x, y in loop_vertices]
        mm_centers = [None if arc is None else
                      (arc[0], (arc[1][0] / 1_000_000, arc[1][1] / 1_000_000))
                      for arc in loop_centers]
        loop = gp.ArcPoly(outline=mm_outline, arc_centers=mm_centers)
        approximated = loop.approximate_arcs(
            max_error=max_curve_error_nm / 1_000_000,
            clip_max_error=True,
        )
        polygon = Polygon(approximated.outline)
        if polygon.is_empty or not polygon.is_valid or polygon.area <= 0:
            raise ValueError("Gerber contour loop is not a valid positive-area polygon")
        loops.append(polygon)

    for index, first in enumerate(loops):
        for second in loops[index + 1:]:
            if first.boundary.intersects(second.boundary):
                raise ValueError("Gerber region loops intersect outside a valid cut-in")
            if first.intersects(second) and not first.contains(second) and not second.contains(first):
                raise ValueError("Gerber region loops overlap without containment")

    depths = []
    for index, polygon in enumerate(loops):
        depths.append(sum(
            1 for parent_index, parent in enumerate(loops)
            if parent_index != index and parent.area > polygon.area
            and parent.contains(polygon.representative_point())
        ))

    filled = []
    for index, shell in enumerate(loops):
        if depths[index] % 2:
            continue
        holes = [hole.exterior.coords for hole_index, hole in enumerate(loops)
                 if depths[hole_index] == depths[index] + 1
                 and loops[index].contains(hole.representative_point())
                 and not any(
                     middle not in (index, hole_index)
                     and depths[middle] == depths[index] + 1
                     and loops[index].contains(loops[middle].representative_point())
                     and loops[middle].contains(hole.representative_point())
                     for middle in range(len(loops))
                 )]
        filled.append(Polygon(shell.exterior.coords, holes))
    if not filled or any(not polygon.is_valid for polygon in filled):
        raise ValueError("Gerber region cut-in fill could not be normalized safely")
    return filled[0] if len(filled) == 1 else __import__("shapely").union_all(filled)


def _primitive_shape(primitive, *, max_curve_error_nm: int, region: bool):
    from gerbonara import graphic_primitives as gp
    from shapely.affinity import rotate
    from shapely.geometry import LineString, Point, Polygon, box

    if isinstance(primitive, gp.Circle):
        radius = primitive.r
        if radius <= 0:
            raise ValueError("zero-size circular Gerber primitive")
        error_mm = max_curve_error_nm / 1_000_000
        if error_mm >= radius:
            raise ValueError("curve tolerance exceeds circle radius")
        quadrant_segments = max(8, math.ceil(math.pi / (2 * math.acos(1 - error_mm / radius))))
        return Point(primitive.x, primitive.y).buffer(radius, quad_segs=quadrant_segments)
    if isinstance(primitive, gp.Rectangle):
        if primitive.w <= 0 or primitive.h <= 0:
            raise ValueError("zero-size rectangular Gerber primitive")
        shape = box(primitive.x - primitive.w / 2, primitive.y - primitive.h / 2,
                    primitive.x + primitive.w / 2, primitive.y + primitive.h / 2)
        return rotate(shape, math.degrees(primitive.rotation), origin=(primitive.x, primitive.y))
    if isinstance(primitive, gp.Line):
        if primitive.width <= 0:
            raise ValueError("zero-width Gerber line primitive")
        return LineString(((primitive.x1, primitive.y1), (primitive.x2, primitive.y2))).buffer(
            primitive.width / 2, cap_style="round", join_style="round", quad_segs=64
        )
    if isinstance(primitive, gp.ArcPoly):
        if region:
            shapes = _arc_poly_shape(primitive, max_curve_error_nm=max_curve_error_nm,
                                     allow_cutins=True)
        else:
            if any(count > 1 for count in Counter(
                tuple(sorted((_nm_point(a), _nm_point(b))))
                for a, b, _ in primitive.segments
            ).values()):
                raise ValueError("self-touching aperture primitive is unsupported")
            approximated = primitive.approximate_arcs(
                max_error=max_curve_error_nm / 1_000_000, clip_max_error=True
            )
            shapes = Polygon(approximated.outline)
        if shapes.is_empty or not shapes.is_valid or shapes.area <= 0:
            raise ValueError("invalid Gerber polygon primitive")
        return shapes
    raise ValueError(f"unsupported Gerber vector primitive: {type(primitive).__name__}")


def extract_positive_copper_artwork(path: Path, *, max_curve_error_nm: int = 100):
    """Parse one X2 Gerber and return ``(function, final-positive-shape)``.

    The returned geometry composes Gerber dark/clear polarity but does not yet
    subtract Excellon drills. Callers must do that before electrical tests.
    Coordinates are millimetres; unsupported or invalid geometry raises.
    """
    from gerbonara import graphic_objects as go
    from gerbonara.rs274x import GerberFile
    from shapely.geometry import GeometryCollection
    from shapely.ops import unary_union

    if type(max_curve_error_nm) is not int or max_curve_error_nm <= 0:
        raise ValueError("max_curve_error_nm must be a positive integer")
    raw = path.read_text(encoding="ascii", errors="strict")
    trimmed = raw.rstrip()
    last_command = trimmed[:-1].rsplit("*", 1)[-1].strip().strip("%").strip() if trimmed.endswith("*") else ""
    if not trimmed.endswith("M02*") or last_command != "M02":
        raise ValueError("Gerber is missing its mandatory final M02 command")
    functions = re.findall(r"%TF\.FileFunction,([^*]+)\*%", raw)
    if len(functions) != 1:
        raise ValueError("Gerber requires exactly one X2 FileFunction")
    file = GerberFile.open(str(path))

    dark_shapes = []
    clear_shapes = []
    for obj in file.objects:
        primitives = list(obj.to_primitives(unit="mm"))
        if not primitives:
            raise ValueError("Gerber object has no vector primitives")
        local_dark = []
        local_clear = []
        is_region = isinstance(obj, go.Region)
        for primitive in primitives:
            shape = _primitive_shape(primitive, max_curve_error_nm=max_curve_error_nm,
                                     region=is_region)
            (local_dark if primitive.polarity_dark == obj.polarity_dark else local_clear).append(shape)
        feature = unary_union(local_dark).difference(unary_union(local_clear)) if local_clear else unary_union(local_dark)
        if obj.polarity_dark:
            dark_shapes.append(feature)
        else:
            clear_shapes.append(feature)

    positive = unary_union(dark_shapes) if dark_shapes else GeometryCollection()
    if clear_shapes:
        positive = positive.difference(unary_union(clear_shapes))
    if not positive.is_valid:
        raise ValueError("composed Gerber artwork is invalid")
    return functions[0], positive


def subtract_drill_geometry(positive, drill_programs, *, max_curve_error_nm: int = 100):
    """Return final copper after all explicitly classified round drills/slots.

    ``positive`` is the positive-polarity artwork from one copper Gerber in mm.
    Every drill program must state whether it is plated or non-plated; both types
    remove copper from every layer. Plating classification is required here so
    that an incomplete Excellon inventory cannot silently pass as geometry.
    The caller remains responsible for reconciling the inventory to the native
    board and for applying the result to every copper layer.
    """
    from shapely.geometry import LineString, Point
    from shapely.ops import unary_union

    if type(max_curve_error_nm) is not int or max_curve_error_nm <= 0:
        raise ValueError("max_curve_error_nm must be a positive integer")
    if positive is None or not positive.is_valid:
        raise ValueError("positive copper artwork must be a valid geometry")

    error_mm = max_curve_error_nm / 1_000_000
    cuts = []
    for program in drill_programs:
        if getattr(program, "units", None) != "mm":
            raise ValueError("drill programs require explicit millimetre units")
        if type(getattr(program, "plated", None)) is not bool:
            raise ValueError("drill programs require explicit plated/non-plated classification")
        for hit in program.hits:
            if (type(hit.diameter_nm) is not int or hit.diameter_nm <= 0
                    or type(hit.x_nm) is not int or type(hit.y_nm) is not int):
                raise ValueError("drill hits require positive integer dimensions and integer coordinates")
            radius_mm = hit.diameter_nm / 2_000_000
            if error_mm >= radius_mm:
                raise ValueError("curve tolerance must be smaller than the drill radius")
            quad_segs = max(8, math.ceil(math.pi / (2 * math.acos(1 - error_mm / radius_mm))))
            cuts.append(Point(hit.x_nm / 1_000_000, hit.y_nm / 1_000_000).buffer(
                radius_mm, quad_segs=quad_segs
            ))
        for slot in program.slots:
            if (type(slot.diameter_nm) is not int or slot.diameter_nm <= 0
                    or len(slot.start) != 2 or len(slot.end) != 2
                    or any(type(value) is not int for value in (*slot.start, *slot.end))
                    or slot.start == slot.end):
                raise ValueError("drill slots require positive integer width and distinct integer endpoints")
            radius_mm = slot.diameter_nm / 2_000_000
            if error_mm >= radius_mm:
                raise ValueError("curve tolerance must be smaller than the slot radius")
            quad_segs = max(8, math.ceil(math.pi / (2 * math.acos(1 - error_mm / radius_mm))))
            cuts.append(LineString((
                (slot.start[0] / 1_000_000, slot.start[1] / 1_000_000),
                (slot.end[0] / 1_000_000, slot.end[1] / 1_000_000),
            )).buffer(radius_mm, cap_style="round", join_style="round", quad_segs=quad_segs))

    if not cuts:
        return positive
    final = positive.difference(unary_union(cuts))
    if not final.is_valid:
        raise ValueError("drill subtraction produced invalid copper geometry")
    return final
