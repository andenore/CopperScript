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
class BoardOutline:
    """Closed polygon; the closing edge is implicit."""

    vertices: tuple[Point, ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "vertices", tuple(self.vertices))
        if len(self.vertices) < 3:
            raise ValueError("a board outline requires at least three vertices")
        if len(set(self.vertices)) < 3:
            raise ValueError("a board outline requires three distinct vertices")

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

    def __post_init__(self) -> None:
        object.__setattr__(self, "layers", tuple(self.layers))
        if not self.id or not self.net or not self.layers:
            raise ValueError("a copper zone requires an id, net, and layer")
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

    def __post_init__(self) -> None:
        if not self.id or self.thickness_nm <= 0:
            raise ValueError("stackup layers require an id and positive thickness")
        if (self.kind is StackupLayerKind.COPPER) != (self.copper_layer is not None):
            raise ValueError("only copper stackup layers name a copper layer")
        for value in (self.relative_permittivity, self.loss_tangent):
            if value is not None and value <= 0:
                raise ValueError("dielectric properties must be positive")


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

    def __post_init__(self) -> None:
        values = (
            self.minimum_clearance_nm,
            self.minimum_hole_clearance_nm,
            self.minimum_track_width_nm,
            self.default_track_width_nm,
            self.default_via_size_nm,
            self.default_via_drill_nm,
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

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
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


@dataclass(frozen=True, slots=True)
class PlacementKeepout:
    name: str
    outline: BoardOutline
    side: BoardSide | None = None
    maximum_component_height_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("placement keepout name cannot be empty")
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

    def __post_init__(self) -> None:
        if not self.reference:
            raise ValueError("component placement rule requires a reference")
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
    return_via_net: str | None = None
    maximum_return_via_distance_nm: Nanometres | None = None
    impedance_evidence_digest: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "allowed_layers", tuple(self.allowed_layers))
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
            ("maximum return via distance", self.maximum_return_via_distance_nm),
        ):
            if value is not None and value <= 0:
                raise ValueError(f"routing {name} must be positive")
        if self.max_vias is not None and self.max_vias < 0:
            raise ValueError("routing maximum via count cannot be negative")
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
    net_routing_rules: tuple[NetRoutingRule, ...] = ()
    zones: tuple[CopperZone, ...] = ()
    copper_keepouts: tuple[CopperKeepout, ...] = ()
    zone_fills: tuple[ZoneFillResult, ...] = ()

    def __post_init__(self) -> None:
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
        object.__setattr__(self, "net_routing_rules", tuple(self.net_routing_rules))
        object.__setattr__(self, "zones", tuple(self.zones))
        object.__setattr__(self, "copper_keepouts", tuple(self.copper_keepouts))
        object.__setattr__(self, "zone_fills", tuple(self.zone_fills))
        self._validate_references()

    def _validate_references(self) -> None:
        placement_refs = [placement.reference for placement in self.placements]
        if len(placement_refs) != len(set(placement_refs)):
            raise ValueError("physical placement references must be unique")
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

        routed_rule_nets: set[str] = set()
        for rule in self.net_routing_rules:
            if rule.net not in known_nets:
                raise ValueError(f"routing rule references unknown net {rule.net!r}")
            if rule.net in routed_rule_nets:
                raise ValueError(f"net {rule.net!r} has multiple routing rules")
            routed_rule_nets.add(rule.net)
            unavailable = set(rule.allowed_layers) - layers
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
