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
    BACK = "B.Cu"


class PadKind(str, Enum):
    SMD = "smd"
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


@dataclass(frozen=True, slots=True)
class Stackup:
    copper_layers: tuple[CopperLayer, ...] = (
        CopperLayer.FRONT,
        CopperLayer.BACK,
    )
    thickness_nm: Nanometres = nm_from_mm("1.6")

    def __post_init__(self) -> None:
        object.__setattr__(self, "copper_layers", tuple(self.copper_layers))
        if len(self.copper_layers) < 1:
            raise ValueError("a stackup requires at least one copper layer")
        if len(set(self.copper_layers)) != len(self.copper_layers):
            raise ValueError("stackup copper layers must be unique")
        if self.thickness_nm <= 0:
            raise ValueError("board thickness must be positive")


@dataclass(frozen=True, slots=True)
class DesignRules:
    minimum_clearance_nm: Nanometres = nm_from_mm("0.2")
    default_track_width_nm: Nanometres = nm_from_mm("0.25")
    default_via_size_nm: Nanometres = nm_from_mm("0.8")
    default_via_drill_nm: Nanometres = nm_from_mm("0.4")

    def __post_init__(self) -> None:
        values = (
            self.minimum_clearance_nm,
            self.default_track_width_nm,
            self.default_via_size_nm,
            self.default_via_drill_nm,
        )
        if any(value <= 0 for value in values):
            raise ValueError("physical design rules must be positive")
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

    def __post_init__(self) -> None:
        if not self.number and self.kind is not PadKind.NON_PLATED_THROUGH_HOLE:
            raise ValueError("electrical footprint pad number cannot be empty")
        object.__setattr__(
            self,
            "rotation_degrees",
            Decimal(str(self.rotation_degrees)) % Decimal(360),
        )
        if self.kind is PadKind.SMD and self.drill is not None:
            raise ValueError("SMD pads cannot have a drill")
        if self.kind is not PadKind.SMD:
            if self.drill is None:
                raise ValueError("through-hole pads require a drill")
            if (
                self.drill.width_nm >= self.size.width_nm
                or self.drill.height_nm >= self.size.height_nm
            ):
                raise ValueError("pad drill must be smaller than pad size")
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
    metadata: Mapping[str, str] = field(default_factory=dict)
    courtyard: tuple[Point, ...] = ()
    height_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
        object.__setattr__(self, "graphics", tuple(self.graphics))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))
        object.__setattr__(self, "courtyard", tuple(self.courtyard))
        if not self.name:
            raise ValueError("footprint name cannot be empty")
        if self.courtyard and len(self.courtyard) < 3:
            raise ValueError("a footprint courtyard requires at least three points")
        if self.height_nm is not None and self.height_nm <= 0:
            raise ValueError("footprint height must be positive")


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

    def __post_init__(self) -> None:
        if self.size_nm <= 0 or self.drill_nm <= 0:
            raise ValueError("via size and drill must be positive")
        if self.drill_nm >= self.size_nm:
            raise ValueError("via drill must be smaller than via size")
        if self.from_layer == self.to_layer:
            raise ValueError("a via must connect distinct copper layers")


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
        self._validate_references()

    def _validate_references(self) -> None:
        placement_refs = [placement.reference for placement in self.placements]
        if len(placement_refs) != len(set(placement_refs)):
            raise ValueError("physical placement references must be unique")
        net_names = [net.name for net in self.nets]
        if len(net_names) != len(set(net_names)):
            raise ValueError("physical net names must be unique")

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
                if physical_pad.kind is PadKind.NON_PLATED_THROUGH_HOLE:
                    raise ValueError(
                        f"non-plated pad {pad_ref.component}.{pad_ref.pad} cannot "
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
