"""Backend-neutral physical PCB intermediate representation.

The electrical IR describes connectivity and intent.  This module describes a
particular physical realization of that design: board geometry, resolved
footprints, placements, nets, tracks, and vias.  Coordinates are stored as
integer nanometres so serialization and backend output are deterministic.

Only a deliberately small manufacturing-oriented subset is modeled here.  New
geometry can be added without introducing KiCad concepts into the electrical
IR or the CopperScript frontend.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from decimal import Decimal, ROUND_HALF_UP
from enum import Enum
from types import MappingProxyType
from typing import Mapping
from .pad_connections import InternalPadGroup, validate_internal_pad_groups


Nanometres = int


def nm_from_mm(value: int | float | str | Decimal) -> Nanometres:
    """Convert millimetres to exact integer nanometres."""

    return int(
        (Decimal(str(value)) * Decimal("1000000")).to_integral_value(
            rounding=ROUND_HALF_UP
        )
    )


class BoardSide(str, Enum):
    FRONT = "front"
    BACK = "back"


class CopperLayer(str, Enum):
    FRONT = "F.Cu"
    INTERNAL_1 = "In1.Cu"
    INTERNAL_2 = "In2.Cu"
    INTERNAL_3 = "In3.Cu"
    INTERNAL_4 = "In4.Cu"
    INTERNAL_5 = "In5.Cu"
    INTERNAL_6 = "In6.Cu"
    INTERNAL_7 = "In7.Cu"
    INTERNAL_8 = "In8.Cu"
    INTERNAL_9 = "In9.Cu"
    INTERNAL_10 = "In10.Cu"
    INTERNAL_11 = "In11.Cu"
    INTERNAL_12 = "In12.Cu"
    INTERNAL_13 = "In13.Cu"
    INTERNAL_14 = "In14.Cu"
    INTERNAL_15 = "In15.Cu"
    INTERNAL_16 = "In16.Cu"
    INTERNAL_17 = "In17.Cu"
    INTERNAL_18 = "In18.Cu"
    INTERNAL_19 = "In19.Cu"
    INTERNAL_20 = "In20.Cu"
    INTERNAL_21 = "In21.Cu"
    INTERNAL_22 = "In22.Cu"
    INTERNAL_23 = "In23.Cu"
    INTERNAL_24 = "In24.Cu"
    INTERNAL_25 = "In25.Cu"
    INTERNAL_26 = "In26.Cu"
    INTERNAL_27 = "In27.Cu"
    INTERNAL_28 = "In28.Cu"
    INTERNAL_29 = "In29.Cu"
    INTERNAL_30 = "In30.Cu"
    BACK = "B.Cu"


class StackupLayerKind(str, Enum):
    COPPER = "copper"
    DIELECTRIC = "dielectric"


class ViaKind(str, Enum):
    THROUGH = "through"
    BLIND = "blind"
    BURIED = "buried"
    MICROVIA = "microvia"


class ReturnViaPolicy(str, Enum):
    ALWAYS = "always"
    REFERENCE_CHANGE = "reference_change"


class TuningStyle(str, Enum):
    """Length-match tuning geometry (D-PHY plan R10)."""

    BUMPS = "bumps"            # one-sided bumps (R3)
    SERPENTINE = "serpentine"  # two-sided S-shaped legs, bumps where one side is blocked


class PadKind(str, Enum):
    SMD = "smd"
    APERTURE = "aperture"
    THROUGH_HOLE = "through_hole"
    NON_PLATED_THROUGH_HOLE = "non_plated_through_hole"


class PadShape(str, Enum):
    CIRCLE = "circle"
    OVAL = "oval"
    RECTANGLE = "rectangle"
    ROUNDRECT = "roundrect"


class FootprintLayer(str, Enum):
    """Placement-relative non-copper footprint drawing layers."""

    SILKSCREEN = "silkscreen"
    FABRICATION = "fabrication"
    COURTYARD = "courtyard"
    ADHESIVE = "adhesive"
    DOCUMENTATION = "documentation"
    SOLDER_MASK = "solder_mask"
    SOLDER_PASTE = "solder_paste"


class AlignmentAxis(str, Enum):
    X = "x"
    Y = "y"


class RelativePlacementKind(str, Enum):
    MAX_DISTANCE = "max_distance"
    MIN_DISTANCE = "min_distance"
    ALIGN = "align"


class RouteKind(str, Enum):
    GENERAL = "general"
    CRITICAL = "critical"
    DIFFERENTIAL = "differential"
    CLOCK = "clock"
    CAN_BUS = "can_bus"
    RF_FEED = "rf_feed"
    POWER = "power"


class ZoneConnection(str, Enum):
    THERMAL = "thermal"
    SOLID = "solid"
    NONE = "none"
    THT_THERMAL = "tht_thermal"


class IslandPolicy(str, Enum):
    REMOVE_ALL = "remove_all"
    KEEP_ALL = "keep_all"
    REMOVE_BELOW_AREA = "remove_below_area"


class ZoneFillMode(str, Enum):
    SOLID = "solid"


@dataclass(frozen=True, slots=True)
class Point:
    x_nm: Nanometres
    y_nm: Nanometres

    @classmethod
    def mm(cls, x: int | float | str, y: int | float | str) -> "Point":
        return cls(nm_from_mm(x), nm_from_mm(y))


@dataclass(frozen=True, slots=True)
class Size:
    width_nm: Nanometres
    height_nm: Nanometres

    def __post_init__(self) -> None:
        if self.width_nm <= 0 or self.height_nm <= 0:
            raise ValueError("physical sizes must be positive")

    @classmethod
    def mm(cls, width: int | float | str, height: int | float | str) -> "Size":
        return cls(nm_from_mm(width), nm_from_mm(height))


@dataclass(frozen=True, slots=True)
class BoardCutout:
    """Named polygon of removed substrate, not a copper keepout."""

    id: str
    vertices: tuple[Point, ...]

    def __post_init__(self) -> None:
        from .mechanical import validated_ring
        if not isinstance(self.id, str) or not self.id.strip():
            raise ValueError("board cutout requires an id")
        object.__setattr__(self, "vertices", validated_ring(self.vertices))


@dataclass(frozen=True, slots=True)
class BoardDatum:
    id: str
    position: Point
    relative_to: str | None = None
    offset: Point = Point(0, 0)

    def __post_init__(self):
        if not self.id or not isinstance(self.position, Point):
            raise ValueError("datum requires an id and a typed point")


@dataclass(frozen=True, slots=True)
class BoardEdge:
    id: str
    start: Point
    end: Point

    def __post_init__(self):
        if not self.id or self.start == self.end:
            raise ValueError("named edge requires an id and distinct endpoints")


@dataclass(frozen=True, slots=True)
class PhysicalAttachment:
    id: str
    reference: str
    target: str | None
    position: Point
    offset: Point
    anchor: str
    anchor_point: Point
    rotation: Decimal
    side: BoardSide

    def __post_init__(self):
        if not self.id or not self.reference or self.anchor not in {"origin", "pad", "mating_face"}:
            raise ValueError("attachment requires stable identities and a supported anchor")
        if not all(isinstance(p,Point) for p in (self.position,self.offset,self.anchor_point)):
            raise ValueError("attachment coordinates require typed points")
        if not isinstance(self.rotation,Decimal) or not self.rotation.is_finite() or not isinstance(self.side,BoardSide):
            raise ValueError("attachment pose requires finite Decimal rotation and BoardSide")


@dataclass(frozen=True, slots=True)
class BodyOverhang:
    id: str
    reference: str
    edge: str
    start_nm: Nanometres
    end_nm: Nanometres
    distance_nm: Nanometres
    reason: str

    def __post_init__(self):
        if not all(isinstance(v,str) and v.strip() for v in (self.id,self.reference,self.edge,self.reason)):
            raise ValueError("body overhang requires identities and an explicit audit reason")
        if not all(type(v) is int for v in (self.start_nm,self.end_nm,self.distance_nm)) or not 0 <= self.start_nm < self.end_nm or self.distance_nm <= 0:
            raise ValueError("body overhang requires a positive depth and ordered nonnegative edge interval")


@dataclass(frozen=True, slots=True)
class ComponentHeight:
    id: str
    reference: str
    height_nm: Nanometres

    def __post_init__(self):
        if not self.id or not self.reference or type(self.height_nm) is not int or self.height_nm <= 0:
            raise ValueError("component height requires identities and a positive typed length")


@dataclass(frozen=True, slots=True)
class AssemblyEnvelope:
    id: str
    outline: BoardOutline
    side: BoardSide
    maximum_height_nm: Nanometres

    def __post_init__(self):
        if not self.id or not isinstance(self.side,BoardSide) or type(self.maximum_height_nm) is not int or self.maximum_height_nm < 0:
            raise ValueError("enclosure requires an id, side and nonnegative typed height")
        if self.outline.cutouts or self.outline.circular_boundary:
            raise ValueError("enclosure requires a simple polygonal region")


@dataclass(frozen=True, slots=True)
class AssemblyAccess:
    id: str
    reference: str
    outline: BoardOutline
    side: str
    purpose: str

    def __post_init__(self):
        if not self.id or not self.reference or self.side not in {'component','opposite'} or not isinstance(self.purpose,str) or not self.purpose.strip():
            raise ValueError("assembly access requires an owner, relative side and explicit purpose")
        if self.outline.cutouts or self.outline.circular_boundary:
            raise ValueError("assembly access requires a simple polygonal local region")


@dataclass(frozen=True, slots=True)
class MechanicalHole:
    """Board-owned round NPTH; no electrical pin, net or BOM entry."""

    id: str
    position: Point
    diameter_nm: Nanometres
    head_clearance_radius_nm: Nanometres = 0

    def __post_init__(self) -> None:
        if (not isinstance(self.id, str) or not self.id.strip()
                or type(self.diameter_nm) is not int or self.diameter_nm <= 0):
            raise ValueError("mechanical hole requires an id and positive integer diameter")
        if type(self.head_clearance_radius_nm) is not int or self.head_clearance_radius_nm < 0:
            raise ValueError("mechanical head clearance radius must be a nonnegative integer")
        if self.head_clearance_radius_nm and 2 * self.head_clearance_radius_nm < self.diameter_nm:
            raise ValueError("mechanical head clearance cannot be smaller than the hole")


@dataclass(frozen=True, slots=True)
class MechanicalSlot:
    """Board-owned non-plated capsule: endpoints are tool-centre positions."""
    id: str
    start: Point
    end: Point
    width_nm: Nanometres

    def __post_init__(self):
        if not isinstance(self.id,str) or not self.id.strip() or not all(isinstance(p,Point) for p in (self.start,self.end)) or self.start==self.end:
            raise ValueError('routed slot requires an id and distinct typed endpoints')
        if type(self.width_nm) is not int or self.width_nm<=0:
            raise ValueError('routed slot width must be a positive integer length')


@dataclass(frozen=True, slots=True)
class BoundaryLine:
    id: str
    start: Point
    end: Point

    def __post_init__(self):
        if not isinstance(self.id,str) or not self.id.strip() or not all(isinstance(p,Point) for p in (self.start,self.end)) or self.start==self.end:
            raise ValueError('boundary line requires an id and distinct typed endpoints')


@dataclass(frozen=True, slots=True)
class BoundaryArc:
    id: str
    start: Point
    mid: Point
    end: Point

    def __post_init__(self):
        if not isinstance(self.id,str) or not self.id.strip() or not all(isinstance(p,Point) for p in (self.start,self.mid,self.end)) or len({self.start,self.mid,self.end})!=3:
            raise ValueError('boundary arc requires an id and three distinct typed points')


@dataclass(frozen=True, slots=True)
class BoardBoundaryPath:
    segments: tuple[BoundaryLine | BoundaryArc,...]
    maximum_chord_error_nm: Nanometres = nm_from_mm('0.01')

    def __post_init__(self):
        object.__setattr__(self,'segments',tuple(self.segments))
        if not 2 <= len(self.segments) <= 256 or not all(isinstance(s,(BoundaryLine,BoundaryArc)) for s in self.segments):
            raise ValueError('boundary path requires 2–256 line/arc primitives')
        if len({s.id for s in self.segments})!=len(self.segments):raise ValueError('boundary primitive IDs must be unique')
        if type(self.maximum_chord_error_nm) is not int or self.maximum_chord_error_nm <= 2:
            raise ValueError('boundary path chord error must exceed 2 nm')
        if any(a.end!=b.start for a,b in zip(self.segments,(*self.segments[1:],self.segments[0]))):
            raise ValueError('boundary path must be exactly connected and closed; no healing')


@dataclass(frozen=True, slots=True)
class CircularBoardBoundary:
    """Authoritative circle; an explicit bounded inscribed ring aids grid consumers."""

    center: Point
    radius_nm: Nanometres
    maximum_chord_error_nm: Nanometres = nm_from_mm("0.01")

    def __post_init__(self) -> None:
        if type(self.radius_nm) is not int or self.radius_nm <= 0:
            raise ValueError("circular board radius must be a positive integer")
        if (type(self.maximum_chord_error_nm) is not int
                or not 2 < self.maximum_chord_error_nm < self.radius_nm):
            raise ValueError("circular board chord error must be >2 nm and smaller than the radius")


@dataclass(frozen=True, slots=True)
class BoardOutline:
    """Actual boundary, with a closed conservative query ring (closing edge implicit)."""

    vertices: tuple[Point, ...]
    cutouts: tuple[BoardCutout, ...] = ()
    circular_boundary: CircularBoardBoundary | None = None
    boundary_path: BoardBoundaryPath | None = None

    def __post_init__(self) -> None:
        from .mechanical import circle_query_ring, validated_ring, validate_cutouts
        object.__setattr__(self, "vertices", validated_ring(self.vertices))
        if self.circular_boundary is not None and self.vertices != circle_query_ring(self.circular_boundary):
            raise ValueError("circular board query ring must match its authoritative circle")
        if self.boundary_path is not None:
            from .mechanical_curves import path_query_ring
            if self.circular_boundary or self.vertices!=path_query_ring(self.boundary_path):
                raise ValueError('board query ring must match its authoritative line/arc path')
        object.__setattr__(self, "cutouts", tuple(self.cutouts))
        validate_cutouts(self)

    @classmethod
    def from_path(cls,path,*,cutouts=()):
        from .mechanical_curves import path_query_ring
        return cls(path_query_ring(path),cutouts,boundary_path=path)

    @classmethod
    def rounded_rectangle(cls,width_mm,height_mm,corner_radius_mm,*,origin=None,maximum_chord_error_mm='0.01'):
        from .mechanical_curves import rounded_rectangle_path
        return cls.from_path(rounded_rectangle_path(origin or Point(0,0),nm_from_mm(width_mm),nm_from_mm(height_mm),
            nm_from_mm(corner_radius_mm),nm_from_mm(maximum_chord_error_mm)))

    @classmethod
    def circle(cls, diameter_mm: int | float | str, *, center: Point | None = None,
               maximum_chord_error_mm: int | float | str = "0.01",
               cutouts: tuple[BoardCutout, ...] = ()) -> "BoardOutline":
        from .mechanical import circle_query_ring
        diameter = nm_from_mm(diameter_mm)
        if diameter <= 0 or diameter % 2:
            raise ValueError("circular board diameter must be positive and an even number of nanometres")
        radius = diameter // 2
        circle = CircularBoardBoundary(center or Point(radius, radius), radius,
                                       nm_from_mm(maximum_chord_error_mm))
        return cls(circle_query_ring(circle), cutouts, circle)

    @classmethod
    def rectangle(
        cls,
        width_mm: int | float | str,
        height_mm: int | float | str,
        *,
        origin: Point | None = None,
    ) -> "BoardOutline":
        start = origin or Point(0, 0)
        width = nm_from_mm(width_mm)
        height = nm_from_mm(height_mm)
        if width <= 0 or height <= 0:
            raise ValueError("board dimensions must be positive")
        return cls(
            (
                start,
                Point(start.x_nm + width, start.y_nm),
                Point(start.x_nm + width, start.y_nm + height),
                Point(start.x_nm, start.y_nm + height),
            )
        )


def _signed_area_twice(vertices: tuple[Point, ...]) -> int:
    return sum(
        first.x_nm * second.y_nm - second.x_nm * first.y_nm
        for first, second in zip(vertices, (*vertices[1:], vertices[0]))
    )


@dataclass(frozen=True, slots=True)
class PolygonRing:
    vertices: tuple[Point, ...]

    def __post_init__(self) -> None:
        vertices = tuple(self.vertices)
        if len(vertices) > 1 and vertices[0] == vertices[-1]:
            vertices = vertices[:-1]
        if len(vertices) < 3 or len(set(vertices)) < 3:
            raise ValueError("a polygon ring requires three distinct vertices")
        if _signed_area_twice(vertices) == 0:
            raise ValueError("a polygon ring cannot have zero area")
        object.__setattr__(self, "vertices", vertices)


@dataclass(frozen=True, slots=True)
class PolygonWithHoles:
    outer: PolygonRing
    holes: tuple[PolygonRing, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "holes", tuple(self.holes))


@dataclass(frozen=True, slots=True)
class ThermalReliefSettings:
    gap_nm: Nanometres = nm_from_mm("0.3")
    spoke_width_nm: Nanometres = nm_from_mm("0.3")
    spoke_count: int = 4

    def __post_init__(self) -> None:
        if self.gap_nm <= 0 or self.spoke_width_nm <= 0:
            raise ValueError("thermal relief gap and spoke width must be positive")
        if self.spoke_count not in {2, 3, 4}:
            raise ValueError("thermal relief spoke count must be 2, 3, or 4")


@dataclass(frozen=True, slots=True)
class CopperZone:
    id: str
    net: str
    layers: tuple[CopperLayer, ...]
    outline: PolygonWithHoles
    priority: int = 0
    clearance_nm: Nanometres | None = None
    minimum_width_nm: Nanometres = nm_from_mm("0.25")
    pad_connection: ZoneConnection = ZoneConnection.THERMAL
    thermal: ThermalReliefSettings = ThermalReliefSettings()
    island_policy: IslandPolicy = IslandPolicy.REMOVE_BELOW_AREA
    minimum_island_area_nm2: int | None = 10_000_000_000_000
    fill_mode: ZoneFillMode = ZoneFillMode.SOLID
    # Explicitly fingerprinted when enabled; preserve legacy zone repr/digests.
    reserve_routing: bool = field(default=False, repr=False)

    def __post_init__(self) -> None:
        object.__setattr__(self, "layers", tuple(self.layers))
        if not self.id or not self.net or not self.layers:
            raise ValueError("a copper zone requires an id, net, and layer")
        if type(self.reserve_routing) is not bool:
            raise ValueError("copper zone reserve_routing must be boolean")
        if len(set(self.layers)) != len(self.layers):
            raise ValueError("copper zone layers must be unique")
        if self.priority < 0 or self.minimum_width_nm <= 0:
            raise ValueError("copper zone priority and minimum width are invalid")
        if self.clearance_nm is not None and self.clearance_nm <= 0:
            raise ValueError("copper zone clearance must be positive")
        if self.island_policy is IslandPolicy.REMOVE_BELOW_AREA:
            if self.minimum_island_area_nm2 is None or self.minimum_island_area_nm2 <= 0:
                raise ValueError("area-based island removal requires a positive area")


@dataclass(frozen=True, slots=True)
class CopperKeepout:
    id: str
    layers: tuple[CopperLayer, ...]
    outline: PolygonWithHoles
    block_tracks: bool = True
    block_vias: bool = True
    block_pads: bool = False
    block_zones: bool = True
    block_footprints: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "layers", tuple(self.layers))
        if not self.id or not self.layers or len(set(self.layers)) != len(self.layers):
            raise ValueError("a copper keepout requires an id and unique layers")
        if not any((self.block_tracks, self.block_vias, self.block_pads, self.block_zones, self.block_footprints)):
            raise ValueError("a copper keepout must block at least one object type")


@dataclass(frozen=True, slots=True)
class ZoneFillResult:
    zone_id: str
    layer: CopperLayer
    input_digest: str
    engine_id: str
    engine_version: str
    polygons: tuple[PolygonWithHoles, ...]
    diagnostics: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "polygons", tuple(self.polygons))
        object.__setattr__(self, "diagnostics", tuple(self.diagnostics))
        if not all((self.zone_id, self.input_digest, self.engine_id, self.engine_version)):
            raise ValueError("zone fill provenance cannot be empty")


@dataclass(frozen=True, slots=True)
class StackupLayer:
    id: str
    kind: StackupLayerKind
    thickness_nm: Nanometres
    copper_layer: CopperLayer | None = None
    material: str | None = None
    relative_permittivity: Decimal | None = None
    loss_tangent: Decimal | None = None
    # Optional fabrication construction of a dielectric: "core" or "prepreg".
    dielectric_type: str | None = None

    def __post_init__(self) -> None:
        if not self.id or self.thickness_nm <= 0:
            raise ValueError("stackup layers require an id and positive thickness")
        if (self.kind is StackupLayerKind.COPPER) != (self.copper_layer is not None):
            raise ValueError("only copper stackup layers name a copper layer")
        for value in (self.relative_permittivity, self.loss_tangent):
            if value is not None and value <= 0:
                raise ValueError("dielectric properties must be positive")
        if self.dielectric_type is not None:
            if self.kind is not StackupLayerKind.DIELECTRIC:
                raise ValueError("only dielectric stackup layers have a dielectric type")
            if self.dielectric_type not in {"core", "prepreg"}:
                raise ValueError("dielectric type must be core or prepreg")


@dataclass(frozen=True, slots=True)
class ViaTechnology:
    id: str
    kind: ViaKind
    from_layer: CopperLayer
    to_layer: CopperLayer
    minimum_drill_nm: Nanometres
    minimum_annular_ring_nm: Nanometres
    maximum_aspect_ratio: Decimal

    def __post_init__(self) -> None:
        if not self.id or self.from_layer is self.to_layer:
            raise ValueError("via technology requires an id and distinct span")
        if self.minimum_drill_nm <= 0 or self.minimum_annular_ring_nm <= 0 or self.maximum_aspect_ratio <= 0:
            raise ValueError("via technology limits must be positive")


@dataclass(frozen=True, slots=True)
class Stackup:
    copper_layers: tuple[CopperLayer, ...] = (
        CopperLayer.FRONT,
        CopperLayer.BACK,
    )
    thickness_nm: Nanometres = nm_from_mm("1.6")
    physical_layers: tuple[StackupLayer, ...] = ()
    via_technologies: tuple[ViaTechnology, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "copper_layers", tuple(self.copper_layers))
        object.__setattr__(self, "physical_layers", tuple(self.physical_layers))
        object.__setattr__(self, "via_technologies", tuple(self.via_technologies))
        if len(self.copper_layers) < 1:
            raise ValueError("a stackup requires at least one copper layer")
        if len(set(self.copper_layers)) != len(self.copper_layers):
            raise ValueError("stackup copper layers must be unique")
        if len(self.copper_layers) > 2:
            expected_internal = tuple(
                layer for layer in CopperLayer
                if layer not in {CopperLayer.FRONT, CopperLayer.BACK}
            )[: len(self.copper_layers) - 2]
            if self.copper_layers != (CopperLayer.FRONT, *expected_internal, CopperLayer.BACK):
                raise ValueError("multilayer stackups use contiguous F.Cu/In1.Cu.../B.Cu order")
        if self.thickness_nm <= 0:
            raise ValueError("board thickness must be positive")
        if self.physical_layers:
            explicit = tuple(layer.copper_layer for layer in self.physical_layers if layer.kind is StackupLayerKind.COPPER)
            if explicit != self.copper_layers:
                raise ValueError("physical stackup copper order must match copper_layers")
            if sum(layer.thickness_nm for layer in self.physical_layers) != self.thickness_nm:
                raise ValueError("physical stackup thickness must equal board thickness")
        ids = [technology.id for technology in self.via_technologies]
        if len(ids) != len(set(ids)):
            raise ValueError("via technology ids must be unique")
        indexes = {layer: index for index, layer in enumerate(self.copper_layers)}
        for technology in self.via_technologies:
            if technology.from_layer not in indexes or technology.to_layer not in indexes:
                raise ValueError("via technology span must be in the stackup")
            low, high = sorted((indexes[technology.from_layer], indexes[technology.to_layer]))
            if technology.kind is ViaKind.THROUGH and (low, high) != (0, len(indexes) - 1):
                raise ValueError("through-via technology must span the complete stackup")
            if technology.kind is ViaKind.MICROVIA and high - low != 1:
                raise ValueError("microvias may span only adjacent copper layers")


@dataclass(frozen=True, slots=True)
class DesignRules:
    minimum_clearance_nm: Nanometres = nm_from_mm("0.2")
    minimum_hole_clearance_nm: Nanometres = nm_from_mm("0.25")
    minimum_track_width_nm: Nanometres = nm_from_mm("0.2")
    default_track_width_nm: Nanometres = nm_from_mm("0.25")
    default_via_size_nm: Nanometres = nm_from_mm("0.8")
    default_via_drill_nm: Nanometres = nm_from_mm("0.4")
    minimum_slot_width_nm: Nanometres = nm_from_mm('1')

    def __post_init__(self) -> None:
        values = (
            self.minimum_clearance_nm,
            self.minimum_hole_clearance_nm,
            self.minimum_track_width_nm,
            self.default_track_width_nm,
            self.default_via_size_nm,
            self.default_via_drill_nm,
            self.minimum_slot_width_nm,
        )
        if any(value <= 0 for value in values):
            raise ValueError("physical design rules must be positive")
        if self.minimum_track_width_nm > self.default_track_width_nm:
            raise ValueError("minimum track width cannot exceed default track width")
        if self.default_via_drill_nm >= self.default_via_size_nm:
            raise ValueError("default via drill must be smaller than via size")


@dataclass(frozen=True, slots=True)
class FootprintPad:
    number: str
    position: Point
    size: Size
    kind: PadKind = PadKind.SMD
    shape: PadShape = PadShape.ROUNDRECT
    rotation_degrees: Decimal | int | float | str = Decimal(0)
    drill: Size | None = None
    roundrect_ratio_ppm: int = 250_000
    has_solder_mask: bool = True
    has_solder_paste: bool = True
    zone_connection: ZoneConnection | None = None
    heatsink: bool = False
    remove_unused_layers: bool = False
    # KiCad `connect` pads are bare, mask-opened contacts (e.g. pogo pads).
    connector_contact: bool = False

    def __post_init__(self) -> None:
        if not self.number and self.kind not in {
            PadKind.NON_PLATED_THROUGH_HOLE,
            PadKind.APERTURE,
        }:
            raise ValueError("electrical footprint pad number cannot be empty")
        object.__setattr__(
            self,
            "rotation_degrees",
            Decimal(str(self.rotation_degrees)) % Decimal(360),
        )
        if self.kind in {PadKind.SMD, PadKind.APERTURE} and self.drill is not None:
            raise ValueError("SMD and aperture pads cannot have a drill")
        if self.connector_contact and (self.kind is not PadKind.SMD or self.has_solder_paste):
            raise ValueError("connector contacts must be non-pasted surface pads")
        if self.kind in {PadKind.THROUGH_HOLE, PadKind.NON_PLATED_THROUGH_HOLE}:
            if self.drill is None:
                raise ValueError("through-hole pads require a drill")
            if self.kind is PadKind.THROUGH_HOLE and (
                self.drill.width_nm >= self.size.width_nm
                or self.drill.height_nm >= self.size.height_nm
            ):
                raise ValueError("pad drill must be smaller than pad size")
            if self.kind is PadKind.NON_PLATED_THROUGH_HOLE and (
                self.drill.width_nm > self.size.width_nm
                or self.drill.height_nm > self.size.height_nm
            ):
                raise ValueError("non-plated drill cannot exceed its hole envelope")
        if not 0 <= self.roundrect_ratio_ppm <= 500_000:
            raise ValueError("roundrect pad ratio must be between 0 and 0.5")


@dataclass(frozen=True, slots=True)
class FootprintLine:
    start: Point
    end: Point
    width_nm: Nanometres
    layer: FootprintLayer

    def __post_init__(self) -> None:
        if self.start == self.end:
            raise ValueError("footprint line cannot have zero length")
        if self.width_nm < 0:
            raise ValueError("footprint line width cannot be negative")


@dataclass(frozen=True, slots=True)
class FootprintRectangle:
    start: Point
    end: Point
    width_nm: Nanometres
    layer: FootprintLayer
    filled: bool = False

    def __post_init__(self) -> None:
        if self.start == self.end:
            raise ValueError("footprint rectangle cannot have zero size")
        if self.width_nm < 0:
            raise ValueError("footprint rectangle width cannot be negative")


@dataclass(frozen=True, slots=True)
class FootprintCircle:
    center: Point
    end: Point
    width_nm: Nanometres
    layer: FootprintLayer
    filled: bool = False

    def __post_init__(self) -> None:
        if self.center == self.end:
            raise ValueError("footprint circle must have a positive radius")
        if self.width_nm < 0:
            raise ValueError("footprint circle width cannot be negative")


@dataclass(frozen=True, slots=True)
class FootprintArc:
    start: Point
    midpoint: Point
    end: Point
    width_nm: Nanometres
    layer: FootprintLayer

    def __post_init__(self) -> None:
        if len({self.start, self.midpoint, self.end}) < 3:
            raise ValueError("footprint arc requires three distinct points")
        if self.width_nm < 0:
            raise ValueError("footprint arc width cannot be negative")


@dataclass(frozen=True, slots=True)
class FootprintPolygon:
    points: tuple[Point, ...]
    width_nm: Nanometres
    layer: FootprintLayer
    filled: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "points", tuple(self.points))
        if len(self.points) < 3:
            raise ValueError("footprint polygon requires at least three points")
        if self.width_nm < 0:
            raise ValueError("footprint polygon width cannot be negative")


FootprintGraphic = (
    FootprintLine
    | FootprintRectangle
    | FootprintCircle
    | FootprintArc
    | FootprintPolygon
)


@dataclass(frozen=True, slots=True)
class PhysicalFootprint:
    name: str
    pads: tuple[FootprintPad, ...]
    body_size: Size
    source_library_id: str | None = None
    graphics: tuple[FootprintGraphic, ...] = ()
    # Footprint-local copper keepouts move and mirror with each placement.
    keepouts: tuple[CopperKeepout, ...] = ()
    metadata: Mapping[str, str] = field(default_factory=dict)
    courtyard: tuple[Point, ...] = ()
    height_nm: Nanometres | None = None
    clearance_nm: Nanometres | None = None
    exclude_from_bom: bool = False
    exclude_from_pos_files: bool = False
    internal_pad_groups: tuple[InternalPadGroup, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
        object.__setattr__(self, "internal_pad_groups", validate_internal_pad_groups(
            self.internal_pad_groups, {pad.number for pad in self.pads if pad.kind not in {
                PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}}))
        for group in self.internal_pad_groups:
            if sum(p.number in group.numbers for p in self.pads
                   if p.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}) < 2:
                raise ValueError("internal pad group requires at least two electrical lands")
        object.__setattr__(self, "graphics", tuple(self.graphics))
        object.__setattr__(self, "keepouts", tuple(self.keepouts))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))
        object.__setattr__(self, "courtyard", tuple(self.courtyard))
        if not self.name:
            raise ValueError("footprint name cannot be empty")
        if self.courtyard and len(self.courtyard) < 3:
            raise ValueError("a footprint courtyard requires at least three points")
        if self.height_nm is not None and self.height_nm <= 0:
            raise ValueError("footprint height must be positive")
        if self.clearance_nm is not None and self.clearance_nm < 0:
            raise ValueError("footprint clearance cannot be negative")
        if len({item.id for item in self.keepouts}) != len(self.keepouts):
            raise ValueError("footprint keepout ids must be unique")


@dataclass(frozen=True, slots=True)
class Placement:
    reference: str
    footprint: str
    position: Point
    rotation_degrees: Decimal | int | float | str = Decimal(0)
    side: BoardSide = BoardSide.FRONT
    value: str = ""
    source_path: str | None = None

    def __post_init__(self) -> None:
        if not self.reference:
            raise ValueError("placement reference cannot be empty")
        object.__setattr__(
            self,
            "rotation_degrees",
            Decimal(str(self.rotation_degrees)) % Decimal(360),
        )


@dataclass(frozen=True, slots=True, order=True)
class PadReference:
    component: str
    pad: str


@dataclass(frozen=True, slots=True)
class PhysicalNet:
    name: str
    pads: tuple[PadReference, ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
        if not self.name:
            raise ValueError("physical net name cannot be empty")


@dataclass(frozen=True, slots=True)
class PhysicalPowerDomain:
    """Derived rail membership for physical planning; never new connectivity.

    Identity is the electrical net, not its voltage or a device-local domain
    name. Sources are known physical supply outputs, not guessed connectors.
    Members include passive rail terminals, so decouplers share the objective.
    """

    net: str
    members: tuple[PadReference, ...]
    sources: tuple[PadReference, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "members", tuple(self.members))
        object.__setattr__(self, "sources", tuple(self.sources))
        if not self.net or not self.members:
            raise ValueError("physical power domain requires a net and members")
        if len(set(self.members)) != len(self.members) or len(set(self.sources)) != len(self.sources):
            raise ValueError("physical power domain terminals must be unique")
        if not set(self.sources) <= set(self.members):
            raise ValueError("physical power domain sources must be members")


@dataclass(frozen=True, slots=True, order=True)
class PlacementTarget:
    reference: str
    pad: str | None = None


@dataclass(frozen=True, slots=True)
class PlacementRegion:
    name: str
    outline: BoardOutline
    side: BoardSide | None = None

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("placement region name cannot be empty")
        if self.outline.cutouts:
            raise ValueError("placement region outlines do not support cutouts")
        if self.outline.circular_boundary:
            raise ValueError("placement region outlines do not support curved boundaries")


@dataclass(frozen=True, slots=True)
class PlacementKeepout:
    name: str
    outline: BoardOutline
    side: BoardSide | None = None
    maximum_component_height_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("placement keepout name cannot be empty")
        if self.outline.cutouts:
            raise ValueError("placement keepout outlines do not support cutouts")
        if self.outline.circular_boundary:
            raise ValueError("placement keepout outlines do not support curved boundaries")
        if (
            self.maximum_component_height_nm is not None
            and self.maximum_component_height_nm < 0
        ):
            raise ValueError("keepout maximum component height cannot be negative")


@dataclass(frozen=True, slots=True)
class ComponentPlacementRule:
    reference: str
    region: str | None = None
    allowed_orientations: tuple[Decimal | int | float | str, ...] = (
        Decimal(0),
        Decimal(90),
        Decimal(180),
        Decimal(270),
    )
    fixed_position: Point | None = None
    fixed_rotation_degrees: Decimal | int | float | str | None = None
    side: BoardSide | None = None
    priority: int = 0
    # Explicit physical edge-mounted exception, never a copper DRC waiver.
    edge_clearance_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        if not self.reference:
            raise ValueError("component placement rule requires a reference")
        if self.edge_clearance_nm is not None and self.edge_clearance_nm <= 0:
            raise ValueError("component edge clearance must be positive")
        orientations = tuple(
            sorted({Decimal(str(value)) % Decimal(360) for value in self.allowed_orientations})
        )
        if not orientations:
            raise ValueError("component placement rule requires a legal orientation")
        object.__setattr__(self, "allowed_orientations", orientations)
        if self.fixed_rotation_degrees is not None:
            rotation = Decimal(str(self.fixed_rotation_degrees)) % Decimal(360)
            if rotation not in orientations:
                raise ValueError("fixed rotation must be one of the allowed orientations")
            object.__setattr__(self, "fixed_rotation_degrees", rotation)


@dataclass(frozen=True, slots=True)
class RelativePlacementRule:
    kind: RelativePlacementKind
    targets: tuple[PlacementTarget, ...]
    distance_nm: Nanometres | None = None
    axis: AlignmentAxis | None = None
    tolerance_nm: Nanometres = 0
    weight: int = 1

    def __post_init__(self) -> None:
        object.__setattr__(self, "targets", tuple(self.targets))
        if len(self.targets) < 2:
            raise ValueError("relative placement rule requires at least two targets")
        if self.kind in {
            RelativePlacementKind.MAX_DISTANCE,
            RelativePlacementKind.MIN_DISTANCE,
        } and (self.distance_nm is None or self.distance_nm <= 0):
            raise ValueError("distance placement rule requires a positive distance")
        if self.kind is RelativePlacementKind.ALIGN and self.axis is None:
            raise ValueError("alignment placement rule requires an axis")
        if self.tolerance_nm < 0 or self.weight <= 0:
            raise ValueError("relative placement tolerance and weight are invalid")


@dataclass(frozen=True, slots=True)
class PlacementGroup:
    name: str
    references: tuple[str, ...]
    anchor: str | None = None
    priority: int = 0
    source: str = "explicit"

    def __post_init__(self) -> None:
        object.__setattr__(self, "references", tuple(dict.fromkeys(self.references)))
        if not self.name or not self.references:
            raise ValueError("placement group requires a name and members")
        if self.anchor is not None and self.anchor not in self.references:
            raise ValueError("placement group anchor must be a member")


@dataclass(frozen=True, slots=True)
class RigidPlacementMember:
    """A footprint pose in a reference template's anchor-pad frame."""

    reference: str
    footprint: str
    footprint_digest: str
    position: Point
    rotation_degrees: Decimal | int | float | str = Decimal(0)

    def __post_init__(self) -> None:
        if not self.reference or not self.footprint:
            raise ValueError("rigid member requires a reference and footprint")
        if len(self.footprint_digest) != 64 or any(
            char not in "0123456789abcdef" for char in self.footprint_digest
        ):
            raise ValueError("rigid member requires a SHA-256 footprint digest")
        rotation = Decimal(str(self.rotation_degrees))
        if not rotation.is_finite():
            raise ValueError("rigid member rotation must be finite")
        object.__setattr__(self, "rotation_degrees", (rotation % Decimal(360) + Decimal(360)) % Decimal(360))


@dataclass(frozen=True, slots=True)
class RigidPlacementCluster:
    """Hard physical macro, not a soft proximity group or electrical hierarchy.

    All poses and keepouts are local to the anchor pad (or footprint origin).
    Initial support is same-side FRONT placement without mirroring. Source is
    an evidence locator, not a claim that the template is RF-qualified.
    """

    name: str
    anchor: PlacementTarget
    members: tuple[RigidPlacementMember, ...]
    source: str
    allowed_rotations: tuple[Decimal | int | float | str, ...] = (0, 90, 180, 270)
    keepouts: tuple[CopperKeepout, ...] = ()
    # Additional courtyard-to-courtyard gap inside the audited macro only.
    # None retains the planner's ordinary gap; copper DRC is never changed.
    internal_clearance_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "members", tuple(self.members))
        object.__setattr__(self, "keepouts", tuple(self.keepouts))
        references = [member.reference for member in self.members]
        if not self.name or not self.source or len(references) < 2:
            raise ValueError("rigid cluster requires a name, evidence source and two members")
        if len(set(references)) != len(references) or self.anchor.reference not in references:
            raise ValueError("rigid cluster requires unique members including its anchor")
        rotations = tuple(Decimal(str(value)) for value in self.allowed_rotations)
        if not rotations or any(not value.is_finite() for value in rotations):
            raise ValueError("rigid cluster requires finite allowed rotations")
        object.__setattr__(self, "allowed_rotations", tuple(sorted({
            (value % Decimal(360) + Decimal(360)) % Decimal(360) for value in rotations
        })))
        if len({item.id for item in self.keepouts}) != len(self.keepouts):
            raise ValueError("rigid cluster keepout ids must be unique")
        if self.internal_clearance_nm is not None and self.internal_clearance_nm < 0:
            raise ValueError("rigid cluster internal courtyard clearance cannot be negative")


@dataclass(frozen=True, slots=True)
class NetRoutingRule:
    """Physical routing intent for one net.

    Global routing uses these values as demand and eligibility constraints;
    detailed routing and DRC use the same values for exact copper geometry.
    """

    net: str
    kind: RouteKind = RouteKind.GENERAL
    priority: int = 0
    width_nm: Nanometres | None = None
    clearance_nm: Nanometres | None = None
    allowed_layers: tuple[CopperLayer, ...] = ()
    max_vias: int | None = None
    max_length_nm: Nanometres | None = None
    differential_partner: str | None = None
    pair_gap_nm: Nanometres | None = None
    max_skew_nm: Nanometres | None = None
    topology: str = "point_to_point"
    target_impedance_ohms: int | None = None
    maximum_uncoupled_length_nm: Nanometres | None = None
    maximum_stub_length_nm: Nanometres | None = None
    tuning_amplitude_limit_nm: Nanometres | None = None
    require_return_vias: bool = False
    # Placement keeps other components out of the pair's terminal corridor.
    reserve_corridor: bool = False
    return_via_net: str | None = None
    maximum_return_via_distance_nm: Nanometres | None = None
    impedance_evidence_digest: str | None = None
    return_via_policy: ReturnViaPolicy = ReturnViaPolicy.ALWAYS
    shared_reference_layer: CopperLayer | None = None
    # Signal-integrity screening intent (D-PHY plan L2/L5). A differential
    # rule's ``target_impedance_ohms`` is its differential target; this is the
    # single-ended target of each member (or of a single-ended net).
    target_single_ended_ohms: int | None = None
    # None means the documented default of 10 percent; see
    # ``effective_impedance_tolerance_percent``.
    impedance_tolerance_percent: Decimal | None = None
    layer_group: str | None = None
    # Breakout-region relaxations (plan L6). The router, its clearance index
    # and physical DRC apply them near terminal lands (``pcbir.breakout``).
    breakout_length_nm: Nanometres | None = None
    breakout_width_nm: Nanometres | None = None
    breakout_gap_nm: Nanometres | None = None
    breakout_clearance_nm: Nanometres | None = None
    # Length-match tuning geometry (plan R10). ``tuning_spacing_nm`` is the
    # least edge gap between adjacent serpentine legs; None means the default
    # (the larger of 3 x width and the clearance).
    tuning_style: TuningStyle = TuningStyle.BUMPS
    tuning_spacing_nm: Nanometres | None = None

    @property
    def effective_impedance_tolerance_percent(self) -> Decimal:
        return Decimal(10) if self.impedance_tolerance_percent is None else self.impedance_tolerance_percent

    def __post_init__(self) -> None:
        object.__setattr__(self, "allowed_layers", tuple(self.allowed_layers))
        object.__setattr__(self, "return_via_policy", ReturnViaPolicy(self.return_via_policy))
        object.__setattr__(self, "tuning_style", TuningStyle(self.tuning_style))
        if self.shared_reference_layer is not None:
            object.__setattr__(self, "shared_reference_layer", CopperLayer(self.shared_reference_layer))
        if self.return_via_policy is ReturnViaPolicy.REFERENCE_CHANGE:
            if not self.require_return_vias or self.shared_reference_layer is None:
                raise ValueError("reference_change requires return vias and an explicit shared reference layer")
        elif self.shared_reference_layer is not None:
            raise ValueError("shared reference layer requires reference_change return-via policy")
        if not self.net:
            raise ValueError("routing rule requires a net")
        if self.priority < 0:
            raise ValueError("routing priority cannot be negative")
        for name, value in (
            ("width", self.width_nm),
            ("clearance", self.clearance_nm),
            ("maximum length", self.max_length_nm),
            ("pair gap", self.pair_gap_nm),
            ("maximum skew", self.max_skew_nm),
            ("maximum uncoupled length", self.maximum_uncoupled_length_nm),
            ("maximum stub length", self.maximum_stub_length_nm),
            ("tuning amplitude limit", self.tuning_amplitude_limit_nm),
            ("tuning spacing", self.tuning_spacing_nm),
            ("maximum return via distance", self.maximum_return_via_distance_nm),
        ):
            if value is not None and value <= 0:
                raise ValueError(f"routing {name} must be positive")
        if self.max_vias is not None and self.max_vias < 0:
            raise ValueError("routing maximum via count cannot be negative")
        if self.tuning_spacing_nm is not None and self.tuning_style is not TuningStyle.SERPENTINE:
            raise ValueError('routing tuning spacing requires tuning_style "serpentine"')
        if self.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}:
            if self.differential_partner is None or self.pair_gap_nm is None:
                raise ValueError(
                    "differential routing requires a partner net and pair gap"
                )
            if self.differential_partner == self.net:
                raise ValueError("a differential net cannot partner with itself")
        if self.target_impedance_ohms is not None and self.target_impedance_ohms <= 0:
            raise ValueError("target impedance must be positive")
        if self.require_return_vias and (not self.return_via_net or self.maximum_return_via_distance_nm is None):
            raise ValueError("return-via routing requires a net and maximum distance")
        if self.impedance_evidence_digest is not None and len(self.impedance_evidence_digest) != 64:
            raise ValueError("impedance evidence digest must be SHA-256")
        if not self.topology:
            raise ValueError("routing topology cannot be empty")
        self._validate_signal_integrity_intent()

    def _validate_signal_integrity_intent(self) -> None:
        if self.target_single_ended_ohms is not None:
            if isinstance(self.target_single_ended_ohms, bool) or not isinstance(self.target_single_ended_ohms, int):
                raise ValueError("target single-ended impedance must be an integer")
            if self.target_single_ended_ohms <= 0:
                raise ValueError("target single-ended impedance must be positive")
            if (self.kind not in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}
                    and self.target_impedance_ohms is not None
                    and self.target_impedance_ohms != self.target_single_ended_ohms):
                raise ValueError("a single-ended rule's target_impedance_ohms and "
                                 "target_single_ended_ohms must agree")
        if self.impedance_tolerance_percent is not None:
            tolerance = self.impedance_tolerance_percent
            if isinstance(tolerance, bool) or not isinstance(tolerance, (Decimal, int, float)):
                raise ValueError("impedance tolerance must be a numeric percentage")
            tolerance = Decimal(str(tolerance))
            if not tolerance.is_finite():
                raise ValueError("impedance tolerance must be a finite percentage")
            object.__setattr__(self, "impedance_tolerance_percent", tolerance)
            if not Decimal(0) < tolerance < Decimal(100):
                raise ValueError("impedance tolerance must be greater than 0 and below 100 percent")
            if self.target_impedance_ohms is None and self.target_single_ended_ohms is None:
                raise ValueError("impedance tolerance requires target_impedance_ohms or target_single_ended_ohms")
        if self.layer_group is not None and (not isinstance(self.layer_group, str) or not self.layer_group.strip()):
            raise ValueError("routing layer group must be a nonempty name")
        breakout = {
            "breakout width": self.breakout_width_nm,
            "breakout gap": self.breakout_gap_nm,
            "breakout clearance": self.breakout_clearance_nm,
        }
        for name, value in (("breakout length", self.breakout_length_nm), *breakout.items()):
            if value is not None and value <= 0:
                raise ValueError(f"routing {name} must be positive")
        relaxations = [name for name, value in breakout.items() if value is not None]
        if relaxations and self.breakout_length_nm is None:
            raise ValueError("breakout width, gap or clearance requires a positive breakout_length")
        if self.breakout_length_nm is not None and not relaxations:
            raise ValueError("breakout_length requires breakout_width, breakout_gap or breakout_clearance")
        # Breakout values relax the declared profile; they never tighten it.
        if (self.breakout_width_nm is not None and self.width_nm is not None
                and self.breakout_width_nm > self.width_nm):
            raise ValueError("breakout width may only relax (not exceed) the routing width")
        if self.breakout_gap_nm is not None:
            if self.pair_gap_nm is None:
                raise ValueError("breakout gap requires a differential pair gap")
            if self.breakout_gap_nm > self.pair_gap_nm:
                raise ValueError("breakout gap may only relax (not exceed) the pair gap")
        if (self.breakout_clearance_nm is not None and self.clearance_nm is not None
                and self.breakout_clearance_nm > self.clearance_nm):
            raise ValueError("breakout clearance may only relax (not exceed) the routing clearance")


@dataclass(frozen=True, slots=True)
class NetMatchGroup:
    """Nets whose routed lengths must agree within ``max_skew_nm`` (plan L4).

    Lengths are total routed track length per net; the group skew is the
    longest minus the shortest member. Verification applies only once every
    member net is connected.
    """

    id: str
    nets: tuple[str, ...]
    max_skew_nm: Nanometres

    def __post_init__(self) -> None:
        object.__setattr__(self, "nets", tuple(self.nets))
        if not self.id:
            raise ValueError("length-match group requires an id")
        if len(self.nets) < 2:
            raise ValueError(f"length-match group {self.id!r} requires at least two nets")
        if len(set(self.nets)) != len(self.nets) or any(not net for net in self.nets):
            raise ValueError(f"length-match group {self.id!r} nets must be unique and nonempty")
        if self.max_skew_nm <= 0:
            raise ValueError(f"length-match group {self.id!r} maximum skew must be positive")


@dataclass(frozen=True, slots=True)
class ComponentHoleClearance:
    """A documented drill clearance scoped to one component's own footprint.

    ``clearance_nm`` applies only between the copper pads and the non-plated
    holes of the same footprint instance. Every other pair (tracks, vias,
    other components' pads, other holes) keeps the board's
    ``minimum_hole_clearance_nm``, which this value may only relax.
    """

    reference: str
    clearance_nm: Nanometres
    reason: str

    def __post_init__(self) -> None:
        if not self.reference:
            raise ValueError("component hole clearance requires a component reference")
        if self.clearance_nm <= 0:
            raise ValueError(f"hole clearance for {self.reference!r} must be positive")
        if not self.reason.strip():
            raise ValueError(f"hole clearance for {self.reference!r} requires a reason")


@dataclass(frozen=True, slots=True)
class PadViaInPadRule:
    """Explicit pad-scoped permission, not a global pad-overlap exemption.

    ``rows`` and ``columns`` turn the permission into a required centred
    array of filled-capped vias (see ``pad_via_arrays``); ``pitch_nm``
    overrides its default even spread.
    """

    pad: PadReference
    process: str = "filled-capped"
    rows: int | None = None
    columns: int | None = None
    pitch_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        if self.process != "filled-capped":
            raise ValueError("via-in-pad supports only the filled-capped process")
        if (self.rows is None) != (self.columns is None):
            raise ValueError("via-in-pad array rows and columns must be given together")
        if self.rows is not None and any(
                type(count) is not int or count <= 0 for count in (self.rows, self.columns)):
            raise ValueError("via-in-pad array rows and columns must be positive integers")
        if self.pitch_nm is not None and (self.rows is None or self.pitch_nm <= 0):
            raise ValueError("via-in-pad array pitch must be positive and requires rows and columns")

    def __repr__(self) -> str:
        # A permission-only rule keeps its historical repr, and with it the
        # placement fingerprints of boards that declare one.
        array = ("" if self.rows is None else
                 f", rows={self.rows!r}, columns={self.columns!r}, pitch_nm={self.pitch_nm!r}")
        return f"PadViaInPadRule(pad={self.pad!r}, process={self.process!r}{array})"


@dataclass(frozen=True, slots=True)
class TrackSegment:
    net: str
    start: Point
    end: Point
    width_nm: Nanometres
    layer: CopperLayer

    def __post_init__(self) -> None:
        if self.start == self.end:
            raise ValueError("track segment cannot have zero length")
        if self.width_nm <= 0:
            raise ValueError("track width must be positive")


@dataclass(frozen=True, slots=True)
class Via:
    net: str
    position: Point
    size_nm: Nanometres
    drill_nm: Nanometres
    from_layer: CopperLayer = CopperLayer.FRONT
    to_layer: CopperLayer = CopperLayer.BACK
    technology: str | None = None
    finish: str = "standard"

    def __post_init__(self) -> None:
        if self.size_nm <= 0 or self.drill_nm <= 0:
            raise ValueError("via size and drill must be positive")
        if self.drill_nm >= self.size_nm:
            raise ValueError("via drill must be smaller than via size")
        if self.from_layer == self.to_layer:
            raise ValueError("a via must connect distinct copper layers")
        if self.finish not in {"standard", "filled-capped"}:
            raise ValueError("unsupported via finish")


@dataclass(frozen=True, slots=True)
class MacroPort:
    """Explicit physical access; electrical connectivity remains in Board.nets."""

    name: str
    net: str
    position: Point
    layer: CopperLayer
    pads: tuple[PadReference, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
        if not self.name or not self.net:
            raise ValueError("macro port requires a name and net")


@dataclass(frozen=True, slots=True)
class MacroPadBinding:
    pad: PadReference
    net: str


@dataclass(frozen=True, slots=True)
class PhysicalHardMacro:
    """Local immutable copper attached to an identity-bound rigid cluster.

    Coordinates use the cluster's anchor frame. Protected regions reserve
    *all* new track/via access, including same-net shortcuts. Ports therefore
    sit outside those regions; owner copper alone may cross the boundary.
    Zone exclusion is separate, explicit CopperKeepout intent on the cluster.
    This experimental subset does not yet import local pours or mirrored poses.
    """

    cluster: str
    asset_sha256: str
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...] = ()
    ports: tuple[MacroPort, ...] = ()
    protected_regions: tuple[CopperKeepout, ...] = ()
    required_layers: tuple[CopperLayer, ...] = ()
    pad_bindings: tuple[MacroPadBinding, ...] = ()
    isolated_pads: tuple[PadReference, ...] = ()

    def __post_init__(self) -> None:
        for name in ("tracks", "vias", "ports", "protected_regions", "required_layers", "pad_bindings", "isolated_pads"):
            object.__setattr__(self, name, tuple(getattr(self, name)))
        if not self.cluster or len(self.asset_sha256) != 64 or any(
            c not in "0123456789abcdef" for c in self.asset_sha256
        ):
            raise ValueError("hard macro requires a cluster and SHA-256 asset identity")
        if not self.tracks and not self.vias:
            raise ValueError("hard macro requires explicit copper")
        if len({p.name for p in self.ports}) != len(self.ports):
            raise ValueError("hard macro port names must be unique")
        if len({r.id for r in self.protected_regions}) != len(self.protected_regions):
            raise ValueError("hard macro protected region IDs must be unique")
        if any(r.outline.holes or not r.block_tracks or not r.block_vias
               for r in self.protected_regions):
            raise ValueError("macro access reservations require solid track/via-blocking polygons")


@dataclass(frozen=True, slots=True)
class PhysicalBoard:
    """A complete, backend-neutral physical realization of one PCB."""

    name: str
    outline: BoardOutline
    footprints: Mapping[str, PhysicalFootprint]
    placements: tuple[Placement, ...]
    nets: tuple[PhysicalNet, ...]
    stackup: Stackup = Stackup()
    rules: DesignRules = DesignRules()
    tracks: tuple[TrackSegment, ...] = ()
    vias: tuple[Via, ...] = ()
    metadata: Mapping[str, str] = field(default_factory=dict)
    regions: tuple[PlacementRegion, ...] = ()
    keepouts: tuple[PlacementKeepout, ...] = ()
    placement_rules: tuple[ComponentPlacementRule, ...] = ()
    relative_rules: tuple[RelativePlacementRule, ...] = ()
    placement_groups: tuple[PlacementGroup, ...] = ()
    power_domains: tuple[PhysicalPowerDomain, ...] = ()
    net_routing_rules: tuple[NetRoutingRule, ...] = ()
    zones: tuple[CopperZone, ...] = ()
    copper_keepouts: tuple[CopperKeepout, ...] = ()
    zone_fills: tuple[ZoneFillResult, ...] = ()
    rigid_clusters: tuple[RigidPlacementCluster, ...] = ()
    hard_macros: tuple[PhysicalHardMacro, ...] = ()
    materialized_macros: tuple[str, ...] = ()
    via_in_pad_rules: tuple[PadViaInPadRule, ...] = ()
    mechanical_holes: tuple[MechanicalHole, ...] = ()
    datums: tuple[BoardDatum, ...] = ()
    boundary_edges: tuple[BoardEdge, ...] = ()
    attachments: tuple[PhysicalAttachment, ...] = ()
    body_overhangs: tuple[BodyOverhang, ...] = ()
    component_heights: tuple[ComponentHeight, ...] = ()
    assembly_envelopes: tuple[AssemblyEnvelope, ...] = ()
    assembly_access: tuple[AssemblyAccess, ...] = ()
    mechanical_slots: tuple[MechanicalSlot,...] = ()
    mechanical_references: tuple["MechanicalReference",...] = ()
    match_groups: tuple[NetMatchGroup, ...] = ()
    component_hole_clearances: tuple[ComponentHoleClearance, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "match_groups", tuple(self.match_groups))
        object.__setattr__(self, "component_hole_clearances", tuple(self.component_hole_clearances))
        object.__setattr__(self, "footprints", MappingProxyType(dict(self.footprints)))
        object.__setattr__(self, "placements", tuple(self.placements))
        object.__setattr__(self, "nets", tuple(self.nets))
        object.__setattr__(self, "tracks", tuple(self.tracks))
        object.__setattr__(self, "vias", tuple(self.vias))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))
        object.__setattr__(self, "regions", tuple(self.regions))
        object.__setattr__(self, "keepouts", tuple(self.keepouts))
        object.__setattr__(self, "placement_rules", tuple(self.placement_rules))
        object.__setattr__(self, "relative_rules", tuple(self.relative_rules))
        object.__setattr__(self, "placement_groups", tuple(self.placement_groups))
        object.__setattr__(self, "power_domains", tuple(self.power_domains))
        object.__setattr__(self, "net_routing_rules", tuple(self.net_routing_rules))
        object.__setattr__(self, "zones", tuple(self.zones))
        object.__setattr__(self, "copper_keepouts", tuple(self.copper_keepouts))
        object.__setattr__(self, "zone_fills", tuple(self.zone_fills))
        object.__setattr__(self, "rigid_clusters", tuple(self.rigid_clusters))
        object.__setattr__(self, "hard_macros", tuple(self.hard_macros))
        object.__setattr__(self, "materialized_macros", tuple(self.materialized_macros))
        object.__setattr__(self, "via_in_pad_rules", tuple(self.via_in_pad_rules))
        object.__setattr__(self, "mechanical_holes", tuple(self.mechanical_holes))
        for field in ("datums", "boundary_edges", "attachments", "body_overhangs", "component_heights", "assembly_envelopes", "assembly_access", "mechanical_slots", "mechanical_references"):
            object.__setattr__(self, field, tuple(getattr(self, field)))
            identities = [item.id for item in getattr(self, field)]
            if len(set(identities)) != len(identities):
                raise ValueError(f"duplicate {field} identity")
        from .mechanical import validate_mechanical_holes
        from .mechanical_references import MechanicalReference, validate_reference_inventory
        if any(not isinstance(r,MechanicalReference) for r in self.mechanical_references):
            raise ValueError("physical references require typed mechanical guides")
        validate_reference_inventory(self.mechanical_references)
        validate_mechanical_holes(self)
        self._validate_references()

    def _validate_references(self) -> None:
        placement_refs = [placement.reference for placement in self.placements]
        if len(placement_refs) != len(set(placement_refs)):
            raise ValueError("physical placement references must be unique")
        datum_by_id = {d.id:d for d in self.datums}
        if set(datum_by_id) & {e.id for e in self.boundary_edges}:
            raise ValueError("datum and edge names must not collide")
        for datum in self.datums:
            current, seen = datum, set()
            while current.relative_to is not None:
                if current.id in seen:
                    raise ValueError("cyclic physical datum dependency")
                seen.add(current.id)
                if current.relative_to not in datum_by_id:
                    raise ValueError("physical datum references unknown parent")
                parent = datum_by_id[current.relative_to]
                if current.position != Point(parent.position.x_nm+current.offset.x_nm,parent.position.y_nm+current.offset.y_nm):
                    raise ValueError("physical datum position does not match its dependency")
                current = parent
        from .mechanical import boundary_line_pairs
        pairs = boundary_line_pairs(self.outline)
        if self.boundary_edges and self.outline.circular_boundary:
            raise ValueError("circular outlines do not expose sampled straight edge IDs")
        for edge in self.boundary_edges:
            if (edge.start,edge.end) not in pairs and (edge.end,edge.start) not in pairs:
                raise ValueError("named edge is not an actual board boundary segment")
        attached = set()
        for attachment in self.attachments:
            if attachment.reference not in placement_refs or attachment.reference in attached:
                raise ValueError("attachment requires a unique known component")
            attached.add(attachment.reference)
            if attachment.target is not None and attachment.target not in set(datum_by_id) | {e.id for e in self.boundary_edges}:
                raise ValueError("attachment references unknown mechanical target")
        for field in ("body_overhangs","component_heights","assembly_access"):
            owned=set()
            for feature in getattr(self,field):
                if feature.reference not in placement_refs or feature.reference in owned:
                    raise ValueError(f"{field} requires unique known component owners")
                owned.add(feature.reference)
        from .mechanical_assembly import expanded_body_outline
        for allowance in self.body_overhangs:
            expanded_body_outline(self.outline,self.boundary_edges,allowance)
        net_names = [net.name for net in self.nets]
        if len(net_names) != len(set(net_names)):
            raise ValueError("physical net names must be unique")
        zone_ids = [zone.id for zone in self.zones]
        if len(zone_ids) != len(set(zone_ids)):
            raise ValueError("copper zone ids must be unique")
        keepout_ids = [keepout.id for keepout in self.copper_keepouts]
        if len(keepout_ids) != len(set(keepout_ids)):
            raise ValueError("copper keepout ids must be unique")
        stackup_layers = set(self.stackup.copper_layers)
        for zone in self.zones:
            if zone.net not in net_names:
                raise ValueError(f"copper zone {zone.id!r} references unknown net {zone.net!r}")
            if not set(zone.layers).issubset(stackup_layers):
                raise ValueError(f"copper zone {zone.id!r} references a layer outside the stackup")
        from .zone_geometry import validate_routing_reservations
        validate_routing_reservations(self.zones)
        for keepout in self.copper_keepouts:
            if not set(keepout.layers).issubset(stackup_layers):
                raise ValueError(f"copper keepout {keepout.id!r} references a layer outside the stackup")
        fill_keys: set[tuple[str, CopperLayer]] = set()
        zone_by_id = {zone.id: zone for zone in self.zones}
        for fill in self.zone_fills:
            key = (fill.zone_id, fill.layer)
            if key in fill_keys:
                raise ValueError("zone fill results must be unique by zone and layer")
            fill_keys.add(key)
            zone = zone_by_id.get(fill.zone_id)
            if zone is None or fill.layer not in zone.layers:
                raise ValueError("zone fill result does not match a declared zone layer")

        pads_by_component: dict[str, dict[str, FootprintPad]] = {}
        for placement in self.placements:
            try:
                footprint = self.footprints[placement.footprint]
            except KeyError as exc:
                raise ValueError(
                    f"placement {placement.reference!r} uses unknown footprint "
                    f"{placement.footprint!r}"
                ) from exc
            pads_by_component[placement.reference] = {
                pad.number: pad for pad in footprint.pads
            }

        assigned_pads: dict[PadReference, str] = {}
        for net in self.nets:
            for pad_ref in net.pads:
                if pad_ref.component not in pads_by_component:
                    raise ValueError(
                        f"net {net.name!r} references unknown placement "
                        f"{pad_ref.component!r}"
                    )
                if pad_ref.pad not in pads_by_component[pad_ref.component]:
                    raise ValueError(
                        f"net {net.name!r} references unknown pad "
                        f"{pad_ref.component}.{pad_ref.pad}"
                    )
                physical_pad = pads_by_component[pad_ref.component][pad_ref.pad]
                if physical_pad.kind in {
                    PadKind.NON_PLATED_THROUGH_HOLE,
                    PadKind.APERTURE,
                }:
                    raise ValueError(
                        f"non-electrical pad {pad_ref.component}.{pad_ref.pad} cannot "
                        "belong to an electrical net"
                    )
                previous = assigned_pads.get(pad_ref)
                if previous is not None:
                    raise ValueError(
                        f"pad {pad_ref.component}.{pad_ref.pad} belongs to both "
                        f"{previous!r} and {net.name!r}"
                    )
                assigned_pads[pad_ref] = net.name

        known_nets = set(net_names)
        for placement in self.placements:
            footprint = self.footprints[placement.footprint]
            for group in footprint.internal_pad_groups:
                nets = {assigned_pads.get(PadReference(placement.reference, number))
                        for number in group.numbers}
                if len(nets - {None}) > 1:
                    raise ValueError("internally connected pads belong to different nets")
                if None in nets and len(nets) > 1:
                    raise ValueError("all numbered pads in an internal group must share a net")
        permitted_pads = set()
        for rule in self.via_in_pad_rules:
            if rule.pad in permitted_pads:
                raise ValueError("multiple via-in-pad permissions for one pad")
            permitted_pads.add(rule.pad)
            if assigned_pads.get(rule.pad) != "GND":
                raise ValueError("via-in-pad permission requires a connected GND pad")
            placement = next(p for p in self.placements if p.reference == rule.pad.component)
            lands = [p for p in self.footprints[placement.footprint].pads if p.number == rule.pad.pad]
            if any(p.kind is not PadKind.SMD for p in lands):
                raise ValueError("via-in-pad permission requires SMD lands")
            if self.metadata.get("fabrication_profile") != "jlcpcb-six-layer" or len(self.stackup.copper_layers) != 6:
                raise ValueError("filled-capped via-in-pad requires the JLCPCB six-layer profile")
            if not any(z.net == "GND" and any(l not in (CopperLayer.FRONT, CopperLayer.BACK)
                                             for l in z.layers) for z in self.zones):
                raise ValueError("via-in-pad permission requires a declared inner GND zone")
            if rule.rows is not None:
                from .pad_via_arrays import validate_array_fit
                validate_array_fit(self, rule, lands)
        layers = set(self.stackup.copper_layers)
        for track in self.tracks:
            if track.net not in known_nets:
                raise ValueError(f"track references unknown net {track.net!r}")
            if track.layer not in layers:
                raise ValueError(f"track uses unavailable layer {track.layer.value!r}")
        for via in self.vias:
            if via.net not in known_nets:
                raise ValueError(f"via references unknown net {via.net!r}")
            if via.from_layer not in layers or via.to_layer not in layers:
                raise ValueError("via uses a layer not present in the stackup")
            technologies = {item.id: item for item in self.stackup.via_technologies}
            if technologies and via.technology is None:
                raise ValueError("vias must select a technology when the stackup defines a catalog")
            if via.technology is not None:
                technology = technologies.get(via.technology)
                if technology is None:
                    raise ValueError(f"via references unknown technology {via.technology!r}")
                if {via.from_layer, via.to_layer} != {technology.from_layer, technology.to_layer}:
                    raise ValueError("via span does not match its selected technology")
                if via.drill_nm < technology.minimum_drill_nm:
                    raise ValueError("via drill is below its technology limit")
                if (via.size_nm - via.drill_nm) // 2 < technology.minimum_annular_ring_nm:
                    raise ValueError("via annular ring is below its technology limit")
                depth = _via_span_depth(self.stackup, via.from_layer, via.to_layer)
                if Decimal(depth) / Decimal(via.drill_nm) > technology.maximum_aspect_ratio:
                    raise ValueError("via aspect ratio exceeds its technology limit")

        region_names = [region.name for region in self.regions]
        if len(region_names) != len(set(region_names)):
            raise ValueError("physical placement region names must be unique")
        keepout_names = [keepout.name for keepout in self.keepouts]
        if len(keepout_names) != len(set(keepout_names)):
            raise ValueError("physical placement keepout names must be unique")
        known_references = set(placement_refs)
        domain_nets = set()
        net_members = {net.name: set(net.pads) for net in self.nets}
        for domain in self.power_domains:
            if domain.net in domain_nets:
                raise ValueError("physical power domains must have unique nets")
            domain_nets.add(domain.net)
            if domain.net not in net_members or not set(domain.members) <= net_members[domain.net]:
                raise ValueError("physical power domain references unknown net or nonmember pad")
        known_regions = set(region_names)
        ruled_references: set[str] = set()
        for rule in self.placement_rules:
            if rule.reference not in known_references:
                raise ValueError(
                    f"placement rule references unknown component {rule.reference!r}"
                )
            if rule.reference in ruled_references:
                raise ValueError(
                    f"component {rule.reference!r} has multiple placement rules"
                )
            ruled_references.add(rule.reference)
            if rule.region is not None and rule.region not in known_regions:
                raise ValueError(
                    f"placement rule references unknown region {rule.region!r}"
                )
        for rule in self.relative_rules:
            for target in rule.targets:
                if target.reference not in known_references:
                    raise ValueError(
                        f"relative placement rule references unknown component "
                        f"{target.reference!r}"
                    )
                if target.pad is not None and target.pad not in pads_by_component[target.reference]:
                    raise ValueError(
                        f"relative placement rule references unknown pad "
                        f"{target.reference}.{target.pad}"
                    )
        group_names = [group.name for group in self.placement_groups]
        if len(group_names) != len(set(group_names)):
            raise ValueError("physical placement group names must be unique")
        for group in self.placement_groups:
            unknown = set(group.references) - known_references
            if unknown:
                raise ValueError(
                    f"placement group {group.name!r} references unknown component "
                    f"{min(unknown)!r}"
                )

        # Validate template identity even before initial grid poses are legalized.
        from .clusters import validate_cluster_bindings
        validate_cluster_bindings(self)
        from .hard_macros import validate_hard_macros
        validate_hard_macros(self)

        routed_rule_nets: set[str] = set()
        partners = {rule.net: rule for rule in self.net_routing_rules}
        for rule in self.net_routing_rules:
            if rule.net not in known_nets:
                raise ValueError(f"routing rule references unknown net {rule.net!r}")
            if rule.net in routed_rule_nets:
                raise ValueError(f"net {rule.net!r} has multiple routing rules")
            routed_rule_nets.add(rule.net)
            unavailable = set(rule.allowed_layers) - layers
            if rule.shared_reference_layer is not None and rule.shared_reference_layer not in layers:
                raise ValueError(f"routing rule for {rule.net!r} uses unavailable shared reference layer")
            if rule.shared_reference_layer in rule.allowed_layers:
                raise ValueError(f"routing rule for {rule.net!r} routes on its declared reference plane")
            if unavailable:
                raise ValueError(
                    f"routing rule for {rule.net!r} uses unavailable layer "
                    f"{min(layer.value for layer in unavailable)!r}"
                )
            if (
                rule.differential_partner is not None
                and rule.differential_partner not in known_nets
            ):
                raise ValueError(
                    f"routing rule for {rule.net!r} references unknown partner "
                    f"{rule.differential_partner!r}"
                )
            if rule.return_via_net is not None and rule.return_via_net not in known_nets:
                raise ValueError(f"routing rule for {rule.net!r} references unknown return net {rule.return_via_net!r}")
            partner = partners.get(rule.differential_partner or "")
            if partner is not None and partner.tuning_style is not rule.tuning_style:
                raise ValueError(f"routing rules for {rule.net!r} and {partner.net!r} must agree on tuning_style")
            # Breakout relaxations are checked against the effective profile,
            # including board defaults when the rule leaves a value implicit.
            if rule.breakout_width_nm is not None:
                if rule.breakout_width_nm > (rule.width_nm or self.rules.default_track_width_nm):
                    raise ValueError(f"routing rule for {rule.net!r}: breakout width may only relax "
                                     "(not exceed) the effective routing width")
                if rule.breakout_width_nm < self.rules.minimum_track_width_nm:
                    raise ValueError(f"routing rule for {rule.net!r}: breakout width is below the "
                                     "board minimum track width")
            if rule.breakout_clearance_nm is not None:
                if rule.breakout_clearance_nm > max(rule.clearance_nm or 0, self.rules.minimum_clearance_nm):
                    raise ValueError(f"routing rule for {rule.net!r}: breakout clearance may only relax "
                                     "(not exceed) the effective routing clearance")
                if rule.breakout_clearance_nm < self.rules.minimum_clearance_nm:
                    raise ValueError(f"routing rule for {rule.net!r}: breakout clearance is below the "
                                     "board minimum clearance")
        group_ids = [group.id for group in self.match_groups]
        if len(group_ids) != len(set(group_ids)):
            raise ValueError("length-match group ids must be unique")
        grouped: dict[str, str] = {}
        for group in self.match_groups:
            for net in group.nets:
                if net not in known_nets:
                    raise ValueError(f"length-match group {group.id!r} references unknown net {net!r}")
                if net in grouped:
                    raise ValueError(f"net {net!r} belongs to length-match groups "
                                     f"{grouped[net]!r} and {group.id!r}")
                grouped[net] = group.id
        scoped: set[str] = set()
        for rule in self.component_hole_clearances:
            if rule.reference in scoped:
                raise ValueError(f"component {rule.reference!r} has more than one hole clearance")
            scoped.add(rule.reference)
            placement = next((p for p in self.placements if p.reference == rule.reference), None)
            if placement is None:
                raise ValueError(f"hole clearance references unknown placement {rule.reference!r}")
            if not any(pad.kind is PadKind.NON_PLATED_THROUGH_HOLE
                       for pad in self.footprints[placement.footprint].pads):
                raise ValueError(f"hole clearance for {rule.reference!r}: its footprint has no "
                                 "non-plated holes")
            if rule.clearance_nm > self.rules.minimum_hole_clearance_nm:
                raise ValueError(f"hole clearance for {rule.reference!r} may only relax (not exceed) "
                                 "the board minimum hole clearance")


def _via_span_depth(stackup: Stackup, first: CopperLayer, second: CopperLayer) -> int:
    if not stackup.physical_layers:
        return stackup.thickness_nm
    positions = {
        layer.copper_layer: index
        for index, layer in enumerate(stackup.physical_layers)
        if layer.copper_layer is not None
    }
    low, high = sorted((positions[first], positions[second]))
    return sum(layer.thickness_nm for layer in stackup.physical_layers[low : high + 1])


def select_via_technology(
    stackup: Stackup,
    from_layer: CopperLayer,
    to_layer: CopperLayer,
    size_nm: int,
    drill_nm: int,
) -> str | None:
    """Choose the first legal technology in stable profile order."""
    if not stackup.via_technologies:
        return None
    depth = _via_span_depth(stackup, from_layer, to_layer)
    for technology in stackup.via_technologies:
        if {from_layer, to_layer} != {technology.from_layer, technology.to_layer}:
            continue
        if drill_nm < technology.minimum_drill_nm:
            continue
        if (size_nm - drill_nm) // 2 < technology.minimum_annular_ring_nm:
            continue
        if Decimal(depth) / Decimal(drill_nm) > technology.maximum_aspect_ratio:
            continue
        return technology.id
    raise ValueError(
        f"no legal via technology for {from_layer.value} to {to_layer.value} "
        f"with size/drill {size_nm}/{drill_nm} nm"
    )
