"""Board artwork intent and deterministic physical silkscreen lowering.

Artwork has no electrical net. Opt-in copper artwork reserves routing space.
The bundled Cu mark is flattened from the existing vector asset.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass, field
from decimal import Decimal
from enum import Enum
from functools import lru_cache
from importlib.metadata import PackageNotFoundError, version
from importlib.resources import files
from math import cos, radians, sin
import re
from xml.etree import ElementTree

from .physical import (BoardSide, CopperLayer, CopperKeepout, PolygonRing, PolygonWithHoles,
                       FootprintArc, FootprintCircle, FootprintGraphic,
                       FootprintLayer, FootprintLine, FootprintPolygon,
                       FootprintRectangle, Point)
from .quantities import Length
from .syntax import CopperScriptError, RawQuantity, SilkscreenDecl


def compiler_version() -> str:
    try:
        return version("copperscript")
    except PackageNotFoundError:
        return "unavailable"


class ArtworkMaterial(str, Enum):
    SILKSCREEN = "silkscreen"
    COPPER = "copper"


@dataclass(frozen=True, slots=True)
class SilkscreenText:
    id: str
    text: str
    position: Point
    size_nm: int = 1_000_000
    thickness_nm: int = 150_000
    rotation_degrees: Decimal = Decimal(0)
    side: BoardSide = BoardSide.FRONT
    material: ArtworkMaterial = ArtworkMaterial.SILKSCREEN
    exposed: bool = False

    def __post_init__(self):
        _identity(self.id, self.side)
        _material(self.material, self.exposed)
        if not isinstance(self.position, Point) or not isinstance(self.text, str) or not self.text.strip():
            raise ValueError("silkscreen text requires a position and nonempty text")
        if any(ord(c) < 32 and c != '\n' for c in self.text):
            raise ValueError("silkscreen text contains unsupported control characters")
        _positive(self.size_nm, "text size")
        _positive(self.thickness_nm, "text thickness")
        if self.thickness_nm * 2 > self.size_nm:
            raise ValueError("text thickness must not exceed half its size")
        object.__setattr__(self, "rotation_degrees", _rotation(self.rotation_degrees))


@dataclass(frozen=True, slots=True)
class SilkscreenGraphic:
    id: str
    graphic: FootprintGraphic
    side: BoardSide = BoardSide.FRONT
    material: ArtworkMaterial = ArtworkMaterial.SILKSCREEN
    exposed: bool = False

    def __post_init__(self):
        _identity(self.id, self.side)
        _material(self.material, self.exposed)
        if not isinstance(self.graphic, (FootprintLine, FootprintRectangle, FootprintCircle,
                                         FootprintArc, FootprintPolygon)):
            raise ValueError("unsupported silkscreen graphic")
        if self.graphic.layer is not FootprintLayer.SILKSCREEN:
            raise ValueError("board silkscreen graphics must use the silkscreen layer")
        if self.graphic.width_nm == 0 and not getattr(self.graphic, "filled", False):
            raise ValueError("unfilled silkscreen graphics require positive thickness")


SilkscreenArtwork = SilkscreenText | SilkscreenGraphic


def _material(material, exposed):
    if not isinstance(material, ArtworkMaterial):
        raise ValueError("artwork material must be silkscreen or copper")
    if type(exposed) is not bool:
        raise ValueError("exposed must be boolean")
    if exposed and material is not ArtworkMaterial.COPPER:
        raise ValueError("exposed requires material = copper")


def _identity(identity, side):
    if not isinstance(identity, str) or not identity.strip():
        raise ValueError("silkscreen requires an id")
    if not isinstance(side, BoardSide):
        raise ValueError("silkscreen side must be front or back")


def _positive(value, label):
    if type(value) is not int or value <= 0:
        raise ValueError(f"{label} must be positive integer nanometres")


def _rotation(value):
    value = Decimal(str(value))
    if not value.is_finite():
        raise ValueError("rotation must be finite degrees")
    return value % Decimal(360)


@dataclass(frozen=True, slots=True)
class SilkscreenBranding:
    enabled: bool = True
    position: Point | None = None
    size_nm: int = 5_000_000
    rotation_degrees: Decimal = Decimal(0)
    side: BoardSide = BoardSide.FRONT
    version: str = field(default_factory=lambda: compiler_version())
    material: ArtworkMaterial = ArtworkMaterial.SILKSCREEN
    exposed: bool = False
    layout: str = "horizontal"

    def __post_init__(self):
        if type(self.enabled) is not bool:
            raise ValueError("branding enabled must be boolean")
        if self.position is not None and not isinstance(self.position, Point):
            raise ValueError("branding position requires a coordinate pair")
        _positive(self.size_nm, "logo size")
        _identity("branding", self.side)
        _material(self.material, self.exposed)
        if self.layout not in {"horizontal", "stacked"}:
            raise ValueError("branding layout must be horizontal or stacked")
        object.__setattr__(self, "rotation_degrees", _rotation(self.rotation_degrees))


@dataclass(frozen=True, slots=True)
class SilkscreenDesign:
    branding: SilkscreenBranding = field(default_factory=SilkscreenBranding)
    artwork: tuple[SilkscreenArtwork, ...] = ()

    def __post_init__(self):
        object.__setattr__(self, "artwork", tuple(self.artwork))
        validate_artwork(self.artwork)


def validate_artwork(artwork):
    if any(not isinstance(item, (SilkscreenText, SilkscreenGraphic)) for item in artwork):
        raise ValueError("silkscreen requires typed artwork")
    if len({item.id for item in artwork}) != len(artwork):
        raise ValueError("duplicate silkscreen feature id")


def _length(value):
    if not isinstance(value, RawQuantity):
        raise ValueError("silkscreen dimensions require typed lengths")
    nm = Length.of(value.value, value.unit).base_value * Decimal(1_000_000_000)
    if not nm.is_finite() or nm != nm.to_integral_value():
        raise ValueError("silkscreen lengths must be exact integer nanometres")
    return int(nm)


def _point(value):
    if not isinstance(value, tuple) or len(value) != 2:
        raise ValueError("expected a coordinate pair (x, y)")
    return Point(*map(_length, value))


def lower_silkscreen(document) -> SilkscreenDesign:
    blocks = [d for d in document.declarations if isinstance(d, SilkscreenDecl)]
    if len(blocks) > 1:
        raise CopperScriptError("SLK001", "only one silkscreen block is allowed", blocks[1].location)
    if not blocks:
        return SilkscreenDesign()
    branding, artwork, ids = SilkscreenBranding(), [], set()
    for item in blocks[0].items:
        try:
            if item.name in ids or item.name.startswith("__copperscript"):
                raise ValueError(f"duplicate or reserved silkscreen feature id {item.name!r}")
            ids.add(item.name)
            p = item.parameters
            common = {"side", "material", "exposed"}
            properties = {
                "branding": {"enabled", "position", "size", "rotation", "layout"},
                "logo": {"position", "size", "rotation"},
                "text": {"text", "position", "size", "thickness", "rotation"},
                "line": {"start", "end", "thickness"},
                "rectangle": {"start", "end", "thickness", "filled"},
                "circle": {"center", "radius", "thickness", "filled"},
                "arc": {"start", "mid", "end", "thickness"},
                "polygon": {"vertices", "thickness", "filled"},
            }
            required = {"branding": set(), "logo": {"position"},
                        "text": {"text", "position"}, "line": {"start", "end"},
                        "rectangle": {"start", "end"}, "circle": {"center", "radius"},
                        "arc": {"start", "mid", "end"}, "polygon": {"vertices"}}[item.kind]
            if set(p) - (properties[item.kind] | common):
                raise ValueError(f"unknown silkscreen property {sorted(set(p) - (properties[item.kind] | common))[0]!r}")
            if required - set(p):
                raise ValueError(f"missing silkscreen property {sorted(required - set(p))[0]!r}")
            side = BoardSide(p.get("side", "front"))
            material = ArtworkMaterial(p.get("material", "silkscreen"))
            exposed = p.get("exposed", False)
            _material(material, exposed)
            rotation = p.get("rotation", 0)
            if isinstance(rotation, (bool, RawQuantity)) or not isinstance(rotation, (int, float)):
                raise ValueError("rotation requires a number in degrees")
            if item.kind in {"branding", "logo"}:
                mark = SilkscreenBranding(
                    enabled=p.get("enabled", True),
                    position=_point(p["position"]) if "position" in p else None,
                    size_nm=_length(p["size"]) if "size" in p else 5_000_000,
                    rotation_degrees=rotation, side=side, material=material, exposed=exposed,
                    layout=p.get("layout", "horizontal"))
                if item.kind == "branding":
                    branding = mark
                else:
                    artwork.extend(_logo(item.name, mark.position, mark.size_nm, mark.rotation_degrees, side, material, exposed))
                continue
            width = _length(p["thickness"]) if "thickness" in p else 150_000
            if item.kind == "text":
                artwork.append(SilkscreenText(item.name, p["text"], _point(p["position"]),
                    _length(p["size"]) if "size" in p else 1_000_000, width, rotation, side, material, exposed))
                continue
            filled = p.get("filled", False)
            if type(filled) is not bool:
                raise ValueError("filled must be boolean")
            layer = FootprintLayer.SILKSCREEN
            if item.kind == "line":
                graphic = FootprintLine(_point(p["start"]), _point(p["end"]), width, layer)
            elif item.kind == "rectangle":
                start, end = _point(p["start"]), _point(p["end"])
                if start.x_nm == end.x_nm or start.y_nm == end.y_nm:
                    raise ValueError("silkscreen rectangle requires positive area")
                graphic = FootprintRectangle(start, end, width, layer, filled)
            elif item.kind == "circle":
                center, radius = _point(p["center"]), _length(p["radius"])
                _positive(radius, "circle radius")
                graphic = FootprintCircle(center, Point(center.x_nm + radius, center.y_nm), width, layer, filled)
            elif item.kind == "arc":
                start, mid, end = map(_point, (p["start"], p["mid"], p["end"]))
                if (mid.x_nm-start.x_nm)*(end.y_nm-start.y_nm) == (mid.y_nm-start.y_nm)*(end.x_nm-start.x_nm):
                    raise ValueError("silkscreen arc points must not be collinear")
                graphic = FootprintArc(start, mid, end, width, layer)
            else:
                from .mechanical import validated_ring
                graphic = FootprintPolygon(validated_ring(tuple(map(_point, p["vertices"]))), width, layer, filled)
            artwork.append(SilkscreenGraphic(item.name, graphic, side, material, exposed))
        except (ValueError, TypeError, ArithmeticError) as exc:
            raise CopperScriptError("SLK002", str(exc), item.location) from exc
    return SilkscreenDesign(branding, tuple(artwork))


@lru_cache(maxsize=1)
def _cu_paths():
    """Flatten the bundled logo's closed M/L/C/Z paths, in its 42-unit viewbox."""
    svg = ElementTree.fromstring(files("pcbir.editor.assets").joinpath("copper-logo.svg").read_text(encoding="utf-8"))
    paths = []
    for path in svg.findall("{http://www.w3.org/2000/svg}path"):
        tokens = iter(re.findall(r"[MLCZ]|-?\d+(?:\.\d+)?", path.attrib["d"]))
        points = []
        for command in tokens:
            if command in {"M", "L"}:
                point = (Decimal(next(tokens)), Decimal(next(tokens)))
                points.append(point)
            elif command == "C":
                p0 = points[-1]
                p1, p2, p3 = [(Decimal(next(tokens)), Decimal(next(tokens))) for _ in range(3)]
                for step in range(1, 9):
                    t = Decimal(step) / 8
                    points.append(tuple((1-t)**3*p0[i] + 3*(1-t)**2*t*p1[i] +
                                        3*(1-t)*t*t*p2[i] + t**3*p3[i] for i in (0, 1)))
            elif command == "Z":
                paths.append(tuple(points))
                points = []
            else:
                raise ValueError("unsupported bundled logo path")
    return tuple(paths)


def _transform(point, position, size, rotation, side):
    # A back-side mark is mirrored as seen through the board, including its
    # relative text placement. Board-authored shape coordinates stay absolute.
    x, y = point[0] * size / 42, point[1] * size / 42
    if side is BoardSide.BACK:
        x = -x
    angle = radians(float(rotation))
    return Point(position.x_nm + round(float(x)*cos(angle) - float(y)*sin(angle)),
                 position.y_nm + round(float(x)*sin(angle) + float(y)*cos(angle)))


def _logo(identity, position, size, rotation, side, material=ArtworkMaterial.SILKSCREEN, exposed=False):
    transform = lambda x, y: _transform((Decimal(str(x))-21, Decimal(str(y))-21), position, size, rotation, side)
    layer = FootprintLayer.SILKSCREEN
    # The colored tile becomes an unfilled border on a single-color print.
    frame = FootprintPolygon(tuple(transform(x, y) for x, y in
        ((3, 0.5), (39, 0.5), (41.5, 3), (41.5, 39), (39, 41.5), (3, 41.5), (0.5, 39), (0.5, 3))),
        max(1, size * 3 // 100), layer)
    artwork = [SilkscreenGraphic(f"{identity}:frame", frame, side, material, exposed)]
    for index, path in enumerate(_cu_paths()):
        artwork.append(SilkscreenGraphic(f"{identity}:cu:{index}",
            FootprintPolygon(tuple(transform(*p) for p in path), 0, layer, True), side, material, exposed))
    artwork.append(SilkscreenText(f"{identity}:29", "29", transform(8, 8),
        max(1, size * 16 // 100), max(1, size * 2 // 100), rotation, side, material, exposed))
    return artwork


def physical_artwork(design, outline, avoid=()):
    """Board artwork with the branding mark placed.

    ``avoid`` holds copper shapes known before placement (the pads of parts
    with a fixed position); the default mark position keeps clear of them.
    """
    artwork = list(design.artwork)
    mark = design.branding
    if mark.enabled:
        position = mark.position or _default_position(mark, outline, avoid)
        artwork.extend(_branding_artwork(mark, position))
    validate_artwork(artwork)
    return tuple(artwork)


def _branding_artwork(mark, position):
    artwork = _logo("__copperscript", position, mark.size_nm, mark.rotation_degrees,
                    mark.side, mark.material, mark.exposed)
    stacked = mark.layout == "stacked"
    labels = (("name", "CopperScript", 0, Decimal("31.5")),
              ("version", f"v{mark.version}", 0, Decimal("44.1"))) if stacked else (
              ("name", "CopperScript", 82, -5), ("version", f"v{mark.version}", 82, 7))
    for identity, text, x, y in labels:
        at = _transform((Decimal(x), Decimal(y)), position, mark.size_nm, mark.rotation_degrees, mark.side)
        artwork.append(SilkscreenText(f"__copperscript:{identity}", text, at,
            max(1, mark.size_nm // (10 if stacked else 5)),
            max(1, mark.size_nm * (2 if stacked else 3) // 100),
            mark.rotation_degrees, mark.side, mark.material, mark.exposed))
    return artwork


# Gap kept between the default branding mark and copper of fixed parts.
BRANDING_COPPER_GAP_NM = 300_000


def _default_position(mark, outline, avoid=()):
    """Bounded search inside the outline, clear of the ``avoid`` copper shapes.

    A conservative label envelope keeps the default off cutouts and curved
    edges where space permits, and off the pads of fixed parts (a corner
    connector), whose mask openings would clip the mark. Pads of parts the
    placer moves later are not known here; final clearance needs native DRC.
    Small boards retain the mark for explicit review and repositioning/resizing
    or opt-out, rather than silently dropping it.
    """
    from .geometry import RoundedConvexShape, shapes_clear
    from .mechanical import shape_in_outline
    xmin, xmax = min(p.x_nm for p in outline.vertices), max(p.x_nm for p in outline.vertices)
    ymin, ymax = min(p.y_nm for p in outline.vertices), max(p.y_nm for p in outline.vertices)
    s = mark.size_nm
    left, right = -s//2 - 150_000, s * 82 // 42 + max(12, len(mark.version)+1)*s//10 + 150_000
    low, high = -s//2 - 150_000, s//2 + 150_000
    copper = mark.material is ArtworkMaterial.COPPER
    if copper or mark.layout == "stacked":
        boxes = [artwork_bounds(item) for item in _branding_artwork(mark, Point(0, 0))]
        left, low = min(b[0] for b in boxes), min(b[1] for b in boxes)
        right, high = max(b[2] for b in boxes), max(b[3] for b in boxes)
    start = Point(xmin + 1_000_000 - left, ymin + 1_000_000 - low)
    for row in range(17):
        for column in range(17):
            x = start.x_nm + column * max(0, xmax-right-1_000_000-start.x_nm)//16
            y = start.y_nm + row * max(0, ymax-high-1_000_000-start.y_nm)//16
            corners = (tuple(Point(x+px,y+py) for px,py in ((left,low),(right,low),(right,high),(left,high)))
                if copper or mark.layout == "stacked" else tuple(_transform((Decimal(px)*42/s, Decimal(py)*42/s), Point(x,y), s,
                mark.rotation_degrees, mark.side) for px,py in ((left,low),(right,low),(right,high),(left,high))))
            envelope = RoundedConvexShape(corners, 0)
            if shape_in_outline(envelope, outline, 0) and all(
                    shapes_clear(envelope, shape, BRANDING_COPPER_GAP_NM) for shape in avoid):
                return Point(x, y)
    return start


def artwork_document(artwork):
    def encode(value):
        if isinstance(value, Decimal):
            from .quantities import decimal_text
            return decimal_text(value)
        if isinstance(value, (BoardSide, FootprintLayer, ArtworkMaterial)):
            return value.value
        if isinstance(value, dict):
            return {key: encode(item) for key, item in value.items()}
        if isinstance(value, (tuple, list)):
            return [encode(item) for item in value]
        return value
    return [{"kind": "text" if isinstance(item, SilkscreenText) else type(item.graphic).__name__,
             **encode(asdict(item))} for item in sorted(artwork, key=lambda item: item.id)]


def design_document(design):
    mark = design.branding
    return {"branding": {"enabled": mark.enabled, "position": asdict(mark.position) if mark.position else None,
                         "size_nm": mark.size_nm, "rotation_degrees": str(mark.rotation_degrees),
                         "side": mark.side.value, "version": mark.version,
                         "material": mark.material.value, "exposed": mark.exposed, "layout": mark.layout},
            "artwork": artwork_document(design.artwork)}


def artwork_layer(item):
    if item.material is ArtworkMaterial.COPPER:
        return CopperLayer.FRONT if item.side is BoardSide.FRONT else CopperLayer.BACK
    return None


def artwork_bounds(item):
    """Conservative integer envelope; native text glyphs remain unqualified.

    Curves reserve their entire circle. Text reserves a deliberately generous
    box per line and rotates that box; this is not a font geometry certificate.
    """
    from fractions import Fraction
    from math import ceil, floor, isqrt
    if isinstance(item, SilkscreenText):
        lines = item.text.split('\n')
        half_width = (max(map(len, lines)) + 2) * item.size_nm
        half_height = (len(lines) + 1) * item.size_nm
        points = tuple(_transform((Decimal(x)*42, Decimal(y)*42), item.position, 1,
            item.rotation_degrees, item.side) for x,y in
            ((-half_width,-half_height),(half_width,-half_height),(half_width,half_height),(-half_width,half_height)))
        margin = (item.thickness_nm + 1)//2 + 2
    else:
        graphic = item.graphic
        margin = (graphic.width_nm + 1)//2
        if isinstance(graphic, FootprintPolygon):
            points = graphic.points
        elif isinstance(graphic, (FootprintLine, FootprintRectangle)):
            points = (graphic.start, graphic.end)
        elif isinstance(graphic, FootprintCircle):
            center, end = graphic.center, graphic.end
            square = (end.x_nm-center.x_nm)**2 + (end.y_nm-center.y_nm)**2
            radius = isqrt(square)
            radius += radius*radius < square
            points = (Point(center.x_nm-radius, center.y_nm-radius), Point(center.x_nm+radius, center.y_nm+radius))
        else:
            a, b, c = graphic.start, graphic.midpoint, graphic.end
            denominator = 2*(a.x_nm*(b.y_nm-c.y_nm)+b.x_nm*(c.y_nm-a.y_nm)+c.x_nm*(a.y_nm-b.y_nm))
            if not denominator:
                raise ValueError("copper artwork arc points must not be collinear")
            squares = [p.x_nm*p.x_nm+p.y_nm*p.y_nm for p in (a,b,c)]
            cx = Fraction(squares[0]*(b.y_nm-c.y_nm)+squares[1]*(c.y_nm-a.y_nm)+squares[2]*(a.y_nm-b.y_nm), denominator)
            cy = Fraction(squares[0]*(c.x_nm-b.x_nm)+squares[1]*(a.x_nm-c.x_nm)+squares[2]*(b.x_nm-a.x_nm), denominator)
            square = (cx-a.x_nm)**2+(cy-a.y_nm)**2
            radius = isqrt(square.numerator//square.denominator)
            radius += radius*radius < square
            points = (Point(floor(cx)-radius, floor(cy)-radius), Point(ceil(cx)+radius, ceil(cy)+radius))
    return (min(p.x_nm for p in points)-margin, min(p.y_nm for p in points)-margin,
            max(p.x_nm for p in points)+margin, max(p.y_nm for p in points)+margin)


def copper_artwork_keepouts(board):
    """Fresh immutable reservations consumed by the ordinary board pipeline."""
    if not any(item.material is ArtworkMaterial.COPPER for item in board.silkscreen):
        return ()
    clearance = max([board.rules.minimum_clearance_nm,
                     *(rule.clearance_nm or 0 for rule in board.net_routing_rules),
                     *(zone.clearance_nm or 0 for zone in board.zones)])
    result = []
    for item in board.silkscreen:
        layer = artwork_layer(item)
        if layer is None:
            continue
        left, low, right, high = artwork_bounds(item)
        corners = (Point(left-clearance, low-clearance), Point(right+clearance, low-clearance),
                   Point(right+clearance, high+clearance), Point(left-clearance, high+clearance))
        result.append(CopperKeepout("__copper_artwork:"+item.id, (layer,),
            PolygonWithHoles(PolygonRing(corners)), block_pads=True, block_footprints=True))
    return tuple(result)
