"""Render a window of a board for review: one panel per copper layer.

``copper view`` replaces per-failure plotting scripts. It draws pads, tracks,
vias and copper areas in a millimetre window, highlights chosen nets, marks
issue positions and can overlay a manufacturer reference layout aligned on a
package's pads. Alongside the picture it returns a short text legend of what is
in the window, so a text-only reader still gets the facts.

A view is a reading aid: it neither refills zones nor checks clearances, and an
overlay does not establish that a reference layout applies to this design.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from math import atan2, cos, floor, log10, sin, sqrt
from pathlib import Path

from .scene import Scene, SceneArea, ScenePad, SceneTrack, SceneVia, track_polygon

PALETTE = ("#d1495b", "#2e86ab", "#3c9d5d", "#e08e0b", "#8e5bb5", "#0f8b8d", "#c05299", "#6d7f1e")
OTHER = "#b9c0c8"
GROUND = "#7f8c99"
REFERENCE = "#ff00aa"
MARKER = "#e00000"


@dataclass
class ViewRequest:
    window: tuple[float, float, float, float]
    layers: tuple[str, ...]
    highlight: tuple[str, ...] = ()
    markers: list[tuple[float, float]] = field(default_factory=list)
    title: str = ""
    labels: bool = True


@dataclass
class Alignment:
    reference: str
    target: str
    angle_degrees: float
    mirrored: bool
    rms_mm: float
    matched_pads: int


# --- choosing what to show ---------------------------------------------------------------

def window_around(points, margin: float = 1.5, minimum: float = 4.0) -> tuple[float, float, float, float]:
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    x0, x1, y0, y1 = min(xs) - margin, max(xs) + margin, min(ys) - margin, max(ys) + margin
    for axis in (0, 1):
        lo, hi = (x0, x1) if axis == 0 else (y0, y1)
        if hi - lo < minimum:
            c = (lo + hi) / 2
            lo, hi = c - minimum / 2, c + minimum / 2
        if axis == 0:
            x0, x1 = lo, hi
        else:
            y0, y1 = lo, hi
    return (x0, y0, x1, y1)


def layers_in_window(scene: Scene, window, nets=()) -> tuple[str, ...]:
    """Copper layers with highlighted (or, failing that, any) copper inside the window."""

    def used(only):
        found = set()
        for track in scene.tracks:
            if (not only or track.net in only) and _segment_hits(track.start, track.end, window):
                found.add(track.layer)
        # Through vias touch every layer, so they do not make a layer interesting.
        for pad in scene.pads:
            if (not only or pad.net in only) and _inside(pad.center, window):
                found.update(pad.layers if len(pad.layers) <= 2 else ())
        for area in scene.areas:
            if (not only or area.net in only) and area.kind == "fill" and _poly_hits(area.outer, window):
                found.add(area.layer)
        return found

    found = used(set(nets)) if nets else set()
    if not found:
        found = used(set())
    return tuple(layer for layer in scene.copper_layers if layer in found) or scene.copper_layers[:1]


# --- reference alignment ----------------------------------------------------------------

def align_reference(reference: Scene, ref_component: str, scene: Scene, component: str) -> tuple[Scene, Alignment]:
    """Rigidly map ``reference`` so ``ref_component``'s pads land on ``component``'s pads.

    Pads are matched by pad number. Mirroring is tried as well (a part placed on
    the other side), and the better least-squares fit wins. The returned scene
    is the reference in this board's coordinates and layer names.
    """

    ours = {p.pad: p.center for p in scene.pads if p.ref == component}
    theirs = {p.pad: p.center for p in reference.pads if p.ref == ref_component}
    common = sorted(set(ours) & set(theirs))
    if len(common) < 2:
        raise ValueError(f"need at least two pads shared by {component} and reference {ref_component}; "
                         f"found {len(common)}")
    best = None
    for mirrored in (False, True):
        src = [((-theirs[n][0]) if mirrored else theirs[n][0], theirs[n][1]) for n in common]
        dst = [ours[n] for n in common]
        angle, offset, rms = _kabsch(src, dst)
        if best is None or rms < best[3] - 1e-9:
            best = (mirrored, angle, offset, rms)
    mirrored, angle, offset, rms = best

    def transform(point):
        x, y = point
        if mirrored:
            x = -x
        c, s = cos(angle), sin(angle)
        return (c * x - s * y + offset[0], s * x + c * y + offset[1])

    layer_map = _layer_map(reference.copper_layers, scene.copper_layers, mirrored)

    def layers(names):
        return tuple(dict.fromkeys(layer_map.get(n, n) for n in names))

    mapped = Scene(name=f"{reference.name} (aligned)", copper_layers=scene.copper_layers, source=reference.source)
    mapped.pads = [replace(p, polygon=tuple(transform(q) for q in p.polygon), center=transform(p.center),
                           layers=layers(p.layers)) for p in reference.pads]
    mapped.tracks = [replace(t, start=transform(t.start), end=transform(t.end), layer=layer_map.get(t.layer, t.layer))
                     for t in reference.tracks]
    mapped.vias = [replace(v, center=transform(v.center), layers=layers(v.layers)) for v in reference.vias]
    mapped.areas = [replace(a, outer=tuple(transform(q) for q in a.outer),
                            holes=tuple(tuple(transform(q) for q in h) for h in a.holes),
                            layer=layer_map.get(a.layer, a.layer)) for a in reference.areas]
    from math import degrees
    return mapped, Alignment(ref_component, component, round(degrees(angle) % 360, 3), mirrored,
                             round(rms, 4), len(common))


def _layer_map(source: tuple[str, ...], target: tuple[str, ...], mirrored: bool) -> dict[str, str]:
    """Outer layers map to outer layers (swapped when mirrored); inner layers keep their order."""

    result = {}
    last = len(source) - 1
    inner_target = target[1:-1]
    for i, name in enumerate(source):
        if i == 0:
            result[name] = target[-1] if mirrored else target[0]
        elif i == last:
            result[name] = target[0] if mirrored else target[-1]
        elif inner_target:
            k = (last - 1 - i) if mirrored else (i - 1)
            result[name] = inner_target[max(0, min(k, len(inner_target) - 1))]
        else:
            result[name] = name
    return result


def _kabsch(src, dst):
    n = len(src)
    cs = (sum(p[0] for p in src) / n, sum(p[1] for p in src) / n)
    cd = (sum(p[0] for p in dst) / n, sum(p[1] for p in dst) / n)
    sxx = sxy = syx = syy = 0.0
    for (px, py), (qx, qy) in zip(src, dst):
        px, py, qx, qy = px - cs[0], py - cs[1], qx - cd[0], qy - cd[1]
        sxx += px * qx
        sxy += px * qy
        syx += py * qx
        syy += py * qy
    angle = atan2(sxy - syx, sxx + syy)
    c, s = cos(angle), sin(angle)
    offset = (cd[0] - (c * cs[0] - s * cs[1]), cd[1] - (s * cs[0] + c * cs[1]))
    error = sum((c * px - s * py + offset[0] - qx) ** 2 + (s * px + c * py + offset[1] - qy) ** 2
                for (px, py), (qx, qy) in zip(src, dst))
    return angle, offset, sqrt(error / n)


# --- legend -------------------------------------------------------------------------------

def legend(scene: Scene, request: ViewRequest, colors: dict[str, str], *, alignment: Alignment | None = None,
           limit: int = 14) -> list[str]:
    x0, y0, x1, y1 = request.window
    lines = [f"window x {x0:.3f}..{x1:.3f} mm, y {y0:.3f}..{y1:.3f} mm (y down); layers {', '.join(request.layers)}"]
    if colors:
        lines.append("highlight: " + ", ".join(f"{net} {colors[net]}" for net in request.highlight))
    pads = [p for p in scene.pads if _inside(p.center, request.window)]
    vias = [v for v in scene.vias if _inside(v.center, request.window)]
    tracks = [t for t in scene.tracks if t.layer in request.layers and _segment_hits(t.start, t.end, request.window)]
    lines.append(f"in window: {len(pads)} pads, {len(vias)} vias, {len(tracks)} track segments")
    focus = [p for p in pads if p.net in request.highlight] if request.highlight else pads
    for pad in focus[:limit]:
        lines.append(f"  pad {pad.ref}.{pad.pad} {pad.net or '-'} @({pad.center[0]:.3f},{pad.center[1]:.3f}) "
                     f"{'/'.join(pad.layers) if len(pad.layers) <= 2 else 'all layers'}")
    if len(focus) > limit:
        lines.append(f"  ... {len(focus) - limit} more pads")
    for net in request.highlight:
        net_vias = [v for v in vias if v.net == net]
        per_layer = {}
        for track in tracks:
            if track.net == net:
                per_layer[track.layer] = per_layer.get(track.layer, 0) + 1
        if net_vias or per_layer:
            lines.append(f"  {net}: {len(net_vias)} via(s); tracks " +
                         (", ".join(f"{layer} {count}" for layer, count in per_layer.items()) or "none"))
    if request.markers:
        shown = ", ".join(f"({x:.3f},{y:.3f})" for x, y in request.markers[:6])
        lines.append(f"markers (red rings): {len(request.markers)}: {shown}" + (" ..." if len(request.markers) > 6 else ""))
    if alignment:
        lines.append(f"reference (magenta outlines): {alignment.reference} -> {alignment.target}, "
                     f"{alignment.matched_pads} pads, rotation {alignment.angle_degrees} deg"
                     f"{', mirrored' if alignment.mirrored else ''}, fit RMS {alignment.rms_mm} mm")
    return lines


# --- rendering ------------------------------------------------------------------------

def net_colors(nets) -> dict[str, str]:
    return {net: PALETTE[i % len(PALETTE)] for i, net in enumerate(nets)}


def render(scene: Scene, request: ViewRequest, output: str | Path, *, reference: Scene | None = None,
           alignment: Alignment | None = None, panel_px: int = 620) -> tuple[Path, list[str]]:
    """Write ``output`` (PNG with Pillow, otherwise SVG) and return it with the legend lines."""

    colors = net_colors(request.highlight)
    lines = legend(scene, request, colors, alignment=alignment)
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    try:
        from PIL import Image, ImageDraw, ImageFont  # noqa: F401
    except ImportError:
        output = output.with_suffix(".svg")
        output.write_text(_svg(scene, request, colors, reference), encoding="utf-8")
        return output, lines + ["(Pillow not installed: wrote SVG)"]
    image = _png(scene, request, colors, reference, panel_px)
    if output.suffix.lower() != ".png":
        output = output.with_suffix(".png")
    image.save(output, optimize=True)
    return output, lines


class _Panel:
    def __init__(self, window, origin, scale, ss):
        self.window, self.origin, self.scale, self.ss = window, origin, scale, ss

    def xy(self, point):
        x0, y0 = self.window[0], self.window[1]
        return ((self.origin[0] + (point[0] - x0) * self.scale) * self.ss,
                (self.origin[1] + (point[1] - y0) * self.scale) * self.ss)

    def r(self, mm):
        return mm * self.scale * self.ss


def _layout(request: ViewRequest, panel_px: int):
    x0, y0, x1, y1 = request.window
    width, height = x1 - x0, y1 - y0
    scale = panel_px / max(width, height)
    panel_w, panel_h = width * scale, height * scale
    n = len(request.layers)
    columns = 1 if n == 1 else 2 if n in (2, 4) else 3
    rows = -(-n // columns)
    pad_l, pad_t, gap, title = 46, 34, 26, 30
    total_w = int(pad_l + columns * panel_w + (columns - 1) * gap + 16)
    total_h = int(title + rows * (panel_h + pad_t) + (rows - 1) * gap + 30)
    origins = [(pad_l + (i % columns) * (panel_w + gap), title + pad_t + (i // columns) * (panel_h + pad_t + gap))
               for i in range(n)]
    return scale, (panel_w, panel_h), origins, (total_w, total_h)


def _font(size):
    from PIL import ImageFont
    for name in ("DejaVuSans.ttf", "Arial.ttf", "arial.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    try:
        return ImageFont.load_default(size=size)
    except TypeError:
        return ImageFont.load_default()


def _png(scene, request, colors, reference, panel_px):
    from PIL import Image, ImageDraw
    ss = 2
    scale, (pw, ph), origins, (tw, th) = _layout(request, panel_px)
    image = Image.new("RGB", (tw * ss, th * ss), "white")
    draw = ImageDraw.Draw(image, "RGBA")
    font, small = _font(13 * ss), _font(10 * ss)
    draw.text((12 * ss, 8 * ss), request.title or scene.name, fill="#111111", font=font)
    size = (int(round(pw * ss)), int(round(ph * ss)))
    for layer, origin in zip(request.layers, origins):
        # Each panel is drawn on its own canvas, so copper never spills past its window.
        canvas = Image.new("RGB", size, "#fbfbf8")
        pen = ImageDraw.Draw(canvas, "RGBA")
        panel = _Panel(request.window, (0, 0), scale, ss)
        _grid_lines(pen, panel, request.window, size)
        _draw_layer(pen, panel, scene, layer, request, colors, outline_only=False)
        if reference is not None:
            _draw_layer(pen, panel, reference, layer, request, {}, outline_only=True)
        for point in request.markers:
            if _inside(point, request.window):
                cx, cy = panel.xy(point)
                radius = max(panel.r(0.35), 7 * ss)
                pen.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), outline=MARKER, width=2 * ss)
        left, top = int(round(origin[0] * ss)), int(round(origin[1] * ss))
        image.paste(canvas, (left, top))
        draw.rectangle((left, top, left + size[0], top + size[1]), outline="#555555", width=ss)
        draw.text((left, top - 22 * ss), layer, fill="#111111", font=font)
        _grid_labels(draw, _Panel(request.window, origin, scale, ss), request.window,
                     (left, top, left + size[0], top + size[1]), small)
    return image.resize((tw, th), Image.LANCZOS)


def _grid_values(lo, hi, step):
    value = floor(lo / step) * step
    while value <= hi + 1e-9:
        if value >= lo - 1e-9:
            yield value
        value += step


def _grid_lines(draw, panel, window, size):
    x0, y0, x1, y1 = window
    step = _nice_step(max(x1 - x0, y1 - y0) / 6)
    for value in _grid_values(x0, x1, step):
        x, _ = panel.xy((value, y0))
        draw.line((x, 0, x, size[1]), fill=(0, 0, 0, 26), width=panel.ss)
    for value in _grid_values(y0, y1, step):
        _, y = panel.xy((x0, value))
        draw.line((0, y, size[0], y), fill=(0, 0, 0, 26), width=panel.ss)


def _grid_labels(draw, panel, window, box, font):
    x0, y0, x1, y1 = window
    step = _nice_step(max(x1 - x0, y1 - y0) / 6)
    left, top, right, bottom = box
    for value in _grid_values(x0, x1, step):
        x, _ = panel.xy((value, y0))
        draw.text((x, bottom + 3 * panel.ss), _fmt(value), fill="#666666", font=font, anchor="mt")
    for value in _grid_values(y0, y1, step):
        _, y = panel.xy((x0, value))
        draw.text((left - 4 * panel.ss, y), _fmt(value), fill="#666666", font=font, anchor="rm")


def _nice_step(raw):
    exponent = floor(log10(raw)) if raw > 0 else 0
    for factor in (1, 2, 5, 10):
        if factor * 10 ** exponent >= raw:
            return factor * 10 ** exponent
    return 10 ** (exponent + 1)


def _fmt(value):
    return f"{value:.2f}".rstrip("0").rstrip(".")


def _rgba(color, alpha):
    color = color.lstrip("#")
    return tuple(int(color[i:i + 2], 16) for i in (0, 2, 4)) + (alpha,)


def _color(net, colors):
    if net in colors:
        return colors[net]
    return GROUND if net.upper() in {"GND", "AGND", "DGND", "VSS"} else OTHER


def _draw_layer(draw, panel, scene, layer, request, colors, *, outline_only):
    window = request.window
    expanded = (window[0] - 2, window[1] - 2, window[2] + 2, window[3] + 2)
    ss = panel.ss
    for area in scene.areas:
        if area.layer != layer or not _poly_hits(area.outer, expanded):
            continue
        points = [panel.xy(p) for p in area.outer]
        if len(points) < 3:
            continue
        color = REFERENCE if outline_only else _color(area.net, colors)
        if area.kind == "fill" and not outline_only:
            draw.polygon(points, fill=_rgba(color, 70 if area.net in colors else 45))
            for hole in area.holes:
                draw.polygon([panel.xy(p) for p in hole], fill="#fbfbf8")
        else:
            draw.line(points + points[:1], fill=_rgba(color, 150), width=ss)
    for track in scene.tracks:
        if track.layer != layer or not _segment_hits(track.start, track.end, expanded):
            continue
        polygon = [panel.xy(p) for p in track_polygon(track.start, track.end, track.width)]
        color = _color(track.net, colors)
        if outline_only:
            draw.line(polygon + polygon[:1], fill=_rgba(REFERENCE, 220), width=ss)
        else:
            draw.polygon(polygon, fill=_rgba(color, 235 if track.net in colors else 200))
    for pad in scene.pads:
        if layer not in pad.layers or not _inside(pad.center, expanded):
            continue
        polygon = [panel.xy(p) for p in pad.polygon]
        if outline_only:
            draw.line(polygon + polygon[:1], fill=_rgba(REFERENCE, 255), width=ss * 2)
            continue
        color = _color(pad.net, colors)
        draw.polygon(polygon, fill=_rgba(color, 255), outline=_rgba("#333333", 120))
        if pad.drill:
            cx, cy = panel.xy(pad.center)
            r = panel.r(pad.drill / 2)
            draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill="white")
    for via in scene.vias:
        if layer not in via.layers or not _inside(via.center, expanded):
            continue
        cx, cy = panel.xy(via.center)
        r, d = panel.r(via.size / 2), panel.r(via.drill / 2)
        if outline_only:
            draw.ellipse((cx - r, cy - r, cx + r, cy + r), outline=_rgba(REFERENCE, 255), width=ss * 2)
            continue
        draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=_rgba(_color(via.net, colors), 255),
                     outline=_rgba("#222222", 160), width=ss)
        draw.ellipse((cx - d, cy - d, cx + d, cy + d), fill="white")
    if request.labels and not outline_only:
        _labels(draw, panel, scene, layer, request, colors)


def _labels(draw, panel, scene, layer, request, colors):
    window = request.window
    width = window[2] - window[0]
    font = _font(int((10 if width > 12 else 11) * panel.ss))
    placed = []
    if width <= 30:
        for ref, footprint in scene.footprints.items():
            on_layer = any(layer in p.layers for p in scene.pads if p.ref == ref and len(p.layers) <= 2) or \
                any(p.ref == ref and len(p.layers) > 2 for p in scene.pads)
            if on_layer and _inside(footprint.center, window):
                x, y = panel.xy(footprint.center)
                _text(draw, (x, y), ref, font, placed, "#111111")
    if width <= 12:
        for pad in scene.pads:
            if layer in pad.layers and _inside(pad.center, window) and (not request.highlight or pad.net in colors):
                x, y = panel.xy(pad.center)
                _text(draw, (x, y + 9 * panel.ss), f"{pad.pad} {pad.net}", font, placed, "#222222")


def _text(draw, xy, text, font, placed, fill):
    x, y = xy
    box = draw.textbbox((x, y), text, font=font, anchor="mm")
    if any(not (box[2] < b[0] or box[0] > b[2] or box[3] < b[1] or box[1] > b[3]) for b in placed):
        return
    placed.append(box)
    draw.rectangle(box, fill=(255, 255, 255, 170))
    draw.text((x, y), text, fill=fill, font=font, anchor="mm")


def _svg(scene, request, colors, reference):
    x0, y0, x1, y1 = request.window
    parts = []
    for i, layer in enumerate(request.layers):
        ox = i * (x1 - x0 + 1)
        g = [f'<g transform="translate({ox - x0},{-y0})"><text x="{x0}" y="{y0 - 0.3}" font-size="0.6">{layer}</text>']
        for track in scene.tracks:
            if track.layer == layer and _segment_hits(track.start, track.end, request.window):
                g.append(f'<line x1="{track.start[0]}" y1="{track.start[1]}" x2="{track.end[0]}" y2="{track.end[1]}" '
                         f'stroke="{_color(track.net, colors)}" stroke-width="{track.width}" stroke-linecap="round"/>')
        for pad in scene.pads:
            if layer in pad.layers and _inside(pad.center, request.window):
                points = " ".join(f"{x:.4f},{y:.4f}" for x, y in pad.polygon)
                g.append(f'<polygon points="{points}" fill="{_color(pad.net, colors)}"/>')
        for via in scene.vias:
            if layer in via.layers and _inside(via.center, request.window):
                g.append(f'<circle cx="{via.center[0]}" cy="{via.center[1]}" r="{via.size / 2}" '
                         f'fill="{_color(via.net, colors)}"/>')
        for x, y in request.markers:
            g.append(f'<circle cx="{x}" cy="{y}" r="0.35" fill="none" stroke="{MARKER}" stroke-width="0.08"/>')
        g.append("</g>")
        parts.extend(g)
    width = len(request.layers) * (x1 - x0 + 1)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 -1 {width} {y1 - y0 + 1}">'
            f'<rect x="0" y="-1" width="{width}" height="{y1 - y0 + 1}" fill="white"/>' + "".join(parts) + "</svg>")


# --- geometry helpers -----------------------------------------------------------------

def _inside(point, window) -> bool:
    return window[0] <= point[0] <= window[2] and window[1] <= point[1] <= window[3]


def _segment_hits(a, b, window) -> bool:
    if _inside(a, window) or _inside(b, window):
        return True
    return not (max(a[0], b[0]) < window[0] or min(a[0], b[0]) > window[2]
                or max(a[1], b[1]) < window[1] or min(a[1], b[1]) > window[3])


def _poly_hits(points, window) -> bool:
    if not points:
        return False
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    return not (max(xs) < window[0] or min(xs) > window[2] or max(ys) < window[1] or min(ys) > window[3])
