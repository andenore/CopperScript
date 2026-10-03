"""Separate electrical and mechanical design inputs; no presentation geometry."""
from __future__ import annotations

from dataclasses import dataclass, field, replace
from decimal import Decimal
from types import MappingProxyType
from typing import Mapping

from .model import Board
from .physical import BoardCutout, BoardOutline, DesignRules, MechanicalHole, PhysicalBoard, Point
from .quantities import Length
from .syntax import CopperScriptError, Document, MechanicalDecl, RawQuantity


@dataclass(frozen=True, slots=True)
class MechanicalDesign:
    outline: BoardOutline
    holes: tuple[MechanicalHole, ...] = ()
    rule_overrides: Mapping[str, int] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "holes", tuple(self.holes))
        object.__setattr__(self, "rule_overrides", MappingProxyType(dict(self.rule_overrides)))
        DesignRules(**self.rule_overrides)
        PhysicalBoard("mechanical-validation", self.outline, {}, (), (), mechanical_holes=self.holes)
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


def lower_mechanical(document: Document) -> MechanicalDesign | None:
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
    seen_rules = False

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

    for item in block.items:
        try:
            p = item.parameters
            if item.name:
                if item.name in ids:
                    raise ValueError(f"duplicate mechanical feature id {item.name!r}")
                ids.add(item.name)
            if item.kind == "rules":
                if seen_rules:
                    raise ValueError("only one mechanical rules block is allowed")
                seen_rules = True
                allowed = {name.removesuffix("_nm") for name in DesignRules.__dataclass_fields__}
                required = set()
            elif item.kind == "hole":
                allowed = {"position", "diameter", "head_clearance_radius"}
                required = {"position", "diameter"}
            elif item.shape == "circle" and item.kind == "outline":
                allowed = {"center", "diameter", "maximum_chord_error"}
                required = {"diameter"}
            elif item.shape == "rectangle" and item.kind == "outline":
                allowed = {"origin", "width", "height"}
                required = {"width", "height"}
            elif item.shape == "polygon":
                allowed = required = {"vertices"}
            else:
                raise ValueError(f"unsupported {item.kind} shape {item.shape!r}")
            if set(p) - allowed:
                raise ValueError(f"unknown mechanical property {sorted(set(p)-allowed)[0]!r}")
            if required - set(p):
                raise ValueError(f"missing mechanical property {sorted(required-set(p))[0]!r}")
            if item.kind == "rules":
                rules = {name + "_nm": length(value) for name, value in p.items()}
            elif item.kind == "hole":
                holes.append(MechanicalHole(item.name, point(p["position"]), length(p["diameter"]),
                    length(p["head_clearance_radius"]) if "head_clearance_radius" in p else 0))
            elif item.kind == "cutout":
                cutouts.append(BoardCutout(item.name, vertices(p["vertices"])))
            else:
                if outline is not None:
                    raise ValueError("exactly one board outline is allowed")
                if item.shape == "circle":
                    outline = BoardOutline.circle(mm(p["diameter"]),
                        center=point(p["center"]) if "center" in p else None,
                        maximum_chord_error_mm=mm(p["maximum_chord_error"]) if "maximum_chord_error" in p else "0.01")
                elif item.shape == "rectangle":
                    outline = BoardOutline.rectangle(mm(p["width"]), mm(p["height"]),
                        origin=point(p["origin"]) if "origin" in p else None)
                else:
                    outline = BoardOutline(vertices(p["vertices"]))
        except (ValueError, TypeError) as exc:
            raise CopperScriptError("MEC002", str(exc), item.location) from exc
    try:
        if outline is None:
            raise ValueError("mechanical block requires exactly one outline")
        return MechanicalDesign(replace(outline, cutouts=tuple(cutouts)), tuple(holes), rules)
    except ValueError as exc:
        raise CopperScriptError("MEC003", str(exc), block.location) from exc
