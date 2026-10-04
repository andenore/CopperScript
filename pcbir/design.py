"""Separate electrical and mechanical design inputs; no presentation geometry."""
from __future__ import annotations

from dataclasses import dataclass, field, replace
from decimal import Decimal
from types import MappingProxyType
from typing import Mapping

from .model import Board
from .physical import (BoardCutout, BoardOutline, BoardSide, CopperKeepout, CopperLayer,
    DesignRules, MechanicalHole, PhysicalBoard, PlacementKeepout, Point,
    PolygonRing, PolygonWithHoles)
from .mechanical_profiles import (MechanicalProfileDefinition, MechanicalProfileInstance,
    MechanicalFeatureSource, expand_mechanical_items)
from .quantities import Length
from .syntax import CopperScriptError, Document, MechanicalDecl, RawQuantity, SourceLocation
from .mechanical_anchors import MechanicalAttachment, resolve_datums, resolve_edges, resolve_attachments
from .physical import BoardDatum, BoardEdge
from .physical import BodyOverhang, ComponentHeight, AssemblyEnvelope, AssemblyAccess
from .physical import MechanicalSlot,BoundaryLine,BoundaryArc,BoardBoundaryPath


@dataclass(frozen=True, slots=True)
class MechanicalConnectorBinding:
    role: str
    reference: str
    anchor_pad: str
    position: Point
    rotation: Decimal
    side: BoardSide
    location: SourceLocation
    footprint: str | None = None


@dataclass(frozen=True, slots=True)
class MechanicalDesign:
    outline: BoardOutline
    holes: tuple[MechanicalHole, ...] = ()
    rule_overrides: Mapping[str, int] = field(default_factory=dict)
    connectors: tuple[MechanicalConnectorBinding, ...] = ()
    keepouts: tuple[PlacementKeepout, ...] = ()
    copper_keepouts: tuple[CopperKeepout, ...] = ()
    profiles: tuple[MechanicalProfileInstance, ...] = ()
    sources: tuple[MechanicalFeatureSource, ...] = ()
    datums: tuple[BoardDatum, ...] = ()
    boundary_edges: tuple[BoardEdge, ...] = ()
    attachments: tuple[MechanicalAttachment, ...] = ()
    body_overhangs: tuple[BodyOverhang, ...] = ()
    component_heights: tuple[ComponentHeight, ...] = ()
    assembly_envelopes: tuple[AssemblyEnvelope, ...] = ()
    assembly_access: tuple[AssemblyAccess, ...] = ()
    slots: tuple[MechanicalSlot,...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "holes", tuple(self.holes))
        for name in ("connectors", "keepouts", "copper_keepouts", "profiles", "sources", "datums", "boundary_edges", "attachments", "body_overhangs", "component_heights", "assembly_envelopes", "assembly_access", "slots"):
            object.__setattr__(self, name, tuple(getattr(self, name)))
        object.__setattr__(self, "rule_overrides", MappingProxyType(dict(self.rule_overrides)))
        DesignRules(**self.rule_overrides)
        PhysicalBoard("mechanical-validation", self.outline, {}, (), (), mechanical_holes=self.holes,mechanical_slots=self.slots,
                      rules=DesignRules(**self.rule_overrides))
        from .geometry import RoundedConvexShape
        from .mechanical import shape_in_outline
        for hole in self.holes:
            if hole.head_clearance_radius_nm and not shape_in_outline(
                    RoundedConvexShape((hole.position,), hole.head_clearance_radius_nm), self.outline, 1):
                raise ValueError(f"mechanical hole {hole.id!r} head clearance must fit board material")


@dataclass(frozen=True, slots=True)
class Design:
    electrical: Board
    mechanical: MechanicalDesign | None = None


def lower_mechanical(document: Document,
                     profiles: Mapping[str, MechanicalProfileDefinition] | None = None) -> MechanicalDesign | None:
    blocks = [d for d in document.declarations if isinstance(d, MechanicalDecl)]
    if not blocks:
        return None
    if len(blocks) != 1:
        raise CopperScriptError("MEC001", "only one mechanical block is allowed", blocks[1].location)
    block = blocks[0]
    outline = None
    cutouts = []
    holes = []
    rules = {}
    ids = set()
    connectors, keepouts, copper_keepouts = [], [], []
    datum_items, edge_items, attachment_items = [], [], []
    assembly_items = []
    boundary_items,slots=[],[]
    path_options=None
    items, sources, instances = expand_mechanical_items(block.items, profiles or {})

    def length(value):
        if not isinstance(value, RawQuantity):
            raise ValueError("mechanical dimensions require typed lengths")
        nm = Length.of(value.value, value.unit).base_value * Decimal(1_000_000_000)
        if not nm.is_finite() or nm != nm.to_integral_value():
            raise ValueError("mechanical lengths must be exact integer nanometres")
        return int(nm)

    def point(value):
        if not isinstance(value, tuple) or len(value) != 2:
            raise ValueError("expected a coordinate pair (x, y)")
        return Point(length(value[0]), length(value[1]))

    def vertices(value):
        if not isinstance(value, tuple):
            raise ValueError("expected a list of coordinate pairs")
        return tuple(point(p) for p in value)

    def mm(value):
        return Decimal(length(value)) / Decimal(1_000_000)

    for item in items:
        try:
            p = item.parameters
            if item.name:
                if item.name in ids:
                    raise ValueError(f"duplicate mechanical feature id {item.name!r}")
                ids.add(item.name)
            if item.kind in {"datum", "edge", "attach"}:
                {"datum": datum_items, "edge": edge_items, "attach": attachment_items}[item.kind].append(item)
                continue
            if item.kind in {'overhang','component_height','enclosure','assembly_access'}:
                assembly_items.append(item)
                continue
            if item.kind=='boundary':
                boundary_items.append(item)
                continue
            if item.kind=='outline' and item.shape=='path':
                if outline is not None or path_options is not None:raise ValueError('exactly one board outline is allowed')
                if set(p)-{'maximum_chord_error'}:raise ValueError('path outline only accepts maximum_chord_error')
                path_options=p
                continue
            if item.kind == "rules":
                allowed = {name.removesuffix("_nm") for name in DesignRules.__dataclass_fields__}
                required = set()
            elif item.kind == "connector":
                allowed = {"component", "anchor_pad", "position", "rotation", "side", "footprint"}
                required = {"component", "anchor_pad", "position", "rotation", "side"}
            elif item.kind in {"keepout", "copper_keepout"}:
                allowed = ({"side", "maximum_height"} if item.kind == "keepout" else
                           {"layers", "block_tracks", "block_vias", "block_pads", "block_zones", "block_footprints"})
                if item.shape == "rectangle":
                    allowed |= {"width", "height", "origin"}
                    required = {"width", "height"}
                elif item.shape == "polygon":
                    allowed |= {"vertices"}
                    required = {"vertices"}
                else:
                    raise ValueError("keepouts require rectangle or polygon geometry")
                if item.kind == "copper_keepout":
                    required |= {"layers"}
            elif item.kind == "hole":
                allowed = {"position", "diameter", "head_clearance_radius"}
                required = {"position", "diameter"}
            elif item.kind=='slot':
                allowed=required={'start','end','width'}
            elif item.shape == "circle" and item.kind == "outline":
                allowed = {"center", "diameter", "maximum_chord_error"}
                required = {"diameter"}
            elif item.shape == "rectangle" and item.kind == "outline":
                allowed = {"origin", "width", "height"}
                required = {"width", "height"}
            elif item.shape=='rounded_rectangle' and item.kind=='outline':
                allowed={'origin','width','height','corner_radius','maximum_chord_error'}
                required={'width','height','corner_radius'}
            elif item.shape == "polygon":
                allowed = required = {"vertices"}
            else:
                raise ValueError(f"unsupported {item.kind} shape {item.shape!r}")
            if set(p) - allowed:
                raise ValueError(f"unknown mechanical property {sorted(set(p)-allowed)[0]!r}")
            if required - set(p):
                raise ValueError(f"missing mechanical property {sorted(required-set(p))[0]!r}")
            if item.kind == "rules":
                additions = {name + "_nm": length(value) for name, value in p.items()}
                if set(rules) & set(additions):
                    raise ValueError("conflicting mechanical rule ownership")
                rules.update(additions)
            elif item.kind == "connector":
                if isinstance(p["anchor_pad"], bool) or not isinstance(p["anchor_pad"], (str, int)) or not str(p["anchor_pad"]):
                    raise ValueError("connector anchor_pad must name a physical pad")
                if isinstance(p["rotation"], bool) or not isinstance(p["rotation"], (int, float)):
                    raise ValueError("connector rotation must be numeric degrees")
                angle = Decimal(str(p["rotation"]))
                if not angle.is_finite():
                    raise ValueError("connector rotation must be finite")
                if "footprint" in p and (not isinstance(p["footprint"], str) or not p["footprint"]):
                    raise ValueError("connector footprint must be a nonempty library identifier")
                if any(c.reference == p["component"] for c in connectors):
                    raise ValueError("multiple profile connector roles bind the same component")
                connectors.append(MechanicalConnectorBinding(item.name, p["component"], str(p["anchor_pad"]),
                    point(p["position"]), angle % 360, BoardSide(p["side"]), item.location, p.get("footprint")))
            elif item.kind in {"keepout", "copper_keepout"}:
                region = (BoardOutline.rectangle(mm(p["width"]), mm(p["height"]),
                          origin=point(p["origin"]) if "origin" in p else None)
                          if item.shape == "rectangle" else BoardOutline(vertices(p["vertices"])))
                if item.kind == "keepout":
                    side = BoardSide(p["side"]) if "side" in p else None
                    keepouts.append(PlacementKeepout(item.name, region, side,
                        length(p["maximum_height"]) if "maximum_height" in p else None))
                else:
                    if not isinstance(p["layers"], str):
                        raise ValueError("copper keepout layers must be a comma-separated string")
                    layers = tuple(CopperLayer(v.strip()) for v in p["layers"].split(","))
                    flags = {key: value for key, value in p.items() if key.startswith("block_")}
                    if any(type(v) is not bool for v in flags.values()):
                        raise ValueError("copper keepout block properties must be booleans")
                    copper_keepouts.append(CopperKeepout(item.name, layers,
                        PolygonWithHoles(PolygonRing(region.vertices)), **flags))
            elif item.kind == "hole":
                holes.append(MechanicalHole(item.name, point(p["position"]), length(p["diameter"]),
                    length(p["head_clearance_radius"]) if "head_clearance_radius" in p else 0))
            elif item.kind=='slot':
                slots.append(MechanicalSlot(item.name,point(p['start']),point(p['end']),length(p['width'])))
            elif item.kind == "cutout":
                cutouts.append(BoardCutout(item.name, vertices(p["vertices"])))
            else:
                if outline is not None or path_options is not None:
                    raise ValueError("exactly one board outline is allowed")
                if item.shape == "circle":
                    outline = BoardOutline.circle(mm(p["diameter"]),
                        center=point(p["center"]) if "center" in p else None,
                        maximum_chord_error_mm=mm(p["maximum_chord_error"]) if "maximum_chord_error" in p else "0.01")
                elif item.shape == "rectangle":
                    outline = BoardOutline.rectangle(mm(p["width"]), mm(p["height"]),
                        origin=point(p["origin"]) if "origin" in p else None)
                elif item.shape=='rounded_rectangle':
                    outline=BoardOutline.rounded_rectangle(mm(p['width']),mm(p['height']),mm(p['corner_radius']),
                        origin=point(p['origin']) if 'origin' in p else None,
                        maximum_chord_error_mm=mm(p['maximum_chord_error']) if 'maximum_chord_error' in p else '0.01')
                else:
                    outline = BoardOutline(vertices(p["vertices"]))
        except (ValueError, TypeError) as exc:
            raise CopperScriptError("MEC002", str(exc), item.location) from exc
    try:
        if path_options is not None:
            primitives=[]
            for item in boundary_items:
                p=item.parameters
                required={'start','end'} if item.shape=='line' else {'start','mid','end'} if item.shape=='arc' else None
                if required is None or set(p)!=required:raise ValueError('boundary primitives require line start/end or arc start/mid/end')
                primitives.append(BoundaryLine(item.name,point(p['start']),point(p['end'])) if item.shape=='line' else
                    BoundaryArc(item.name,point(p['start']),point(p['mid']),point(p['end'])))
            outline=BoardOutline.from_path(BoardBoundaryPath(tuple(primitives),length(path_options['maximum_chord_error']) if 'maximum_chord_error' in path_options else 10000))
        elif boundary_items:raise ValueError('boundary declarations require an outline path')
        if outline is None:
            raise ValueError("mechanical block requires exactly one outline")
        datums = resolve_datums(datum_items, point)
        edges = resolve_edges(edge_items, outline, point)
        if outline.boundary_path:
            edges=(*edges,*(BoardEdge(s.id,s.start,s.end) for s in outline.boundary_path.segments if isinstance(s,BoundaryLine)))
            if len({e.id for e in edges})!=len(edges):raise ValueError('duplicate boundary edge IDs')
        attachments = resolve_attachments(attachment_items, datums, edges, outline, point)
        from .mechanical_assembly import lower_assembly
        assembly = lower_assembly(assembly_items, replace(outline,cutouts=tuple(cutouts)), edges, point, length)
        return MechanicalDesign(replace(outline, cutouts=tuple(cutouts)), tuple(holes), rules,
            tuple(connectors), tuple(keepouts), tuple(copper_keepouts), instances, sources,
            datums, edges, attachments, *assembly, tuple(slots))
    except (ValueError, TypeError) as exc:
        raise CopperScriptError("MEC003", str(exc), block.location) from exc
