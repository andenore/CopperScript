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
    drill_nm: Nanometres | None = None

    def __post_init__(self) -> None:
        if not self.number:
            raise ValueError("footprint pad number cannot be empty")
        if self.kind is PadKind.SMD and self.drill_nm is not None:
            raise ValueError("SMD pads cannot have a drill")
        if self.kind is not PadKind.SMD:
            if self.drill_nm is None or self.drill_nm <= 0:
                raise ValueError("through-hole pads require a positive drill")
            if self.drill_nm >= min(self.size.width_nm, self.size.height_nm):
                raise ValueError("pad drill must be smaller than pad size")


@dataclass(frozen=True, slots=True)
class PhysicalFootprint:
    name: str
    pads: tuple[FootprintPad, ...]
    body_size: Size
    source_library_id: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", tuple(self.pads))
        if not self.name:
            raise ValueError("footprint name cannot be empty")
        numbers = [pad.number for pad in self.pads]
        if len(numbers) != len(set(numbers)):
            raise ValueError(f"footprint {self.name!r} has duplicate pad numbers")


@dataclass(frozen=True, slots=True)
class Placement:
    reference: str
    footprint: str
    position: Point
    rotation_degrees: int = 0
    side: BoardSide = BoardSide.FRONT
    value: str = ""
    source_path: str | None = None

    def __post_init__(self) -> None:
        if not self.reference:
            raise ValueError("placement reference cannot be empty")
        object.__setattr__(self, "rotation_degrees", self.rotation_degrees % 360)


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

    def __post_init__(self) -> None:
        object.__setattr__(self, "footprints", MappingProxyType(dict(self.footprints)))
        object.__setattr__(self, "placements", tuple(self.placements))
        object.__setattr__(self, "nets", tuple(self.nets))
        object.__setattr__(self, "tracks", tuple(self.tracks))
        object.__setattr__(self, "vias", tuple(self.vias))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))
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
