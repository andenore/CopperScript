"""Electrical-to-physical lowering for PCB backend development.

This is intentionally not an automatic placer. It can resolve verified
external footprints or, by explicit request, generate proxy geometry. Both
paths use deterministic grid placement and mark the result as a draft.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
import json
import re
from typing import Callable, Mapping

from .elaborate import elaborate
from .modes import pins_bonded_to_pad
from .pad_connections import merge_internal_pad_groups
from .footprints import FootprintResolver
from .importers import FootprintImportResult
from .model import (
    Board,
    ComponentInstance,
    ConstraintKind,
    DeviceDefinition,
    FlatElectricalView,
    PartDefinition,
)
from .physical import (
    AlignmentAxis,
    BoardOutline,
    BoardSide,
    ComponentHoleClearance,
    ComponentPlacementRule,
    CopperLayer,
    CopperZone,
    DesignRules,
    FootprintPad,
    IslandPolicy,
    PadReference,
    PadKind,
    PadViaInPadRule,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    NetMatchGroup,
    NetRoutingRule,
    Placement,
    PlacementGroup,
    PlacementKeepout,
    PlacementRegion,
    PlacementTarget,
    Point,
    PolygonRing,
    PolygonWithHoles,
    RelativePlacementKind,
    RelativePlacementRule,
    RouteKind,
    ReturnViaPolicy,
    Size,
    Stackup,
    TuningStyle,
    ZoneConnection,
    nm_from_mm,
)
from .quantities import Length, Quantity
from .design import Design
from .mechanical_profiles import mechanical_provenance
from .power_planning import lower_power_domains
from .zone_geometry import lower_zone_outline


@dataclass(frozen=True, slots=True)
class PrototypePhysicalOptions:
    board_width_mm: float = 100.0
    board_height_mm: float = 80.0
    copper_layers: int = 2
    fabrication_profile: str = "generic"
    columns: int = 3
    margin_mm: float = 12.0

    def __post_init__(self) -> None:
        if self.board_width_mm <= 0 or self.board_height_mm <= 0:
            raise ValueError("prototype board dimensions must be positive")
        if self.columns < 1:
            raise ValueError("prototype placement columns must be at least one")
        if self.copper_layers not in {2, 4, 6}:
            raise ValueError("prototype physicalizer supports two, four, or six copper layers")
        if self.fabrication_profile not in {"generic", "jlcpcb-four-layer", "jlcpcb-six-layer"}:
            raise ValueError("unknown prototype fabrication profile")
        if self.fabrication_profile == "jlcpcb-four-layer" and self.copper_layers != 4:
            raise ValueError("JLCPCB four-layer profile requires four copper layers")
        if self.fabrication_profile == "jlcpcb-six-layer" and self.copper_layers != 6:
            raise ValueError("JLCPCB six-layer profile requires six copper layers")
        if self.margin_mm <= 0:
            raise ValueError("prototype board margin must be positive")
        if self.margin_mm * 2 >= min(self.board_width_mm, self.board_height_mm):
            raise ValueError("prototype board margin leaves no placement area")


@dataclass(frozen=True, slots=True)
class FootprintAuditEntry:
    reference: str
    components: tuple[str, ...]
    source_path: str | None
    source_sha256: str | None
    warnings: tuple[str, ...]
    errors: tuple[str, ...]
    provenance: tuple[tuple[str, str], ...] = ()

    @property
    def passed(self) -> bool:
        return not self.errors


@dataclass(frozen=True, slots=True)
class FootprintAudit:
    entries: tuple[FootprintAuditEntry, ...]

    @property
    def passed(self) -> bool:
        return bool(self.entries) and all(entry.passed for entry in self.entries)


def audit_resolved_footprints(board: Board, resolver: FootprintResolver) -> FootprintAudit:
    """Resolve every selected footprint and report all gaps in one pass."""

    flat = elaborate(board)
    selected: dict[str, list[tuple[ComponentInstance, PartDefinition]]] = {}
    for component in flat.components:
        part = flat.library[component.part]
        reference = _selected_footprint(component, part)
        if reference is not None:
            selected.setdefault(reference, []).append((component, part))
    entries: list[FootprintAuditEntry] = []
    for reference, uses in sorted(selected.items()):
        errors: list[str] = []
        warnings: tuple[str, ...] = ()
        source_path: str | None = None
        source_sha256: str | None = None
        provenance = ()
        try:
            result = resolver.resolve(reference)
            warnings = result.warnings
            source_path = result.footprint.metadata.get("source_path")
            source_sha256 = result.footprint.metadata.get("source_sha256")
            provenance = tuple(sorted((k, v) for k, v in result.footprint.metadata.items()
                if k in {"resolution", "managed_reference", "source_asset", "module_path", "module_version", "module_checksum"}))
            for component, part in uses:
                try:
                    _validate_footprint_pins(part, component, result.footprint)
                except ValueError as exc:
                    errors.append(str(exc))
        except ValueError as exc:
            errors.append(str(exc))
        entries.append(
            FootprintAuditEntry(
                reference,
                tuple(component.ref for component, _ in uses),
                source_path,
                source_sha256,
                warnings,
                tuple(errors),
                provenance,
            )
        )
    return FootprintAudit(tuple(entries))


def prototype_physicalize(
    board: Board | Design, options: PrototypePhysicalOptions | None = None
) -> PhysicalBoard:
    """Create a deterministic, inspectable PCB draft from electrical IR.

    Components without a selected footprint are omitted.  Selected footprint
    names are retained as provenance, but pad geometry is a generic proxy.
    """

    metadata = {
        "generator": "copperscript-prototype-physicalizer",
        "prototype_footprints": "true",
        "prototype_placement": "true",
        "fabrication_ready": "false",
    }
    return _physicalize(
        board,
        lambda part, component, selected: _proxy_footprint(
            part, component, selected
        ),
        options or PrototypePhysicalOptions(),
        metadata,
    )


def resolved_physicalize(
    board: Board | Design,
    resolver: FootprintResolver,
    options: PrototypePhysicalOptions | None = None,
) -> PhysicalBoard:
    """Create a PCB draft using verified external footprint geometry.

    Placement is still a deterministic inspection grid.  Unlike
    :func:`prototype_physicalize`, this path never creates proxy pad geometry:
    every selected footprint must resolve and its electrical pad numbers must
    exactly match the corresponding part definition.
    """

    cache: dict[str, FootprintImportResult] = {}
    reported_warnings: set[str] = set()
    warning_messages: list[str] = []
    metadata = {
        "generator": "copperscript-resolved-footprint-physicalizer",
        "resolved_footprints": "true",
        "prototype_placement": "true",
        "fabrication_ready": "false",
    }

    def resolve_footprint(
        part: PartDefinition,
        component: ComponentInstance,
        selected: str,
    ) -> PhysicalFootprint:
        result = cache.get(selected)
        if result is None:
            result = resolver.resolve(selected)
            cache[selected] = result
        _validate_footprint_pins(part, component, result.footprint)
        if selected not in reported_warnings:
            reported_warnings.add(selected)
            warning_messages.extend(
                f"{selected}: {warning}" for warning in result.warnings
            )
            if warning_messages:
                metadata["footprint_import_warnings"] = "\n".join(warning_messages)
        return result.footprint

    return _physicalize(
        board,
        resolve_footprint,
        options or PrototypePhysicalOptions(),
        metadata,
    )


_FootprintProvider = Callable[
    [PartDefinition, ComponentInstance, str], PhysicalFootprint
]


def _physicalize(
    board: Board | Design,
    footprint_provider: _FootprintProvider,
    options: PrototypePhysicalOptions,
    metadata: dict[str, str],
) -> PhysicalBoard:
    mechanical = board.mechanical if isinstance(board, Design) else None
    board = board.electrical if isinstance(board, Design) else board
    outline = mechanical.outline if mechanical else BoardOutline.rectangle(
        options.board_width_mm, options.board_height_mm)
    flat = elaborate(board)
    if options.fabrication_profile in {"jlcpcb-four-layer", "jlcpcb-six-layer"}:
        # JLCPCB's published multilayer minimum is 0.09 mm; this is an
        # escape-rule floor, not a recommendation for every signal or a
        # substitute for impedance/current/assembly qualification.
        rules = DesignRules(
            minimum_clearance_nm=nm_from_mm("0.09"),
            minimum_track_width_nm=nm_from_mm("0.09"),
            default_track_width_nm=nm_from_mm("0.20"),
        )
        metadata["fabrication_profile"] = options.fabrication_profile
    else:
        rules = DesignRules()
    if mechanical:
        rules = replace(rules, **mechanical.rule_overrides)
    selected_components = tuple(
        sorted(
            (
                (component, flat.library[component.part], selected)
                for component in flat.components
                if (
                    selected := _selected_footprint(
                        component, flat.library[component.part]
                    )
                )
                is not None
            ),
            key=lambda item: item[0].ref,
        )
    )
    component_index = {
        component.ref: component for component, _, _ in selected_components
    }
    # Keep optional electrical pin names alongside physical pad numbers so
    # editors can reveal useful identities (e.g. ``PA9`` or ``VDD``) without
    # making the backend-neutral footprint geometry depend on a symbol format.
    pad_name_map: dict[str, dict[str, str]] = {}
    for component, part, _ in selected_components:
        by_number: dict[str, list[str]] = {}
        for pin in part.pins.values():
            by_number.setdefault(pin.number, []).append(pin.name)
        if by_number:
            pad_name_map[component.ref] = {
                number: "/".join(dict.fromkeys(names))
                for number, names in sorted(by_number.items())
            }
    if pad_name_map:
        metadata["pad_names"] = json.dumps(pad_name_map, sort_keys=True)
    footprints: dict[str, PhysicalFootprint] = {}
    placements: list[Placement] = []

    xs, ys = [p.x_nm for p in outline.vertices], [p.y_nm for p in outline.vertices]
    if outline.circular_boundary is not None:
        circle = outline.circular_boundary
        xs = [circle.center.x_nm-circle.radius_nm, circle.center.x_nm+circle.radius_nm]
        ys = [circle.center.y_nm-circle.radius_nm, circle.center.y_nm+circle.radius_nm]
    left, top = min(xs) / 1e6, min(ys) / 1e6
    width, height = (max(xs)-min(xs)) / 1e6, (max(ys)-min(ys)) / 1e6
    margin = min(options.margin_mm, min(width, height) / 4)
    usable_width = width - 2 * margin
    usable_height = height - 2 * margin
    rows = max(
        1, (len(selected_components) + options.columns - 1) // options.columns
    )
    for index, (component, part, selected) in enumerate(selected_components):
        footprint = footprint_provider(part, component, selected)
        if not part.assembled and footprint.internal_pad_groups:
            footprint = replace(footprint, name=f"{footprint.name}__unassembled",
                                internal_pad_groups=())
        elif part.internal_pad_groups and part.assembled:
            groups = merge_internal_pad_groups(footprint.internal_pad_groups, part.internal_pad_groups)
            # Proxy geometry has only one land per number. A duplicate-land
            # declaration is a no-op there, never invented extra geometry.
            if metadata.get("prototype_footprints") == "true":
                groups = tuple(g for g in groups if len(g.numbers) > 1)
            # Package facts can differ for components using identical land
            # geometry. Give the derived footprint its own canonical identity.
            suffix = sha256(repr(groups).encode()).hexdigest()[:12]
            footprint = replace(footprint, name=f"{footprint.name}__internal_{suffix}",
                                internal_pad_groups=groups)
        existing = footprints.get(footprint.name)
        if existing is not None and existing != footprint:
            raise ValueError(
                f"footprint ID {footprint.name!r} resolved to conflicting geometry"
            )
        footprints.setdefault(footprint.name, footprint)
        column = index % options.columns
        row = index // options.columns
        x = left + margin + usable_width * (column + 0.5) / options.columns
        y = top + margin + usable_height * (row + 0.5) / rows
        placements.append(
            Placement(
                reference=component.ref,
                footprint=footprint.name,
                position=Point.mm(x, y),
                value=_component_value(component, part),
                source_path=component.ref,
            )
        )

    nets: list[PhysicalNet] = []
    placement_index = {p.reference: p for p in placements}
    for net in sorted(flat.nets, key=lambda item: item.name):
        pad_refs: list[PadReference] = []
        for endpoint in net.endpoints:
            component = component_index.get(endpoint.component)
            if component is None:
                continue
            part = flat.library[component.part]
            pin_number = _physical_pin_number(
                component, part, flat.devices, endpoint.pin
            )
            if pin_number is not None:
                pad_refs.append(PadReference(component.ref, pin_number))
        # An unmentioned alternate numbered terminal inherits its declared
        # internal group's net. Conflicting explicit nets are rejected by IR.
        for reference in tuple(pad_refs):
            fp = footprints[placement_index[reference.component].footprint]
            for group in fp.internal_pad_groups:
                if reference.pad in group.numbers:
                    pad_refs.extend(PadReference(reference.component, n) for n in group.numbers)
        if pad_refs:
            nets.append(PhysicalNet(net.name, tuple(sorted(set(pad_refs)))))

    omitted = sorted(
        component.ref
        for component in flat.components
        if _selected_footprint(component, flat.library[component.part]) is None
    )
    metadata["omitted_components"] = ",".join(omitted)
    (
        regions,
        keepouts,
        placement_rules,
        relative_rules,
        placement_groups,
        net_routing_rules,
        zones,
        via_in_pad_rules,
    ) = _lower_physical_constraints(flat, component_index, metadata, outline)
    attachments = ()
    if mechanical:
        keepouts = (*keepouts, *mechanical.keepouts)
        if len({k.name for k in keepouts}) != len(keepouts):
            raise ValueError("conflicting placement keepout names")
        placement_rules = _bind_profile_connectors(mechanical, flat, footprints, placements, placement_rules)
        from .mechanical_anchors import bind_attachments
        explicit_angles = frozenset(target for c in flat.constraints if c.kind is ConstraintKind.ALLOWED_ORIENTATIONS for target in c.targets)
        placement_rules, attachments = bind_attachments(mechanical.attachments, footprints, placements, placement_rules,
                                                       _merge_rule, explicit_angles=explicit_angles)
        metadata["mechanical_attachment_owners"] = json.dumps({a.reference:a.id for a in attachments},sort_keys=True)
        if mechanical.profiles:
            metadata["mechanical_provenance"] = json.dumps(mechanical_provenance(mechanical), sort_keys=True)
            metadata["mechanical_connector_roles"] = json.dumps({c.reference: c.role for c in mechanical.connectors}, sort_keys=True)
    fixed_rules = {r.reference: r for r in placement_rules}
    placements = [replace(p, position=fixed_rules[p.reference].fixed_position or p.position,
                         rotation_degrees=(fixed_rules[p.reference].fixed_rotation_degrees
                             if fixed_rules[p.reference].fixed_rotation_degrees is not None else p.rotation_degrees),
                         side=fixed_rules[p.reference].side or p.side)
                  if p.reference in fixed_rules else p for p in placements]
    copper_layers = (
        (CopperLayer.FRONT, CopperLayer.BACK)
        if options.copper_layers == 2
        else (
            CopperLayer.FRONT,
            CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2,
            *((CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4) if options.copper_layers == 6 else ()),
            CopperLayer.BACK,
        )
    )
    # A declared source stack-up must name exactly the selected copper layers.
    stackup = (mechanical.stackup.to_stackup(copper_layers)
               if mechanical and mechanical.stackup is not None
               else Stackup(copper_layers=copper_layers))
    return PhysicalBoard(
        name=board.name,
        outline=outline,
        stackup=stackup,
        rules=rules,
        footprints=footprints,
        placements=tuple(placements),
        nets=tuple(nets),
        power_domains=lower_power_domains(flat, nets, _physical_pin_number),
        metadata=metadata,
        regions=regions,
        keepouts=keepouts,
        placement_rules=placement_rules,
        relative_rules=relative_rules,
        placement_groups=placement_groups,
        net_routing_rules=net_routing_rules,
        zones=zones,
        via_in_pad_rules=via_in_pad_rules,
        mechanical_holes=mechanical.holes if mechanical else (),
        copper_keepouts=mechanical.copper_keepouts if mechanical else (),
        datums=mechanical.datums if mechanical else (),
        boundary_edges=mechanical.boundary_edges if mechanical else (),
        attachments=attachments,
        body_overhangs=mechanical.body_overhangs if mechanical else (),
        component_heights=mechanical.component_heights if mechanical else (),
        assembly_envelopes=mechanical.assembly_envelopes if mechanical else (),
        assembly_access=mechanical.assembly_access if mechanical else (),
        mechanical_slots=mechanical.slots if mechanical else (),
        mechanical_references=mechanical.references if mechanical else (),
        match_groups=_lower_match_groups(flat),
        component_hole_clearances=_lower_hole_clearances(flat, footprints, placements, rules, metadata),
    )


def _bind_profile_connectors(mechanical, flat, footprints, placements, rules):
    from .placement import transformed_local_point
    poses = {p.reference: p for p in placements}
    by_reference = {r.reference: r for r in rules}
    components = {c.ref: c for c in flat.components}
    for binding in mechanical.connectors:
        prefix = f"{binding.location}: connector role {binding.role!r}: "
        if binding.reference not in poses:
            raise ValueError(prefix + "bound component needs a selected footprint; it cannot be omitted")
        pose = poses[binding.reference]
        fp = footprints[pose.footprint]
        selected = _selected_footprint(components[binding.reference], flat.library[components[binding.reference].part])
        if binding.footprint is not None and binding.footprint != selected:
            raise ValueError(prefix + f"requires footprint {binding.footprint!r}, got {selected!r}")
        pads = [p for p in fp.pads if p.number == binding.anchor_pad
                and p.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}]
        if len(pads) != 1:
            raise ValueError(prefix + "anchor_pad must identify exactly one electrical physical land")
        zero = replace(pose, position=Point(0, 0), rotation_degrees=binding.rotation, side=binding.side)
        offset = transformed_local_point(zero, pads[0].position)
        origin = Point(binding.position.x_nm-offset.x_nm, binding.position.y_nm-offset.y_nm)
        existing = by_reference.get(binding.reference)
        for key, value in (("fixed_position", origin), ("fixed_rotation_degrees", binding.rotation), ("side", binding.side)):
            old = getattr(existing, key) if existing else None
            if old is not None and old != value:
                raise ValueError(prefix + f"conflicts with existing {key}")
        explicit_angles = any(c.kind is ConstraintKind.ALLOWED_ORIENTATIONS and
                              any(t.partition(".")[0] == binding.reference for t in c.targets) for c in flat.constraints)
        orientations = existing.allowed_orientations if existing and explicit_angles else (binding.rotation,)
        by_reference[binding.reference] = _merge_rule(existing, binding.reference,
            fixed_position=origin, fixed_rotation_degrees=binding.rotation, side=binding.side,
            allowed_orientations=orientations, priority=max(existing.priority if existing else 0, 1000))
    return tuple(by_reference[key] for key in sorted(by_reference))


def _lower_physical_constraints(
    flat: FlatElectricalView,
    components: Mapping[str, ComponentInstance],
    metadata: dict[str, str],
    outline: BoardOutline,
) -> tuple[
    tuple[PlacementRegion, ...],
    tuple[PlacementKeepout, ...],
    tuple[ComponentPlacementRule, ...],
    tuple[RelativePlacementRule, ...],
    tuple[PlacementGroup, ...],
    tuple[NetRoutingRule, ...],
    tuple[CopperZone, ...],
    tuple[PadViaInPadRule, ...],
]:
    regions: list[PlacementRegion] = []
    keepouts: list[PlacementKeepout] = []
    rules: dict[str, ComponentPlacementRule] = {}
    relative: list[RelativePlacementRule] = []
    groups: list[PlacementGroup] = []
    routing_rules: list[NetRoutingRule] = []
    zones: list[CopperZone] = []
    via_in_pad_rules: list[PadViaInPadRule] = []
    skipped: list[str] = []

    def targets(values: tuple[str, ...]) -> tuple[PlacementTarget, ...] | None:
        lowered: list[PlacementTarget] = []
        for value in values:
            reference, separator, pin = value.partition(".")
            component = components.get(reference)
            if component is None:
                skipped.append(value)
                return None
            physical_pad = None
            if separator:
                part = flat.library[component.part]
                physical_pad = _physical_pin_number(
                    component, part, flat.devices, pin
                )
                if physical_pad is None:
                    raise ValueError(
                        f"placement constraint target {value!r} has no physical pad"
                    )
            lowered.append(PlacementTarget(reference, physical_pad))
        return tuple(lowered)

    net_names = {item.name for item in flat.nets}
    for index, constraint in enumerate(flat.constraints):
        if constraint.kind is ConstraintKind.VIA_IN_PAD:
            if len(constraint.targets) != 1:
                raise ValueError("via_in_pad requires exactly one component.pin target")
            lowered = targets(constraint.targets)
            if lowered is None or lowered[0].pad is None:
                raise ValueError("via_in_pad target must resolve to a physical pad")
            parameters = constraint.parameters
            if set(parameters) - {"process", "rows", "columns", "pitch"}:
                raise ValueError("unknown via_in_pad parameter")
            # rows/columns request a required array; without them this stays
            # a permission for the single last-resort plane contact.
            if ("rows" in parameters) != ("columns" in parameters):
                raise ValueError("via_in_pad rows and columns must be given together")
            if "pitch" in parameters and "rows" not in parameters:
                raise ValueError("via_in_pad pitch requires rows and columns")
            if any(type(parameters[name]) is not int or parameters[name] <= 0
                   for name in ("rows", "columns") if name in parameters):
                raise ValueError("via_in_pad rows and columns must be positive integers")
            via_in_pad_rules.append(PadViaInPadRule(
                PadReference(lowered[0].reference, lowered[0].pad),
                str(parameters.get("process", "filled-capped")),
                parameters.get("rows"), parameters.get("columns"),
                _optional_constraint_length(parameters, "pitch"),
            ))
            continue
        if constraint.kind is ConstraintKind.COPPER_ZONE:
            if len(constraint.targets) != 1:
                raise ValueError("copper_zone requires exactly one net target")
            net = constraint.targets[0]
            if net not in net_names:
                raise ValueError(f"copper_zone references unknown net {net!r}")
            parameters = constraint.parameters
            unknown = set(parameters) - {"layers", "inset", "pad_connection", "clearance", "minimum_width", "island_policy",
                                          "region", "polygon_mm", "x", "y", "width", "height", "priority", "reserve_routing"}
            if unknown:
                raise ValueError(f"unknown copper_zone parameter {sorted(unknown)[0]!r}")
            layers = _constraint_layers(parameters.get("layers"))
            if not layers:
                raise ValueError("copper_zone requires at least one copper layer")
            zone_outline = lower_zone_outline(parameters, outline, flat.constraints)
            priority = parameters.get("priority", 0)
            if type(priority) is not int or priority < 0:
                raise ValueError("copper_zone priority must be a nonnegative integer")
            island_policy = IslandPolicy(str(parameters.get("island_policy", "remove_below_area")))
            zone = CopperZone(
                id=constraint.constraint_id or f"zone:{net}:{index}",
                net=net,
                layers=layers,
                outline=zone_outline,
                priority=priority,
                reserve_routing=_constraint_bool(parameters, "reserve_routing", False),
                clearance_nm=_optional_constraint_length(parameters, "clearance"),
                minimum_width_nm=_optional_constraint_length(parameters, "minimum_width") or nm_from_mm("0.25"),
                pad_connection=ZoneConnection(str(parameters.get("pad_connection", "thermal"))),
                island_policy=island_policy,
                minimum_island_area_nm2=(10_000_000_000_000
                    if island_policy is IslandPolicy.REMOVE_BELOW_AREA else None),
            )
            zones.append(zone)
            continue
        if constraint.kind is ConstraintKind.ROUTING:
            if len(constraint.targets) != 1:
                raise ValueError("routing requires exactly one net target")
            net = constraint.targets[0]
            if net not in net_names:
                raise ValueError(f"routing constraint references unknown net {net!r}")
            parameters = constraint.parameters
            routing_rules.append(
                NetRoutingRule(
                    net=net,
                    kind=RouteKind(str(parameters.get("kind", "general"))),
                    priority=_constraint_int(parameters, "priority", 0),
                    width_nm=_optional_constraint_length(parameters, "width"),
                    clearance_nm=_optional_constraint_length(parameters, "clearance"),
                    allowed_layers=_constraint_layers(parameters.get("allowed_layers")),
                    max_vias=_optional_constraint_int(parameters, "max_vias"),
                    max_length_nm=_optional_constraint_length(parameters, "max_length"),
                    differential_partner=_optional_string(parameters, "partner"),
                    pair_gap_nm=_optional_constraint_length(parameters, "pair_gap"),
                    max_skew_nm=_optional_constraint_length(parameters, "max_skew"),
                    topology=str(parameters.get("topology", "point_to_point")),
                    target_impedance_ohms=_optional_constraint_int(parameters, "target_impedance_ohms"),
                    maximum_uncoupled_length_nm=_optional_constraint_length(parameters, "maximum_uncoupled_length"),
                    maximum_stub_length_nm=_optional_constraint_length(parameters, "maximum_stub_length"),
                    tuning_amplitude_limit_nm=_optional_constraint_length(parameters, "tuning_amplitude_limit"),
                    require_return_vias=_constraint_bool(parameters, "require_return_vias", False),
                    reserve_corridor=_reserve_corridor(net, parameters),
                    return_via_net=_optional_string(parameters, "return_via_net"),
                    maximum_return_via_distance_nm=_optional_constraint_length(parameters, "maximum_return_via_distance"),
                    impedance_evidence_digest=_optional_string(parameters, "impedance_evidence_digest"),
                    return_via_policy=ReturnViaPolicy(str(parameters.get("return_via_policy", "always"))),
                    shared_reference_layer=(CopperLayer(str(parameters["shared_reference_layer"]))
                                            if "shared_reference_layer" in parameters else None),
                    target_single_ended_ohms=_optional_constraint_int(parameters, "target_single_ended_ohms"),
                    impedance_tolerance_percent=_optional_percent(parameters, "impedance_tolerance_percent"),
                    layer_group=_optional_string(parameters, "layer_group"),
                    breakout_length_nm=_optional_constraint_length(parameters, "breakout_length"),
                    breakout_width_nm=_optional_constraint_length(parameters, "breakout_width"),
                    breakout_gap_nm=_optional_constraint_length(parameters, "breakout_gap"),
                    breakout_clearance_nm=_optional_constraint_length(parameters, "breakout_clearance"),
                    tuning_style=TuningStyle(str(parameters.get("tuning_style", "bumps"))),
                    tuning_spacing_nm=_optional_constraint_length(parameters, "tuning_spacing"),
                )
            )
            continue
        if constraint.kind is ConstraintKind.LENGTH_MATCH:
            continue  # net groups lower separately; see _lower_match_groups
        if constraint.kind is ConstraintKind.HOLE_CLEARANCE:
            continue  # needs footprint geometry; see _lower_hole_clearances
        lowered_targets = targets(constraint.targets)
        if lowered_targets is None:
            continue
        parameters = constraint.parameters
        if constraint.kind in {ConstraintKind.MAX_DISTANCE, ConstraintKind.MIN_DISTANCE}:
            distance = _constraint_length(parameters, "distance")
            relative.append(
                RelativePlacementRule(
                    RelativePlacementKind(constraint.kind.value),
                    lowered_targets,
                    distance_nm=distance,
                    weight=_constraint_int(parameters, "weight", 10),
                )
            )
            groups.append(
                PlacementGroup(
                    f"proximity:{index}",
                    tuple(target.reference for target in lowered_targets),
                    anchor=lowered_targets[-1].reference,
                    priority=_constraint_int(parameters, "priority", 50),
                    source="constraint",
                )
            )
        elif constraint.kind is ConstraintKind.PLACEMENT_REGION:
            name = str(parameters.get("name", f"region:{index}"))
            region = PlacementRegion(
                name,
                BoardOutline.rectangle(
                    _constraint_length(parameters, "width") / 1_000_000,
                    _constraint_length(parameters, "height") / 1_000_000,
                    origin=Point(
                        _constraint_length(parameters, "x"),
                        _constraint_length(parameters, "y"),
                    ),
                ),
                _constraint_side(parameters.get("side")),
            )
            regions.append(region)
            for target in lowered_targets:
                rules[target.reference] = _merge_rule(
                    rules.get(target.reference), target.reference, region=name
                )
        elif constraint.kind is ConstraintKind.KEEPOUT:
            keepouts.append(
                PlacementKeepout(
                    str(parameters.get("name", f"keepout:{index}")),
                    BoardOutline.rectangle(
                        _constraint_length(parameters, "width") / 1_000_000,
                        _constraint_length(parameters, "height") / 1_000_000,
                        origin=Point(
                            _constraint_length(parameters, "x"),
                            _constraint_length(parameters, "y"),
                        ),
                    ),
                    _constraint_side(parameters.get("side")),
                    (
                        _constraint_length(parameters, "maximum_height")
                        if "maximum_height" in parameters
                        else None
                    ),
                )
            )
        elif constraint.kind is ConstraintKind.FIXED_PLACEMENT:
            if len(lowered_targets) != 1:
                raise ValueError("fixed_placement requires exactly one component")
            target = lowered_targets[0]
            if ("x" in parameters) != ("y" in parameters):
                raise ValueError("fixed_placement position requires both x and y")
            if not set(parameters) & {"x", "rotation", "side"}:
                raise ValueError("fixed_placement requires position, rotation or side")
            if target.pad is not None:
                raise ValueError("fixed_placement requires a component, not a pin")
            rotation = Decimal(str(parameters["rotation"])) if "rotation" in parameters else None
            # Explicit orientation constraints must not be silently overridden
            # by a fixed pose merely because declarations occur in that order.
            existing = rules.get(target.reference)
            orientations = ((rotation,) if rotation is not None else
                            existing.allowed_orientations if existing else (0, 90, 180, 270))
            for c in flat.constraints:
                if c.kind is ConstraintKind.ALLOWED_ORIENTATIONS and any(
                        t.reference == target.reference for t in (targets(c.targets) or ())):
                    orientations = _constraint_orientations(c.parameters.get("values"))
            rules[target.reference] = _merge_rule(
                rules.get(target.reference),
                target.reference,
                fixed_position=Point(
                    _constraint_length(parameters, "x"),
                    _constraint_length(parameters, "y"),
                ) if "x" in parameters else None,
                fixed_rotation_degrees=rotation,
                allowed_orientations=orientations,
                edge_clearance_nm=_optional_constraint_length(parameters, "edge_clearance"),
                side=_constraint_side(parameters.get("side")),
                priority=_constraint_int(parameters, "priority", 1000),
            )
        elif constraint.kind is ConstraintKind.ALLOWED_ORIENTATIONS:
            orientations = _constraint_orientations(parameters.get("values"))
            for target in lowered_targets:
                rules[target.reference] = _merge_rule(
                    rules.get(target.reference),
                    target.reference,
                    allowed_orientations=orientations,
                )
        elif constraint.kind is ConstraintKind.ALIGN:
            relative.append(
                RelativePlacementRule(
                    RelativePlacementKind.ALIGN,
                    lowered_targets,
                    axis=AlignmentAxis(str(parameters.get("axis", "x"))),
                    tolerance_nm=(
                        _constraint_length(parameters, "tolerance")
                        if "tolerance" in parameters
                        else 0
                    ),
                    weight=_constraint_int(parameters, "weight", 5),
                )
            )
        elif constraint.kind is ConstraintKind.PLACEMENT_GROUP:
            references = tuple(target.reference for target in lowered_targets)
            groups.append(
                PlacementGroup(
                    str(parameters.get("name", f"group:{index}")),
                    references,
                    anchor=str(parameters.get("anchor", references[0])),
                    priority=_constraint_int(parameters, "priority", 0),
                    source="constraint",
                )
            )

    for module in flat.module_instances:
        members = tuple(
            sorted(
                reference
                for reference in components
                if reference.startswith(f"{module.path}/")
            )
        )
        if members:
            groups.append(
                PlacementGroup(
                    f"module:{module.path}",
                    members,
                    anchor=max(members, key=lambda item: len(flat.library[components[item].part].pins)),
                    priority=100,
                    source="hierarchy",
                )
            )

    for interface in flat.interfaces:
        members = tuple(sorted(reference for reference in interface.bindings if reference in components))
        if len(members) >= 2:
            groups.append(
                PlacementGroup(
                    f"interface:{interface.name}",
                    members,
                    anchor=max(members, key=lambda item: len(flat.library[components[item].part].pins)),
                    priority=25,
                    source="interface",
                )
            )

    if skipped:
        metadata["omitted_constraint_targets"] = ",".join(sorted(set(skipped)))
    return (
        tuple(regions),
        tuple(keepouts),
        tuple(rules[reference] for reference in sorted(rules)),
        tuple(relative),
        _unique_groups(groups),
        tuple(routing_rules),
        tuple(zones),
        tuple(via_in_pad_rules),
    )


def _lower_match_groups(flat: FlatElectricalView) -> tuple[NetMatchGroup, ...]:
    """Lower ``length_match`` constraints to physical ``NetMatchGroup`` IR."""

    net_names = {item.name for item in flat.nets}
    groups: list[NetMatchGroup] = []
    owner: dict[str, str] = {}
    for index, constraint in enumerate(flat.constraints):
        if constraint.kind is not ConstraintKind.LENGTH_MATCH:
            continue
        origin = constraint.origins[0] if constraint.origins else "length_match"
        try:
            group_id = constraint.constraint_id or f"length_match:{index}"
            if any(group.id == group_id for group in groups):
                raise ValueError(f"duplicate length_match id {group_id!r}")
            for net in constraint.targets:
                if net not in net_names:
                    raise ValueError(f"length_match references unknown net {net!r}")
                if net in owner:
                    raise ValueError(f"net {net!r} already belongs to length_match group {owner[net]!r}")
                owner[net] = group_id
            groups.append(NetMatchGroup(group_id, tuple(constraint.targets),
                                        _constraint_length(constraint.parameters, "max_skew")))
        except ValueError as exc:
            raise ValueError(f"{origin}: {exc}") from exc
    return tuple(groups)


def _lower_hole_clearances(
    flat: FlatElectricalView,
    footprints: Mapping[str, PhysicalFootprint],
    placements: list[Placement],
    rules: DesignRules,
    metadata: Mapping[str, str],
) -> tuple[ComponentHoleClearance, ...]:
    """Lower ``hole_clearance`` constraints once footprint geometry is known.

    Errors carry the constraint's source location and compile error code.
    Proxy footprints have no holes, so inspection-only proxy boards keep the
    board rule everywhere instead of failing.
    """

    if metadata.get("prototype_footprints") == "true":
        return ()
    poses = {placement.reference: placement for placement in placements}
    components = {component.ref for component in flat.components}
    lowered: dict[str, ComponentHoleClearance] = {}
    for constraint in flat.constraints:
        if constraint.kind is not ConstraintKind.HOLE_CLEARANCE:
            continue
        origin = constraint.origins[0] if constraint.origins else "hole_clearance"
        reference = constraint.targets[0]
        if reference not in components:
            raise ValueError(f"{origin}: CMP113: hole_clearance target {reference!r} is not a component")
        if reference in lowered:
            raise ValueError(f"{origin}: CMP114: component {reference!r} already has a hole_clearance constraint")
        pose = poses.get(reference)
        if pose is None or not any(pad.kind is PadKind.NON_PLATED_THROUGH_HOLE
                                   for pad in footprints[pose.footprint].pads):
            raise ValueError(f"{origin}: CMP115: component {reference!r} has no footprint with "
                             "non-plated holes for hole_clearance to apply to")
        clearance = _constraint_length(constraint.parameters, "clearance")
        if clearance > rules.minimum_hole_clearance_nm:
            raise ValueError(f"{origin}: CMP116: hole_clearance for {reference!r} may only relax "
                             f"(not exceed) the board minimum hole clearance "
                             f"({rules.minimum_hole_clearance_nm} nm)")
        lowered[reference] = ComponentHoleClearance(
            reference, clearance, str(constraint.parameters["reason"]))
    return tuple(lowered[reference] for reference in sorted(lowered))


def _optional_percent(parameters: Mapping[str, object], name: str) -> Decimal | None:
    if name not in parameters:
        return None
    value = parameters[name]
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"routing constraint parameter {name!r} must be numeric")
    return Decimal(str(value))


def _merge_rule(
    existing: ComponentPlacementRule | None,
    reference: str,
    **changes: object,
) -> ComponentPlacementRule:
    if existing is not None:
        for key in ("fixed_position", "fixed_rotation_degrees", "side"):
            if key in changes and getattr(existing, key) is not None:
                if changes[key] is None:
                    changes.pop(key)  # An unspecified value never erases an owned rule.
                elif getattr(existing, key) != changes[key]:
                    raise ValueError(f"conflicting {key} constraints for {reference!r}")
    return ComponentPlacementRule(reference, **changes) if existing is None else replace(existing, **changes)


def _constraint_length(parameters: Mapping[str, object], name: str) -> int:
    value = parameters.get(name)
    if not isinstance(value, Length):
        raise ValueError(f"placement constraint parameter {name!r} must be a length")
    return int(value.in_unit("mm") * Decimal(1_000_000))


def _constraint_int(parameters: Mapping[str, object], name: str, default: int) -> int:
    value = parameters.get(name, default)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"placement constraint parameter {name!r} must be numeric")
    return int(value)


def _optional_constraint_int(parameters: Mapping[str, object], name: str) -> int | None:
    return _constraint_int(parameters, name, 0) if name in parameters else None


def _optional_constraint_length(parameters: Mapping[str, object], name: str) -> int | None:
    return _constraint_length(parameters, name) if name in parameters else None


def _optional_string(parameters: Mapping[str, object], name: str) -> str | None:
    value = parameters.get(name)
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError(f"routing constraint parameter {name!r} must be a string")
    return value


def _constraint_bool(parameters: Mapping[str, object], name: str, default: bool) -> bool:
    value = parameters.get(name, default)
    if not isinstance(value, bool):
        raise ValueError(f"routing constraint parameter {name!r} must be boolean")
    return value


def _reserve_corridor(net: str, parameters: Mapping[str, object]) -> bool:
    reserve = _constraint_bool(parameters, "reserve_corridor", False)
    if reserve and "partner" not in parameters:
        raise ValueError(
            f"routing constraint for {net!r}: reserve_corridor requires a differential partner"
        )
    return reserve


def _constraint_layers(value: object) -> tuple[CopperLayer, ...]:
    if value is None:
        return ()
    if not isinstance(value, str):
        raise ValueError("routing allowed_layers must be a comma-separated string")
    try:
        return tuple(CopperLayer(item.strip()) for item in value.split(",") if item.strip())
    except ValueError as exc:
        raise ValueError("routing allowed_layers contains an unknown copper layer") from exc


def _constraint_side(value: object) -> BoardSide | None:
    if value is None or value == "both":
        return None
    if not isinstance(value, str):
        raise ValueError("placement constraint side must be front, back, or both")
    return BoardSide(value)


def _constraint_orientations(value: object) -> tuple[Decimal, ...]:
    if not isinstance(value, str):
        raise ValueError("allowed orientations must be a comma-separated string")
    try:
        result = tuple(Decimal(item.strip()) for item in value.split(",") if item.strip())
    except InvalidOperation as exc:
        raise ValueError("allowed orientations contain a non-numeric value") from exc
    if not result:
        raise ValueError("allowed orientations cannot be empty")
    return result


def _unique_groups(groups: list[PlacementGroup]) -> tuple[PlacementGroup, ...]:
    names: dict[str, int] = {}
    result: list[PlacementGroup] = []
    for group in groups:
        count = names.get(group.name, 0)
        names[group.name] = count + 1
        result.append(group if count == 0 else replace(group, name=f"{group.name}:{count}"))
    return tuple(result)


def _proxy_footprint(
    part: PartDefinition,
    component: ComponentInstance,
    selected: str,
) -> PhysicalFootprint:
    ordered_pins = tuple(sorted(part.pins.values(), key=lambda pin: _natural(pin.number)))
    count = len(ordered_pins)
    digest = sha256(
        f"{part.name}\0{selected}\0{','.join(pin.number for pin in ordered_pins)}".encode()
    ).hexdigest()[:10]
    name = f"CopperScript/{_safe(selected)}_{digest}"

    if count == 2:
        positions = (Point.mm(-1.0, 0), Point.mm(1.0, 0))
        body_size = Size.mm(1.6, 0.8)
    else:
        left_count = (count + 1) // 2
        right_count = count - left_count
        pitch = 1.27
        positions = tuple(
            Point.mm(-2.5, (index - (left_count - 1) / 2) * pitch)
            for index in range(left_count)
        ) + tuple(
            Point.mm(2.5, (index - (right_count - 1) / 2) * pitch)
            for index in range(right_count)
        )
        body_size = Size.mm(4.0, max(3.0, max(left_count, right_count) * pitch))

    pads = tuple(
        FootprintPad(pin.number, position, Size.mm(1.0, 1.0))
        for pin, position in zip(ordered_pins, positions, strict=True)
    )
    return PhysicalFootprint(
        name=name,
        pads=pads,
        body_size=body_size,
        source_library_id=selected,
    )


def _selected_footprint(
    component: ComponentInstance, part: PartDefinition
) -> str | None:
    if component.footprint:
        return component.footprint
    return part.footprints[0] if part.footprints else None


def _validate_footprint_pins(
    part: PartDefinition,
    component: ComponentInstance,
    footprint: PhysicalFootprint,
) -> None:
    part_numbers = {pin.number for pin in part.pins.values()}
    footprint_numbers = {pad.number for pad in footprint.pads if pad.number}
    missing = sorted(part_numbers - footprint_numbers, key=_natural)
    extra = sorted(footprint_numbers - part_numbers, key=_natural)
    if missing or extra:
        details: list[str] = []
        if missing:
            details.append(f"missing pads {', '.join(missing)}")
        if extra:
            details.append(f"unknown pads {', '.join(extra)}")
        raise ValueError(
            f"component {component.ref!r} part {part.name!r} is incompatible with "
            f"footprint {footprint.source_library_id or footprint.name!r}: "
            + "; ".join(details)
        )


def _physical_pin_number(
    component: ComponentInstance,
    part: PartDefinition,
    devices: Mapping[str, DeviceDefinition],
    endpoint_name: str,
) -> str | None:
    direct = part.pins.get(endpoint_name)
    if direct is not None:
        return direct.number

    unit_name, separator, terminal_name = endpoint_name.partition(".")
    device = devices.get(part.device or "")
    unit = device.units.get(unit_name) if device is not None and separator else None
    terminal = unit.terminals.get(terminal_name) if unit is not None else None
    if terminal is None:
        return None
    matches = [pin.number for pin in pins_bonded_to_pad(component, part, device, terminal.pad)]
    return matches[0] if len(matches) == 1 else None


def _component_value(component: ComponentInstance, part: PartDefinition) -> str:
    if component.value is None:
        return part.name.rsplit(".", 1)[-1]
    if isinstance(component.value, Quantity):
        return str(component.value)
    return component.value


def _natural(value: str) -> tuple[tuple[int, object], ...]:
    return tuple(
        (0, int(piece)) if piece.isdigit() else (1, piece.casefold())
        for piece in re.split(r"(\d+)", value)
        if piece
    )


def _safe(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", value).strip("_") or "Footprint"
