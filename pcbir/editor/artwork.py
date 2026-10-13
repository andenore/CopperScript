"""Read-only artwork geometry for the editor, derived from accepted physical IR."""
from math import atan2, pi, sqrt
from types import SimpleNamespace

from ..mechanical_curves import arc_circle
from ..physical import (FootprintArc, FootprintCircle, FootprintLayer, FootprintLine,
                       FootprintPolygon, FootprintRectangle, Point)
from ..placement import transformed_local_point
from ..silkscreen import SilkscreenText


def _point(point):
    return [point.x_nm, point.y_nm]


def graphic_scene(graphic, transform=lambda point: point):
    result = {"width_nm": graphic.width_nm, "filled": getattr(graphic, "filled", False)}
    point = lambda value: _point(transform(value))
    if isinstance(graphic, FootprintLine):
        result.update(kind="line", start=point(graphic.start), end=point(graphic.end))
    elif isinstance(graphic, FootprintRectangle):
        a, b = graphic.start, graphic.end
        result.update(kind="polygon", vertices=[point(p) for p in
            (a, Point(b.x_nm, a.y_nm), b, Point(a.x_nm, b.y_nm))])
    elif isinstance(graphic, FootprintCircle):
        result.update(kind="circle", center=point(graphic.center), end=point(graphic.end))
    elif isinstance(graphic, FootprintPolygon):
        result.update(kind="polygon", vertices=[point(p) for p in graphic.points])
    elif isinstance(graphic, FootprintArc):
        a, m, b = [transform(p) for p in (graphic.start, graphic.midpoint, graphic.end)]
        cx, cy, r2 = arc_circle(SimpleNamespace(start=a, mid=m, end=b))
        angles = [atan2(float(p.y_nm-cy), float(p.x_nm-cx)) for p in (a, m, b)]
        sweep = (angles[2]-angles[0]) % (2*pi)
        if (angles[1]-angles[0]) % (2*pi) > sweep:
            sweep -= 2*pi
        # Preserve all three exact points; radius/flags are derived presentation
        # values. Artwork arcs may exceed the boundary pipeline's 180° limit.
        result.update(kind="arc", start=_point(a), mid=_point(m), end=_point(b),
                      radius_nm=sqrt(r2), large_arc=abs(sweep)>pi+1e-12, sweep=sweep>0)
    else:
        raise ValueError(f"unsupported editor artwork graphic: {type(graphic).__name__}")
    return result


def board_artwork_scene(board):
    result = []
    for item in board.silkscreen:
        entry = {"id": item.id, "side": item.side.value, "material": item.material.value,
                 "exposed": item.exposed, "branding": item.id.startswith("__copperscript:"),
                 "layer": ("F" if item.side.value=="front" else "B") +
                          (".Cu" if item.material.value=="copper" else ".SilkS")}
        if isinstance(item, SilkscreenText):
            entry.update(kind="text", text=item.text, position=_point(item.position),
                size_nm=item.size_nm, width_nm=item.thickness_nm,
                rotation=str(item.rotation_degrees), mirrored=item.side.value=="back")
        else:
            entry.update(graphic_scene(item.graphic))
        result.append(entry)
    return result


def footprint_silkscreen_scene(footprint, pose):
    return [{"id": f"{pose.reference}:silkscreen:{index}",
             **graphic_scene(graphic, lambda point: transformed_local_point(pose, point))}
            for index, graphic in enumerate(footprint.graphics)
            if graphic.layer is FootprintLayer.SILKSCREEN]
