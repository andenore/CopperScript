"""CopperScript compiler: parsing and lowering to hierarchical electrical IR."""

from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import dataclass, field, replace
from pathlib import Path
import re
from typing import Callable, Mapping

from .library import LIBRARIES, library_factory
from .design import Design, lower_mechanical
from .mechanical_profiles import MechanicalProfileDefinition
from .pad_connections import InternalPadGroup
from .modes import active_bonded_pads, condition_active, effective_modes
from .model import (
    BondDefinition,
    Board,
    ComponentInstance,
    Constraint,
    ConstraintKind,
    Dependency,
    DeviceDefinition,
    DevicePadDefinition,
    Interface,
    Condition,
    ConnectionPolicy,
    Direction,
    DriveMode,
    ElectricalProfile,
    FunctionalUnitDefinition,
    GroupKind,
    ModuleDefinition,
    ModuleInstance,
    MuxOption,
    Net,
    PartDefinition,
    ModeGroupDefinition,
    PackagePinDefinition,
    PadSetDefinition,
    PeripheralDefinition,
    PeripheralSelection,
    PeripheralSignalDefinition,
    PeripheralSignalSelection,
    PinType,
    PowerDomainDefinition,
    PowerRailState,
    PowerState,
    QuantityRange,
    RelativeVoltage,
    RouteRule,
    SelectorScheme,
    SelectionUsage,
    SignalDomain,
    SignalGroupDefinition,
    SourceReference,
    Supply,
    TerminalBinding,
    UnpoweredBehavior,
    ep,
)
from .parser import parse
from .packages import PackageResolver, ResolvedPackage
from .constraint_coverage import ConstraintMode
from .quantities import (
    Capacitance,
    Current,
    Frequency,
    Impedance,
    Inductance,
    Length,
    Quantity,
    Resistance,
    Voltage,
)
from .syntax import (
    ComponentDecl,
    ConfigurationDecl,
    ConstraintDecl,
    CopperScriptError,
    Document,
    DevicePropertyDecl,
    InterfaceDecl,
    ModuleInstanceDecl,
    MechanicalProfileUseDecl,
    MuxDecl,
    NetDecl,
    PortDecl,
    PartPropertyDecl,
    PadDecl,
    PeripheralDecl,
    PowerDomainDecl,
    PowerStateDecl,
    PinDecl,
    RawQuantity,
    ResourceDecl,
    UnitDecl,
    SignalGroupDecl,
    ModeGroupDecl,
    PadSetDecl,
    RouteRuleDecl,
    Scalar,
    SourceLocation,
    SupplyDecl,
)


LibraryFactory = Callable[[], dict[str, PartDefinition]]

QUANTITY_TYPES: dict[str, type[Quantity]] = {
    unit: quantity_type
    for quantity_type in (
        Voltage,
        Impedance,
        Resistance,
        Capacitance,
        Inductance,
        Length,
        Current,
        Frequency,
    )
    for unit in quantity_type.UNITS
}


def _resolve_board_imports(
    document: Document,
    *,
    locked: bool = False,
    offline: bool = False,
) -> PackageContents:
    """Resolve a board's packages once for both electrical and mechanical lowering."""

    filename = document.location.filename
    if document.kind != "board":
        _error("CMP019", "compiler entry source must declare a board", document.location)
    if document.imports and filename.startswith("<"):
        _error("CMP020", "package imports require a source filename", document.location)
    if document.imports:
        resolver = PackageResolver.for_source(
            Path(filename).resolve(), document.location, locked=locked, offline=offline
        )
        imported = _load_imports(document, resolver, ())
    else:
        imported = PackageContents({}, {}, {}, ())
    return imported


def compile_design_source(source: str, filename: str = "<memory>", *,
                          locked: bool = False, offline: bool = False) -> Design:
    """Compile a complete design, with mechanics separate from electrical IR."""
    document = parse(source, filename)
    imported = _resolve_board_imports(document, locked=locked, offline=offline)
    mechanical = lower_mechanical(document, imported.profiles)
    electrical = _compile_board(document, imported.modules, imported.parts,
                                imported.devices, imported.dependencies)
    if mechanical:
        from .elaborate import elaborate
        references = {c.ref for c in elaborate(electrical).components}
        for connector in (*mechanical.connectors, *mechanical.attachments, *mechanical.body_overhangs, *mechanical.component_heights, *mechanical.assembly_access):
            if connector.reference not in references:
                _error("MEC005", f"mechanical binding references unknown component {connector.reference!r}", getattr(connector,'location',document.location))
    return Design(electrical, mechanical)


def compile_source(source: str, filename: str = "<memory>", *,
                   locked: bool = False, offline: bool = False) -> Board:
    """Electrical projection of a validated design, for ERC/schematics/simulation."""
    return compile_design_source(source, filename, locked=locked, offline=offline).electrical


def compile_design_file(path: str | Path, *, locked: bool = False, offline: bool = False) -> Design:
    source_path = Path(path).resolve()
    try:
        source = source_path.read_text(encoding="utf-8")
    except OSError as exc:
        _error("CMP001", str(exc), SourceLocation(str(source_path), 0, 1, 1))
    return compile_design_source(source, str(source_path), locked=locked, offline=offline)


def compile_file(path: str | Path, *, locked: bool = False, offline: bool = False) -> Board:
    return compile_design_file(path, locked=locked, offline=offline).electrical


def lower(document: Document) -> Board:
    """Lower an import-free board syntax tree."""

    if document.kind != "board":
        _error("CMP019", "lowering entry must declare a board", document.location)
    if document.imports:
        _error("CMP020", "lower() cannot resolve package imports", document.location)
    lower_mechanical(document)
    return _compile_board(document, {}, {}, {}, ())


def _read_document(path: Path) -> Document:
    try:
        source = path.read_text(encoding="utf-8")
    except OSError as exc:
        location = SourceLocation(str(path), 0, 1, 1)
        raise CopperScriptError("CMP001", f"cannot read source: {exc}", location) from exc
    return parse(source, str(path))


@dataclass(frozen=True, slots=True)
class PackageContents:
    parts: dict[str, PartDefinition]
    devices: dict[str, DeviceDefinition]
    modules: dict[str, ModuleDefinition]
    dependencies: tuple[Dependency, ...]
    profiles: dict[str, MechanicalProfileDefinition] = field(default_factory=dict)


def _load_imports(
    document: Document,
    resolver: PackageResolver,
    stack: tuple[str, ...],
    namespace_prefix: str = "",
) -> PackageContents:
    parts: dict[str, PartDefinition] = {}
    devices: dict[str, DeviceDefinition] = {}
    modules: dict[str, ModuleDefinition] = {}
    profiles: dict[str, MechanicalProfileDefinition] = {}
    dependencies: list[Dependency] = []
    aliases: set[str] = set()
    for declaration in document.imports:
        if declaration.alias in aliases:
            _error("CMP039", f"duplicate import alias {declaration.alias!r}", declaration.location)
        aliases.add(declaration.alias)
        resolved = resolver.resolve(declaration.path, declaration.location)
        if resolved.import_path in stack:
            chain = " -> ".join((*stack, resolved.import_path))
            _error("CMP021", f"cyclic package import: {chain}", declaration.location)
        namespace = (
            f"{namespace_prefix}.{declaration.alias}"
            if namespace_prefix
            else declaration.alias
        )
        package = _load_package(
            resolved, namespace, resolver, (*stack, resolved.import_path)
        )
        _merge_library(parts, package.parts, declaration.location)
        _merge_devices(devices, package.devices, declaration.location)
        for definition in package.modules.values():
            _register_module(modules, definition, declaration.location)
        _merge_profiles(profiles, package.profiles, declaration.location)
        dependencies.extend(package.dependencies)
        dependencies.append(
            Dependency(
                resolved.import_path,
                resolved.module_path,
                resolved.version,
                resolved.checksum,
            )
        )
    return PackageContents(parts, devices, modules, _unique_dependencies(dependencies), profiles)


def _load_package(
    resolved: ResolvedPackage,
    namespace: str,
    resolver: PackageResolver,
    stack: tuple[str, ...],
) -> PackageContents:
    source_paths = sorted(resolved.directory.glob("*.copper"))
    if not source_paths:
        location = SourceLocation(str(resolved.directory), 0, 1, 1)
        _error("CMP022", f"package {resolved.import_path!r} exports no .copper files", location)
    documents = [_read_document(path) for path in source_paths]
    for document in documents:
        if document.kind not in {"device", "part", "module", "board_profile"}:
            _error(
                "CMP022",
                f"package source declares a {document.kind}, expected device, part, module, or board_profile",
                document.location,
            )

    device_documents = {
        document.name: document for document in documents if document.kind == "device"
    }
    part_documents = {document.name: document for document in documents if document.kind == "part"}
    module_documents = {
        document.name: document for document in documents if document.kind == "module"
    }
    profile_documents = {d.name: d for d in documents if d.kind == "board_profile"}
    if len(profile_documents) != sum(d.kind == "board_profile" for d in documents):
        _error("MEC004", "package contains duplicate board profile names", documents[0].location)
    if len(part_documents) != sum(document.kind == "part" for document in documents):
        _error("CMP040", "package contains duplicate part names", documents[0].location)
    if len(module_documents) != sum(document.kind == "module" for document in documents):
        _error("CMP041", "package contains duplicate module names", documents[0].location)

    if len(device_documents) != sum(document.kind == "device" for document in documents):
        _error("CMP052", "package contains duplicate device names", documents[0].location)

    devices = {
        f"{namespace}.{name}": _compile_device(document, f"{namespace}.{name}")
        for name, document in device_documents.items()
    }

    parts = {
        f"{namespace}.{name}": _compile_part(
            document,
            f"{namespace}.{name}",
            lambda raw_name: (
                f"{namespace}.{raw_name}" if raw_name in device_documents else raw_name
            ),
            devices,
        )
        for name, document in part_documents.items()
    }
    for name, document in part_documents.items():
        key = f"{namespace}.{name}"
        parts[key] = replace(parts[key], footprints=tuple(
            resolver.qualify_footprint(ref, document.location) for ref in parts[key].footprints))
    modules: dict[str, ModuleDefinition] = {}
    profiles: dict[str, MechanicalProfileDefinition] = {}
    dependencies: list[Dependency] = []
    import_contexts: dict[str, PackageContents] = {}
    for name, document in {**module_documents, **{f"profile:{k}": v for k, v in profile_documents.items()}}.items():
        imported = _load_imports(document, resolver, stack, namespace)
        import_contexts[name] = imported
        _merge_library(parts, imported.parts, document.location)
        _merge_devices(devices, imported.devices, document.location)
        for definition in imported.modules.values():
            _register_module(modules, definition, document.location)
        _merge_profiles(profiles, imported.profiles, document.location)
        dependencies.extend(imported.dependencies)

    compiling: list[str] = []

    def compile_local(name: str) -> ModuleDefinition:
        qualified_name = f"{namespace}.{name}"
        existing = modules.get(qualified_name)
        if existing is not None:
            return existing
        if name in compiling:
            cycle = " -> ".join((*compiling, name))
            _error("CMP021", f"cyclic module dependency: {cycle}", module_documents[name].location)
        compiling.append(name)
        document = module_documents[name]
        imported = import_contexts[name]
        aliases = {
            declaration.alias: f"{namespace}.{declaration.alias}"
            for declaration in document.imports
        }

        def resolve_module(raw_name: str) -> str:
            if "." in raw_name:
                head, tail = raw_name.split(".", 1)
                return f"{aliases.get(head, head)}.{tail}"
            if raw_name in module_documents:
                compile_local(raw_name)
                return f"{namespace}.{raw_name}"
            return raw_name

        def resolve_part(raw_name: str) -> str:
            if "." in raw_name:
                head, tail = raw_name.split(".", 1)
                return f"{aliases.get(head, head)}.{tail}"
            if raw_name in part_documents:
                return f"{namespace}.{raw_name}"
            return raw_name

        available_modules = dict(modules)
        for local_dependency in {
            declaration.module
            for declaration in document.declarations
            if isinstance(declaration, ModuleInstanceDecl)
            and "." not in declaration.module
            and declaration.module in module_documents
        }:
            definition = compile_local(local_dependency)
            available_modules[definition.name] = definition
        extra_parts = dict(parts)
        _merge_library(extra_parts, imported.parts, document.location)
        definition = _compile_module(
            document,
            {**available_modules, **imported.modules},
            extra_parts,
            qualified_name,
            resolve_part,
            resolve_module,
            devices,
        )
        definition = replace(definition, components=tuple(
            replace(component, footprint=resolver.qualify_footprint(component.footprint, document.location))
            if component.footprint else component for component in definition.components))
        _register_module(modules, definition, document.location)
        compiling.pop()
        return definition

    for module_name in module_documents:
        compile_local(module_name)
    for name, document in profile_documents.items():
        aliases = {d.alias: f"{namespace}.{d.alias}" for d in document.imports}
        items = []
        for item in document.declarations:
            if isinstance(item, MechanicalProfileUseDecl):
                raw = item.profile
                if "." in raw:
                    head, tail = raw.split(".", 1)
                    qualified = f"{aliases.get(head, head)}.{tail}"
                else:
                    qualified = f"{namespace}.{raw}"
                item = replace(item, profile=qualified)
            items.append(item)
        definition = MechanicalProfileDefinition(f"{namespace}.{name}", document.location, tuple(items))
        _merge_profiles(profiles, {definition.name: definition}, document.location)
    return PackageContents(parts, devices, modules, _unique_dependencies(dependencies), profiles)


def _merge_profiles(profiles, additions, location):
    for name, definition in additions.items():
        if name in profiles and profiles[name] != definition:
            _error("MEC004", f"conflicting board profile {name!r}", location)
        profiles[name] = definition


def _unique_dependencies(dependencies: list[Dependency]) -> tuple[Dependency, ...]:
    unique = {
        (item.import_path, item.module_path, item.version, item.checksum): item
        for item in dependencies
    }
    return tuple(unique[key] for key in sorted(unique))


def _register_module(
    modules: dict[str, ModuleDefinition],
    definition: ModuleDefinition,
    location: SourceLocation,
) -> None:
    existing = modules.get(definition.name)
    if existing is not None and existing != definition:
        _error("CMP023", f"conflicting definitions for module {definition.name!r}", location)
    modules.setdefault(definition.name, definition)


def _compile_board(
    document: Document,
    modules: dict[str, ModuleDefinition],
    imported_parts: Mapping[str, PartDefinition],
    imported_devices: Mapping[str, DeviceDefinition],
    dependencies: tuple[Dependency, ...],
) -> Board:
    body, ports, declarations = _lower_unit(
        document, imported_parts, imported_devices=imported_devices
    )
    if ports:
        _error("CMP024", "ports may only be declared inside a module", document.location)
    instances = _validate_instances(body, declarations, modules)
    _validate_hole_clearance_targets(document, body, instances, modules)
    for net in body.nets:
        for endpoint in net.endpoints:
            if endpoint.component == "port":
                _error("CMP025", "board nets cannot reference module boundary ports", document.location)
    return Board(
        name=body.name,
        library=body.library,
        components=body.components,
        nets=body.nets,
        supplies=body.supplies,
        interfaces=body.interfaces,
        constraints=body.constraints,
        module_instances=instances,
        module_definitions=modules,
        dependencies=dependencies,
        devices=body.devices,
        peripheral_selections=body.peripheral_selections,
        power_states=body.power_states,
    )


def _compile_module(
    document: Document,
    modules: dict[str, ModuleDefinition],
    imported_parts: Mapping[str, PartDefinition] | None = None,
    name: str | None = None,
    resolve_part: Callable[[str], str] = lambda value: value,
    resolve_module: Callable[[str], str] = lambda value: value,
    imported_devices: Mapping[str, DeviceDefinition] | None = None,
) -> ModuleDefinition:
    body, ports, declarations = _lower_unit(
        document,
        imported_parts or {},
        resolve_part,
        resolve_module,
        imported_devices or {},
    )
    _validate_module_ports(body, ports, document.location)
    if body.power_states:
        _error("CMP072", "power states may only be declared on a board", document.location)
    instances = _validate_instances(body, declarations, modules)
    _validate_hole_clearance_targets(document, body, instances, modules)
    return ModuleDefinition(
        name=name or document.name,
        ports=ports,
        library=body.library,
        components=body.components,
        module_instances=instances,
        nets=body.nets,
        supplies=body.supplies,
        interfaces=body.interfaces,
        constraints=body.constraints,
        devices=body.devices,
        peripheral_selections=body.peripheral_selections,
    )


def _lower_unit(
    document: Document,
    imported_parts: Mapping[str, PartDefinition] | None = None,
    resolve_part: Callable[[str], str] = lambda value: value,
    resolve_module: Callable[[str], str] = lambda value: value,
    imported_devices: Mapping[str, DeviceDefinition] | None = None,
) -> tuple[Board, dict[str, PinType], tuple[ModuleInstanceDecl, ...]]:
    library = _load_libraries(document)
    _merge_library(library, imported_parts or {}, document.location)
    devices = dict(imported_devices or {})
    components: list[ComponentInstance] = []
    ports: dict[str, PinType] = {}
    module_instances: list[ModuleInstanceDecl] = []
    nets: list[Net] = []
    supplies: list[Supply] = []
    interfaces: list[Interface] = []
    constraints: list[Constraint] = []
    configurations: list[ConfigurationDecl] = []
    power_states: list[PowerState] = []

    for declaration in document.declarations:
        if isinstance(declaration, ComponentDecl):
            if declaration.ref == "port":
                _error("CMP026", "'port' is reserved and cannot be a component reference", declaration.location)
            components.append(_component(declaration, resolve_part(declaration.part)))
        elif isinstance(declaration, PortDecl):
            if declaration.name in ports:
                _error("CMP027", f"duplicate port {declaration.name!r}", declaration.location)
            try:
                ports[declaration.name] = PinType(declaration.pin_type)
            except ValueError:
                allowed = ", ".join(item.value for item in PinType)
                _error(
                    "CMP028",
                    f"unknown port type {declaration.pin_type!r}; expected one of: {allowed}",
                    declaration.location,
                )
        elif isinstance(declaration, ModuleInstanceDecl):
            if declaration.ref == "port":
                _error("CMP026", "'port' is reserved and cannot be a module reference", declaration.location)
            module_instances.append(
                ModuleInstanceDecl(
                    declaration.location,
                    declaration.ref,
                    resolve_module(declaration.module),
                )
            )
        elif isinstance(declaration, NetDecl):
            nets.append(Net(declaration.name, tuple(ep(value) for value in declaration.endpoints)))
        elif isinstance(declaration, SupplyDecl):
            supplies.append(_supply(declaration))
        elif isinstance(declaration, InterfaceDecl):
            interfaces.append(_interface(declaration))
        elif isinstance(declaration, ConstraintDecl):
            constraints.append(_constraint(declaration))
        elif isinstance(declaration, ConfigurationDecl):
            configurations.append(declaration)
        elif isinstance(declaration, PowerStateDecl):
            power_states.append(_power_state(declaration))

    selections = tuple(
        _configuration(declaration, components, library, devices)
        for declaration in configurations
    )

    return (
        Board(
            name=document.name,
            library=library,
            components=tuple(components),
            nets=tuple(nets),
            supplies=tuple(supplies),
            interfaces=tuple(interfaces),
            constraints=tuple(constraints),
            devices=devices,
            peripheral_selections=selections,
            power_states=tuple(power_states),
        ),
        ports,
        tuple(module_instances),
    )


def _load_libraries(document: Document) -> dict[str, PartDefinition]:
    library: dict[str, PartDefinition] = {}
    for library_name in document.libraries:
        try:
            factory = library_factory(library_name)
        except ValueError as exc:
            _error("CMP002", str(exc), document.location)
        if factory is None:
            _error("CMP002", f"unknown library {library_name!r}", document.location)
        _merge_library(library, factory(), document.location)
    return library


def _merge_library(
    destination: dict[str, PartDefinition],
    incoming: Mapping[str, PartDefinition],
    location: SourceLocation,
) -> None:
    for part_name, part in incoming.items():
        existing = destination.get(part_name)
        if existing is not None and existing != part:
            _error("CMP003", f"conflicting definitions for part {part_name!r}", location)
        destination.setdefault(part_name, part)


def _merge_devices(
    destination: dict[str, DeviceDefinition],
    incoming: Mapping[str, DeviceDefinition],
    location: SourceLocation,
) -> None:
    for device_name, device in incoming.items():
        existing = destination.get(device_name)
        if existing is not None and existing != device:
            _error("CMP053", f"conflicting definitions for device {device_name!r}", location)
        destination.setdefault(device_name, device)


def _validate_module_ports(
    body: Board, ports: dict[str, PinType], location: SourceLocation
) -> None:
    counts: Counter[str] = Counter()
    for net in body.nets:
        for endpoint in net.endpoints:
            if endpoint.component != "port":
                continue
            if endpoint.pin not in ports:
                _error("CMP029", f"net references undeclared port {endpoint.pin!r}", location)
            counts[endpoint.pin] += 1
    for port_name in ports:
        if counts[port_name] == 0:
            _error("CMP030", f"port {port_name!r} is not connected to an internal net", location)
        if counts[port_name] > 1:
            _error("CMP031", f"port {port_name!r} appears on multiple internal nets", location)


def _validate_instances(
    body: Board,
    declarations: tuple[ModuleInstanceDecl, ...],
    modules: Mapping[str, ModuleDefinition],
) -> tuple[ModuleInstance, ...]:
    known_refs = {component.ref for component in body.components}
    result: list[ModuleInstance] = []
    endpoints_by_owner: dict[str, list[tuple[str, str]]] = defaultdict(list)
    for net in body.nets:
        for endpoint in net.endpoints:
            endpoints_by_owner[endpoint.component].append((endpoint.pin, net.name))

    for declaration in declarations:
        if declaration.ref in known_refs:
            _error(
                "CMP032",
                f"instance reference {declaration.ref!r} conflicts with another instance",
                declaration.location,
            )
        definition = modules.get(declaration.module)
        if definition is None:
            _error("CMP033", f"unknown module {declaration.module!r}", declaration.location)

        connections: dict[str, list[str]] = defaultdict(list)
        for port_name, net_name in endpoints_by_owner.get(declaration.ref, []):
            if port_name not in definition.ports:
                _error(
                    "CMP034",
                    f"module {definition.name} has no port {port_name!r}",
                    declaration.location,
                )
            connections[port_name].append(net_name)
        for port_name in definition.ports:
            connected_nets = sorted(set(connections.get(port_name, [])))
            if not connected_nets:
                _error(
                    "CMP038",
                    f"module port {declaration.ref}.{port_name} is not connected",
                    declaration.location,
                )
            if len(connected_nets) > 1:
                _error(
                    "CMP035",
                    f"module port {declaration.ref}.{port_name} is connected to multiple nets: "
                    + ", ".join(connected_nets),
                    declaration.location,
                )
        result.append(ModuleInstance(declaration.ref, declaration.module))
        known_refs.add(declaration.ref)
    return tuple(result)


def _component(declaration: ComponentDecl, part_name: str | None = None) -> ComponentInstance:
    _reject_unknown_attributes(
        declaration.attributes, {"value", "footprint", "modes"}, declaration.location
    )
    value = declaration.attributes.get("value")
    if isinstance(value, RawQuantity):
        compiled_value: Quantity | str | None = _quantity(value, declaration.location)
    elif value is None or isinstance(value, str):
        compiled_value = value
    else:
        _error("CMP004", "component value must be a quantity or string", declaration.location)

    footprint = declaration.attributes.get("footprint")
    if footprint is not None and not isinstance(footprint, str):
        _error("CMP005", "component footprint must be a string", declaration.location)
    modes = _key_values(declaration.attributes.get("modes", ""), "component modes", declaration.location)
    return ComponentInstance(
        ref=declaration.ref,
        part=part_name or declaration.part,
        value=compiled_value,
        footprint=footprint,
        modes=modes,
    )


def _compile_device(document: Document, qualified_name: str) -> DeviceDefinition:
    properties: dict[str, str] = {}
    pads: dict[str, DevicePadDefinition] = {}
    power_domains: dict[str, PowerDomainDefinition] = {}
    peripherals: dict[str, PeripheralDefinition] = {}
    resources: list[str] = []
    mux_options: list[MuxOption] = []
    units: dict[str, FunctionalUnitDefinition] = {}
    groups: dict[str, SignalGroupDefinition] = {}
    mode_groups: dict[str, ModeGroupDefinition] = {}
    pad_sets: dict[str, PadSetDefinition] = {}
    route_rules: list[RouteRule] = []
    for declaration in document.declarations:
        if isinstance(declaration, DevicePropertyDecl):
            if declaration.name in properties:
                _error("CMP054", f"duplicate device property {declaration.name!r}", declaration.location)
            if not isinstance(declaration.value, str):
                _error("CMP055", "device metadata values must be strings or identifiers", declaration.location)
            properties[declaration.name] = declaration.value
        elif isinstance(declaration, PadDecl):
            if declaration.name in pads:
                _error("CMP073", f"duplicate device pad {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(
                declaration.attributes,
                {
                    "domains",
                    "directions",
                    "drive_modes",
                    "traits",
                    "power_domain",
                    "unpowered",
                    "voltage_min",
                    "voltage_max",
                    "absolute_min",
                    "absolute_max",
                    "when",
                },
                declaration.location,
            )
            profile = _electrical_profile(declaration.attributes, declaration.location)
            power_domain = _optional_string(
                declaration.attributes.get("power_domain"), "pad power_domain", declaration.location
            )
            raw_unpowered = _string_value(
                declaration.attributes.get("unpowered", UnpoweredBehavior.UNKNOWN.value),
                "pad unpowered behavior",
                declaration.location,
            )
            try:
                unpowered = UnpoweredBehavior(raw_unpowered)
            except ValueError:
                allowed = ", ".join(item.value for item in UnpoweredBehavior)
                _error("CMP074", f"unknown unpowered behavior {raw_unpowered!r}; expected: {allowed}", declaration.location)
            pads[declaration.name] = DevicePadDefinition(
                name=declaration.name,
                profile=profile,
                power_domain=power_domain,
                unpowered_behavior=unpowered,
                when=_condition(declaration.attributes.get("when"), declaration.location),
            )
        elif isinstance(declaration, PowerDomainDecl):
            if declaration.name in power_domains:
                _error("CMP075", f"duplicate power domain {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(
                declaration.attributes,
                {"supply_pads", "voltage_min", "voltage_max", "requires"},
                declaration.location,
            )
            supply_pads = _csv_names(
                _required(declaration.attributes, "supply_pads", declaration.location),
                "power-domain supply_pads",
                declaration.location,
            )
            power_domains[declaration.name] = PowerDomainDefinition(
                declaration.name,
                supply_pads,
                _voltage_range(declaration.attributes, declaration.location),
                _csv_names(declaration.attributes.get("requires", ""), "domain requirements", declaration.location),
            )
        elif isinstance(declaration, ResourceDecl):
            if declaration.name in resources:
                _error("CMP056", f"duplicate device resource {declaration.name!r}", declaration.location)
            resources.append(declaration.name)
        elif isinstance(declaration, PeripheralDecl):
            if declaration.name in peripherals:
                _error("CMP057", f"duplicate peripheral {declaration.name!r}", declaration.location)
            signals: dict[str, PeripheralSignalDefinition] = {}
            for signal in declaration.signals:
                if signal.name in signals:
                    _error("CMP058", f"duplicate peripheral signal {signal.name!r}", signal.location)
                _reject_unknown_attributes(signal.attributes, {"required"}, signal.location)
                required = signal.attributes.get("required", True)
                if not isinstance(required, bool):
                    _error("CMP059", "signal required property must be true or false", signal.location)
                signals[signal.name] = PeripheralSignalDefinition(
                    signal.name, _profile_for_pin_type(signal.pin_type, signal.location), required
                )
            if not signals:
                _error("CMP061", "peripheral must declare at least one signal", declaration.location)
            peripherals[declaration.name] = PeripheralDefinition(
                declaration.name, declaration.kind, signals
            )
        elif isinstance(declaration, MuxDecl):
            _reject_unknown_attributes(
                declaration.attributes,
                {"selector", "resource", "setting", "when"},
                declaration.location,
            )
            selector = _required(declaration.attributes, "selector", declaration.location)
            resource = declaration.attributes.get("resource")
            setting = declaration.attributes.get("setting")
            if not isinstance(selector, str):
                _error("CMP062", "mux selector must be a string or identifier", declaration.location)
            if resource is not None and not isinstance(resource, str):
                _error("CMP063", "mux resource must be an identifier", declaration.location)
            if setting is not None and not isinstance(setting, str):
                _error("CMP064", "mux setting must be a string or identifier", declaration.location)
            if (resource is None) != (setting is None):
                _error("CMP065", "mux resource and setting must be specified together", declaration.location)
            mux_options.append(
                MuxOption(
                    declaration.pad,
                    declaration.peripheral,
                    declaration.signal,
                    selector,
                    resource,
                    setting,
                    _condition(declaration.attributes.get("when"), declaration.location),
                )
            )
        elif isinstance(declaration, UnitDecl):
            if declaration.name in units:
                _error("CMP088", f"duplicate functional unit {declaration.name!r}", declaration.location)
            raw = dict(declaration.terminals)
            shared = raw.pop("shared", False)
            if not isinstance(shared, bool):
                _error("CMP089", "unit shared property must be true or false", declaration.location)
            terminals = {
                name: TerminalBinding(_string_value(value, "terminal pad", declaration.location))
                for name, value in raw.items()
            }
            units[declaration.name] = FunctionalUnitDefinition(
                declaration.name, declaration.kind, terminals, shared
            )
        elif isinstance(declaration, SignalGroupDecl):
            if declaration.name in groups:
                _error("CMP090", f"duplicate signal group {declaration.name!r}", declaration.location)
            raw = dict(declaration.attributes)
            when = _condition(raw.pop("when", None), declaration.location)
            members = {
                name: _string_value(value, "signal-group member", declaration.location)
                for name, value in raw.items()
            }
            groups[declaration.name] = SignalGroupDefinition(
                declaration.name, declaration.kind, members, when=when
            )
        elif isinstance(declaration, ModeGroupDecl):
            if declaration.name in mode_groups:
                _error("CMP091", f"duplicate mode group {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(declaration.attributes, {"choices", "default"}, declaration.location)
            choices = _csv_names(
                _required(declaration.attributes, "choices", declaration.location),
                "mode choices",
                declaration.location,
            )
            default = _optional_string(declaration.attributes.get("default"), "mode default", declaration.location)
            if default is not None and default not in choices:
                _error("CMP092", f"mode default {default!r} is not a declared choice", declaration.location)
            mode_groups[declaration.name] = ModeGroupDefinition(declaration.name, choices, default)
        elif isinstance(declaration, PadSetDecl):
            if declaration.name in pad_sets:
                _error("CMP093", f"duplicate pad set {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(declaration.attributes, {"pads"}, declaration.location)
            pad_sets[declaration.name] = PadSetDefinition(
                declaration.name,
                _csv_names(_required(declaration.attributes, "pads", declaration.location), "pad set", declaration.location),
            )
        elif isinstance(declaration, RouteRuleDecl):
            _reject_unknown_attributes(
                declaration.attributes, {"pad_set", "selector", "parameters", "when"}, declaration.location
            )
            route_rules.append(
                RouteRule(
                    declaration.peripheral,
                    declaration.signal,
                    _string_value(_required(declaration.attributes, "pad_set", declaration.location), "route pad_set", declaration.location),
                    SelectorScheme(
                        _string_value(_required(declaration.attributes, "selector", declaration.location), "selector scheme", declaration.location),
                        _key_values(declaration.attributes.get("parameters", ""), "selector parameters", declaration.location),
                    ),
                    _condition(declaration.attributes.get("when"), declaration.location),
                )
            )
        else:
            _error(
                "CMP066",
                "device files may only contain metadata, pads, power domains, peripherals, resources, and mux options",
                document.location,
            )

    for mux in mux_options:
        if mux.pad not in pads:
            _error("CMP076", f"mux references unknown device pad {mux.pad!r}", document.location)
        peripheral = peripherals.get(mux.peripheral)
        if peripheral is None:
            _error("CMP067", f"mux references unknown peripheral {mux.peripheral!r}", document.location)
        if mux.signal not in peripheral.signals:
            _error(
                "CMP068",
                f"mux references unknown signal {mux.peripheral}.{mux.signal}",
                document.location,
            )
        if mux.resource is not None and mux.resource not in resources:
            _error("CMP069", f"mux references undeclared resource {mux.resource!r}", document.location)
    for pad in pads.values():
        for reference in _relative_limit_references(pad.profile):
            if reference not in pads:
                _error(
                    "CMP122",
                    f"pad {pad.name!r} voltage limit references unknown device pad {reference!r}",
                    document.location,
                )
        if pad.power_domain is not None and pad.power_domain not in power_domains:
            _error("CMP077", f"pad {pad.name!r} references unknown power domain {pad.power_domain!r}", document.location)
    for domain in power_domains.values():
        unknown = sorted(set(domain.supply_pads) - set(pads))
        if unknown:
            _error("CMP078", f"power domain {domain.name!r} references unknown supply pad {unknown[0]!r}", document.location)
    for unit in units.values():
        for terminal in unit.terminals.values():
            if terminal.pad not in pads:
                _error("CMP094", f"unit {unit.name!r} references unknown pad {terminal.pad!r}", document.location)
    for group in groups.values():
        unknown = sorted(set(group.members.values()) - set(pads))
        if unknown:
            _error("CMP095", f"signal group {group.name!r} references unknown pad {unknown[0]!r}", document.location)
        if str(group.kind) == GroupKind.DIFFERENTIAL_PAIR.value and set(group.members) != {"positive", "negative"}:
            _error("CMP096", f"differential group {group.name!r} requires positive and negative members", document.location)
    for pad_set in pad_sets.values():
        unknown = sorted(set(pad_set.pads) - set(pads))
        if unknown:
            _error("CMP097", f"pad set {pad_set.name!r} references unknown pad {unknown[0]!r}", document.location)
    for route in route_rules:
        peripheral = peripherals.get(route.peripheral)
        if peripheral is None or route.signal not in peripheral.signals:
            _error("CMP098", f"route references unknown signal {route.peripheral}.{route.signal}", document.location)
        if route.pad_set not in pad_sets:
            _error("CMP099", f"route references unknown pad set {route.pad_set!r}", document.location)
    mux_keys = [(mux.pad, mux.peripheral, mux.signal) for mux in mux_options]
    if len(mux_keys) != len(set(mux_keys)):
        _error("CMP070", "device contains a duplicate mux option", document.location)
    return DeviceDefinition(
        name=qualified_name,
        pads=pads,
        peripherals=peripherals,
        mux_options=tuple(mux_options),
        power_domains=power_domains,
        units=units,
        signal_groups=groups,
        mode_groups=mode_groups,
        pad_sets=pad_sets,
        route_rules=tuple(route_rules),
        resources=tuple(resources),
        metadata={
            key: value for key, value in properties.items() if not key.startswith("source_")
        },
        source=_source_reference(properties, document.location),
    )


def _compile_part(
    document: Document,
    qualified_name: str,
    resolve_device: Callable[[str], str] = lambda value: value,
    devices: Mapping[str, DeviceDefinition] | None = None,
) -> PartDefinition:
    properties: dict[str, Scalar] = {}
    pins: dict[str, PackagePinDefinition] = {}
    for declaration in document.declarations:
        if isinstance(declaration, PartPropertyDecl):
            if declaration.name in properties:
                _error("CMP042", f"duplicate part property {declaration.name!r}", declaration.location)
            properties[declaration.name] = declaration.value
        elif isinstance(declaration, PinDecl):
            if declaration.name in pins:
                _error("CMP043", f"duplicate pin {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(
                declaration.attributes,
                {
                    "number",
                    "domains",
                    "directions",
                    "drive_modes",
                    "traits",
                    "bond",
                    "voltage_min",
                    "voltage_max",
                    "absolute_min",
                    "absolute_max",
                    "connection",
                    "required_net_traits",
                    "when",
                },
                declaration.location,
            )
            number = _required(declaration.attributes, "number", declaration.location)
            if not isinstance(number, str):
                _error("CMP044", "pin number must be a string", declaration.location)
            bonds = _csv_names(
                declaration.attributes.get("bond", ""), "pin bond", declaration.location
            )
            try:
                policy = ConnectionPolicy(
                    _string_value(
                        declaration.attributes.get("connection", ConnectionPolicy.NORMAL.value),
                        "connection policy",
                        declaration.location,
                    )
                )
            except ValueError:
                _error("CMP100", "unknown package-pin connection policy", declaration.location)
            profile = (
                _electrical_profile(declaration.attributes, declaration.location)
                if any(
                    name in declaration.attributes
                    for name in (
                        "domains", "directions", "drive_modes", "traits",
                        "voltage_min", "voltage_max", "absolute_min", "absolute_max",
                    )
                )
                else None
            )
            bond_condition = _condition(declaration.attributes.get("when"), declaration.location)
            compiled_bonds: list[BondDefinition] = []
            for bond in bonds:
                pad, separator, raw_condition = bond.partition("@")
                compiled_bonds.append(
                    BondDefinition(
                        pad,
                        _condition(raw_condition, declaration.location)
                        if separator
                        else bond_condition,
                    )
                )
            pins[declaration.name] = PackagePinDefinition(
                name=declaration.name,
                number=number,
                profile=profile,
                bonds=tuple(compiled_bonds),
                connection_policy=policy,
                required_net_traits=frozenset(
                    _csv_names(
                        declaration.attributes.get("required_net_traits", ""),
                        "required net traits",
                        declaration.location,
                    )
                ),
            )
        else:
            _error("CMP046", "part files may only contain properties and pins", document.location)

    _reject_unknown_attributes(
        properties,
        {
            "category",
            "traits",
            "manufacturer",
            "assembled",
            "footprint",
            "device",
            "source_document",
            "source_revision",
            "source_location",
            "source_url",
            "source_checksum",
            "internal_pad_groups",
        },
        document.location,
    )
    category = _string_value(
        properties.get("category", "component.generic"), "part category", document.location
    )
    traits = frozenset(
        _csv_names(properties.get("traits", ""), "part traits", document.location)
    )
    manufacturer = properties.get("manufacturer")
    if manufacturer is not None and not isinstance(manufacturer, str):
        _error("CMP048", "part manufacturer must be a string", document.location)
    assembled = properties.get("assembled", True)
    if not isinstance(assembled, bool):
        _error("CMP048", "part assembled must be a boolean", document.location)
    footprint = properties.get("footprint")
    if footprint is not None and not isinstance(footprint, str):
        _error("CMP049", "part footprint must be a string", document.location)
    device = properties.get("device")
    if device is not None and not isinstance(device, str):
        _error("CMP071", "part device must be an identifier", document.location)
    resolved_device = resolve_device(device) if device else None
    device_definition = (devices or {}).get(resolved_device or "")
    for pin in pins.values():
        if resolved_device and not pin.bonds and pin.connection_policy is not ConnectionPolicy.DO_NOT_CONNECT:
            _error("CMP079", f"device-backed pin {pin.name!r} must declare bond", document.location)
        if (
            not resolved_device
            and pin.profile is None
            and pin.connection_policy not in (ConnectionPolicy.DO_NOT_CONNECT, ConnectionPolicy.OPTIONAL)
        ):
            _error("CMP080", f"standalone pin {pin.name!r} must declare an electrical profile", document.location)
        if device_definition is not None:
            unknown = sorted({bond.pad for bond in pin.bonds} - set(device_definition.pads))
            if unknown:
                _error("CMP081", f"pin {pin.name!r} bonds unknown device pad {unknown[0]!r}", document.location)
        for reference in _relative_limit_references(pin.profile):
            known = reference in pins or (
                device_definition is not None and reference in device_definition.pads
            )
            # An unresolved device is reported elsewhere; only reject a
            # reference that cannot name any pin or pad of a known part.
            if not known and (not resolved_device or device_definition is not None):
                _error(
                    "CMP122",
                    f"pin {pin.name!r} voltage limit references unknown pin or device pad {reference!r}",
                    document.location,
                )
    if not pins:
        _error("CMP050", "part must declare at least one pin", document.location)
    pin_numbers = [pin.number for pin in pins.values()]
    if len(pin_numbers) != len(set(pin_numbers)):
        _error("CMP087", "part contains duplicate physical pin numbers", document.location)
    raw_groups = properties.get("internal_pad_groups", "")
    if not isinstance(raw_groups, str):
        _error("CMP088", "internal_pad_groups must be a quoted string", document.location)
    try:
        groups = tuple(InternalPadGroup(tuple(value.replace(",", " ").split()))
                       for value in raw_groups.split(";")) if raw_groups else ()
        from .pad_connections import validate_internal_pad_groups
        groups = validate_internal_pad_groups(groups, pin_numbers)
    except ValueError as exc:
        _error("CMP088", str(exc), document.location)
    return PartDefinition(
        name=qualified_name,
        pins=pins,
        category=category,
        traits=traits,
        footprints=(footprint,) if footprint else (),
        manufacturer=manufacturer,
        assembled=assembled,
        device=resolved_device,
        source=_source_reference(properties, document.location),
        internal_pad_groups=groups,
    )


def _configuration(
    declaration: ConfigurationDecl,
    components: list[ComponentInstance],
    library: Mapping[str, PartDefinition],
    devices: Mapping[str, DeviceDefinition],
) -> PeripheralSelection:
    component = next((item for item in components if item.ref == declaration.component), None)
    device: DeviceDefinition | None = None
    part: PartDefinition | None = None
    if component is not None:
        part = library.get(component.part)
        if part is not None and part.device is not None:
            device = devices.get(part.device)

    raw_signals = dict(declaration.signals)
    raw_usage = raw_signals.pop("usage", SelectionUsage.EXCLUSIVE.value)
    try:
        usage = SelectionUsage(raw_usage)
    except ValueError:
        allowed = ", ".join(item.value for item in SelectionUsage)
        _error("CMP082", f"unknown configuration usage {raw_usage!r}; expected: {allowed}", declaration.location)

    signals: dict[str, PeripheralSignalSelection] = {}
    modes = effective_modes(component, device) if component is not None else {}
    for signal, pin in raw_signals.items():
        package_pin = part.pins.get(pin) if part is not None else None
        bonded_pads = (
            active_bonded_pads(component, package_pin, device)
            if package_pin is not None and component is not None
            else ()
        )
        options = (
            [
                option
                for option in device.mux_options
                if option.pad in bonded_pads
                and option.peripheral == declaration.peripheral
                and option.signal == signal
            ]
            if device is not None
            else []
        )
        option = options[0] if len(options) == 1 else None
        selected_pad = bonded_pads[0] if len(bonded_pads) == 1 else None
        route = (
            next(
                (
                    rule
                    for rule in device.route_rules
                    if rule.peripheral == declaration.peripheral
                    and rule.signal == signal
                    and selected_pad in device.pad_sets[rule.pad_set].pads
                    and condition_active(rule.when, modes)
                ),
                None,
            )
            if device is not None and selected_pad is not None
            else None
        )
        signals[signal] = PeripheralSignalSelection(
            pin=pin,
            pad=selected_pad,
            selector=option.selector if option else _derive_selector(route, selected_pad),
            resource=option.resource if option else None,
            setting=option.setting if option else None,
        )
    return PeripheralSelection(
        declaration.component,
        declaration.peripheral,
        declaration.name,
        signals,
        usage,
    )


def _power_state(declaration: PowerStateDecl) -> PowerState:
    rails: dict[str, PowerRailState] = {}
    for rail, raw_state in declaration.rails.items():
        try:
            rails[rail] = PowerRailState(raw_state)
        except ValueError:
            allowed = ", ".join(item.value for item in PowerRailState)
            _error("CMP083", f"unknown power rail state {raw_state!r}; expected: {allowed}", declaration.location)
    return PowerState(declaration.name, rails)


def _profile_for_pin_type(value: str, location: SourceLocation) -> ElectricalProfile:
    profiles = {
        PinType.PASSIVE.value: ElectricalProfile(
            frozenset({SignalDomain.ANALOG}), frozenset({Direction.PASSIVE})
        ),
        PinType.INPUT.value: ElectricalProfile(
            frozenset({SignalDomain.DIGITAL}), frozenset({Direction.INPUT})
        ),
        PinType.OUTPUT.value: ElectricalProfile(
            frozenset({SignalDomain.DIGITAL}),
            frozenset({Direction.OUTPUT}),
            frozenset({DriveMode.PUSH_PULL}),
        ),
        PinType.BIDIRECTIONAL.value: ElectricalProfile(
            frozenset({SignalDomain.DIGITAL}),
            frozenset({Direction.BIDIRECTIONAL}),
            frozenset({DriveMode.PUSH_PULL}),
        ),
        PinType.OPEN_DRAIN.value: ElectricalProfile(
            frozenset({SignalDomain.DIGITAL}),
            frozenset({Direction.BIDIRECTIONAL}),
            frozenset({DriveMode.OPEN_DRAIN}),
        ),
        PinType.POWER_IN.value: ElectricalProfile(
            frozenset({SignalDomain.POWER}), frozenset({Direction.INPUT})
        ),
        PinType.POWER_OUT.value: ElectricalProfile(
            frozenset({SignalDomain.POWER}), frozenset({Direction.OUTPUT})
        ),
    }
    try:
        return profiles[value]
    except KeyError:
        _error("CMP060", f"unknown signal electrical profile {value!r}", location)


def _electrical_profile(
    attributes: Mapping[str, Scalar], location: SourceLocation
) -> ElectricalProfile:
    try:
        domains = frozenset(
            SignalDomain(name)
            for name in _csv_names(
                _required(dict(attributes), "domains", location), "signal domains", location
            )
        )
        directions = frozenset(
            Direction(name)
            for name in _csv_names(
                _required(dict(attributes), "directions", location), "directions", location
            )
        )
        drive_modes = frozenset(
            DriveMode(name)
            for name in _csv_names(attributes.get("drive_modes", ""), "drive modes", location)
        )
    except ValueError as exc:
        _error("CMP084", str(exc), location)
    operating = _voltage_range(attributes, location)
    return ElectricalProfile(
        domains,
        directions,
        drive_modes,
        frozenset(_csv_names(attributes.get("traits", ""), "electrical traits", location)),
        operating,
        absolute_voltage=_absolute_voltage_range(attributes, operating, location),
    )


def _voltage_range(
    attributes: Mapping[str, Scalar], location: SourceLocation
) -> QuantityRange[Voltage] | None:
    minimum = _optional_voltage(attributes.get("voltage_min"), location)
    maximum = _optional_voltage(attributes.get("voltage_max"), location)
    return QuantityRange(minimum=minimum, maximum=maximum) if minimum or maximum else None


# ``VTERM``, ``VTERM+0.1V`` or ``VDDIO - 300mV``: a pad name and an optional
# signed voltage offset. The lazy reference lets names contain ``-``.
_RELATIVE_LIMIT = re.compile(
    r"\s*(?P<reference>[A-Za-z_][A-Za-z0-9_-]*?)\s*"
    r"(?:(?P<sign>[+-])\s*(?P<number>\d+(?:\.\d+)?)\s*(?P<unit>[A-Za-z]+))?\s*"
)
_FIXED_LIMIT = re.compile(r"\s*(?P<number>[+-]?\d+(?:\.\d+)?)\s*(?P<unit>[A-Za-z]+)\s*")


def _voltage_limit(
    value: Scalar | None, label: str, location: SourceLocation
) -> Voltage | RelativeVoltage | None:
    """A fixed voltage, or ``"PAD[+|-]OFFSET"`` optionally with a fixed cap.

    ``"VTERM+0.1V, 1.36V"`` is the tighter of the pad-relative and the fixed
    limit.
    """

    if value is None:
        return None
    if isinstance(value, RawQuantity):
        compiled = _quantity(value, location)
        if not isinstance(compiled, Voltage):
            _error("CMP120", f"{label} must use V or mV", location)
        return compiled
    usage = (
        f"{label} must be a voltage such as 1.35V or a pad-relative limit such as "
        f"\"VTERM+0.1V\", optionally capped as \"VTERM+0.1V, 1.36V\""
    )
    if not isinstance(value, str):
        _error("CMP120", usage, location)
    terms = value.split(",")
    if len(terms) > 2:
        _error("CMP120", usage, location)
    relative: tuple[str, Voltage] | None = None
    fixed: Voltage | None = None
    for term in terms:
        fixed_match = _FIXED_LIMIT.fullmatch(term)
        relative_match = None if fixed_match else _RELATIVE_LIMIT.fullmatch(term)
        if fixed_match is not None:
            if fixed is not None or fixed_match["unit"] not in Voltage.UNITS:
                _error("CMP120", usage, location)
            fixed = Voltage.of(fixed_match["number"], fixed_match["unit"])
        elif relative_match is not None:
            if relative is not None:
                _error("CMP120", usage, location)
            if relative_match["number"] is None:
                offset = Voltage.of(0, "V")
            else:
                if relative_match["unit"] not in Voltage.UNITS:
                    _error("CMP120", f"{label} offset must use V or mV", location)
                sign = "" if relative_match["sign"] == "+" else "-"
                offset = Voltage.of(f"{sign}{relative_match['number']}", relative_match["unit"])
            relative = (relative_match["reference"], offset)
        else:
            _error("CMP120", usage, location)
    if relative is None:
        return fixed
    return RelativeVoltage(relative[0], relative[1], fixed)


def _absolute_voltage_range(
    attributes: Mapping[str, Scalar],
    operating: QuantityRange[Voltage] | None,
    location: SourceLocation,
) -> QuantityRange[Voltage] | None:
    """Absolute-maximum ratings; fixed bounds must enclose the operating range."""

    minimum = _voltage_limit(attributes.get("absolute_min"), "absolute_min", location)
    maximum = _voltage_limit(attributes.get("absolute_max"), "absolute_max", location)
    if minimum is None and maximum is None:
        return None
    # Only fixed parts can be compared before a component resolves references.
    fixed_minimum = minimum.limit if isinstance(minimum, RelativeVoltage) else minimum
    fixed_maximum = maximum.limit if isinstance(maximum, RelativeVoltage) else maximum
    if fixed_minimum is not None and fixed_maximum is not None and fixed_maximum < fixed_minimum:
        _error(
            "CMP121",
            f"absolute_min {fixed_minimum} exceeds absolute_max {fixed_maximum}",
            location,
        )
    if operating is not None:
        if (
            fixed_minimum is not None
            and operating.minimum is not None
            and operating.minimum < fixed_minimum
        ):
            _error(
                "CMP121",
                f"voltage_min {operating.minimum} is below absolute_min {fixed_minimum}",
                location,
            )
        if (
            fixed_maximum is not None
            and operating.maximum is not None
            and fixed_maximum < operating.maximum
        ):
            _error(
                "CMP121",
                f"voltage_max {operating.maximum} exceeds absolute_max {fixed_maximum}",
                location,
            )
    return QuantityRange(minimum=minimum, maximum=maximum, rating="absolute")


def _relative_limit_references(profile: ElectricalProfile | None) -> tuple[str, ...]:
    absolute = profile.absolute_voltage if profile is not None else None
    if absolute is None:
        return ()
    return tuple(
        bound.reference
        for bound in (absolute.minimum, absolute.maximum)
        if isinstance(bound, RelativeVoltage)
    )


def _key_values(value: Scalar, label: str, location: SourceLocation) -> dict[str, str]:
    result: dict[str, str] = {}
    for item in _csv_names(value, label, location):
        key, separator, selected = item.partition("=")
        if not separator or not key or not selected:
            _error("CMP101", f"{label} entries must use NAME=VALUE", location)
        if key in result:
            _error("CMP101", f"{label} contains duplicate key {key!r}", location)
        result[key] = selected
    return result


def _condition(value: Scalar | None, location: SourceLocation) -> Condition | None:
    return None if value is None else Condition(_key_values(value, "condition", location))


def _derive_selector(rule: RouteRule | None, pad: str | None) -> str | None:
    if rule is None or pad is None:
        return None
    if rule.selector.kind == "pad_name":
        return pad
    if rule.selector.kind == "nrf_psel":
        # P0_13 -> 13, P1_02 -> 34; deterministic and sufficient for PSEL data.
        if "_" in pad and pad[1:2].isdigit():
            port, pin = pad[1:].split("_", 1)
            if port.isdigit() and pin.isdigit():
                return str(int(port) * 32 + int(pin))
    return f"{rule.selector.kind}:{pad}"


def _csv_names(value: Scalar, label: str, location: SourceLocation) -> tuple[str, ...]:
    text = _string_value(value, label, location)
    if not text.strip():
        return ()
    names = tuple(item.strip() for item in text.split(",") if item.strip())
    if len(names) != len(set(names)):
        _error("CMP085", f"{label} contains a duplicate name", location)
    return names


def _string_value(value: Scalar, label: str, location: SourceLocation) -> str:
    if not isinstance(value, str):
        _error("CMP086", f"{label} must be a string or identifier", location)
    return value


def _optional_string(value: Scalar | None, label: str, location: SourceLocation) -> str | None:
    return None if value is None else _string_value(value, label, location)


def _source_reference(
    properties: Mapping[str, Scalar], location: SourceLocation
) -> SourceReference | None:
    values = {
        field: properties.get(f"source_{field}")
        for field in ("document", "revision", "location", "url", "checksum")
    }
    if not any(value is not None for value in values.values()):
        return None
    return SourceReference(
        **{
            name: _string_value(value, f"source_{name}", location)
            if value is not None
            else None
            for name, value in values.items()
        }
    )


def _optional_voltage(value: Scalar | None, location: SourceLocation) -> Voltage | None:
    if value is None:
        return None
    compiled = _quantity(value, location)
    if not isinstance(compiled, Voltage):
        _error("CMP051", "pin voltage limits must use V or mV", location)
    return compiled


def _supply(declaration: SupplyDecl) -> Supply:
    _reject_unknown_attributes(
        declaration.attributes, {"voltage", "net", "source", "external"}, declaration.location
    )
    voltage = _required(declaration.attributes, "voltage", declaration.location)
    compiled_voltage = _quantity(voltage, declaration.location)
    if not isinstance(compiled_voltage, Voltage):
        _error("CMP006", "supply voltage must use V or mV", declaration.location)

    net = declaration.attributes.get("net", declaration.name)
    if not isinstance(net, str):
        _error("CMP007", "supply net must be a net name", declaration.location)
    source = declaration.attributes.get("source")
    if source is not None and not isinstance(source, str):
        _error("CMP008", "supply source must be an endpoint", declaration.location)
    external = declaration.attributes.get("external", False)
    if not isinstance(external, bool):
        _error("CMP009", "supply external property must be true or false", declaration.location)
    return Supply(
        name=declaration.name,
        voltage=compiled_voltage,
        net=net,
        source=ep(source) if source is not None else None,
        externally_driven=external,
    )


def _interface(declaration: InterfaceDecl) -> Interface:
    type_name = declaration.kind if "." in declaration.kind else f"std.{declaration.kind}"
    known_attributes = {"pullup"}
    signals: dict[str, str] = {}
    for name, value in declaration.attributes.items():
        if name == "pullup":
            continue
        if not isinstance(value, str):
            _error("CMP011", f"interface signal {name} must be a net name", declaration.location)
        signals[name] = value
        known_attributes.add(name)
    if type_name == "std.i2c" and not {"sda", "scl"} <= set(signals):
        _error("CMP011", "std.i2c requires sda and scl net mappings", declaration.location)
    pullup = declaration.attributes.get("pullup")
    if pullup is not None and not isinstance(pullup, str):
        _error("CMP012", "I2C pullup must be a supply name", declaration.location)
    bindings = {binding.component: binding.signals for binding in declaration.bindings}
    return Interface(
        name=declaration.name,
        type_name=type_name,
        signals=signals,
        bindings=bindings,
        pullup_supply=pullup,
    )


def _constraint(declaration: ConstraintDecl) -> Constraint:
    try:
        kind = ConstraintKind(declaration.kind)
    except ValueError:
        _error("CMP013", f"unsupported constraint kind {declaration.kind!r}", declaration.location)
    parameters: dict[str, Quantity | str | int | float] = {}
    metadata_names = {"id", "mode", "consumers", "verifier"}
    for name, value in declaration.parameters.items():
        if name in metadata_names:
            continue
        if isinstance(value, RawQuantity):
            parameters[name] = _quantity(value, declaration.location)
        elif isinstance(value, (str, int, float)):
            parameters[name] = value
        else:
            _error("CMP014", f"unsupported constraint parameter {name!r}", declaration.location)
    constraint_id = declaration.parameters.get("id")
    if constraint_id is not None and not isinstance(constraint_id, str):
        _error("CMP014", "constraint id must be a string", declaration.location)
    raw_mode = declaration.parameters.get("mode", "require")
    if not isinstance(raw_mode, str):
        _error("CMP014", "constraint mode must be a name", declaration.location)
    try:
        mode = ConstraintMode(raw_mode)
    except ValueError:
        _error("CMP014", f"unsupported constraint mode {raw_mode!r}", declaration.location)
    raw_consumers = declaration.parameters.get("consumers", "")
    if not isinstance(raw_consumers, str):
        _error("CMP014", "constraint consumers must be a comma-separated string", declaration.location)
    consumers = tuple(item.strip() for item in raw_consumers.split(",") if item.strip())
    verifier = declaration.parameters.get("verifier")
    if verifier is not None and not isinstance(verifier, str):
        _error("CMP014", "constraint verifier must be a string", declaration.location)
    if kind is ConstraintKind.ROUTING:
        _validate_routing_signal_intent(declaration, parameters)
    elif kind is ConstraintKind.LENGTH_MATCH:
        _validate_length_match(declaration, parameters)
    elif kind is ConstraintKind.HOLE_CLEARANCE:
        _validate_hole_clearance(declaration, parameters)
    try:
        return Constraint(
            kind=kind,
            targets=declaration.targets,
            parameters=parameters,
            constraint_id=constraint_id,
            mode=mode,
            consumers=consumers,
            verifier=verifier,
            origins=(str(declaration.location),),
        )
    except ValueError as exc:
        _error("CMP014", str(exc), declaration.location)


_BREAKOUT_RELAXES = {
    "breakout_width": "width",
    "breakout_gap": "pair_gap",
    "breakout_clearance": "clearance",
}


def _positive_length(parameters: Mapping[str, object], name: str, label: str,
                     location: SourceLocation) -> Length | None:
    value = parameters.get(name)
    if value is None:
        return None
    if not isinstance(value, Length):
        _error("CMP110" if label == "routing" else "CMP111",
               f"{label} parameter {name!r} must be a length", location)
    if value.base_value <= 0:
        _error("CMP110" if label == "routing" else "CMP111",
               f"{label} parameter {name!r} must be positive", location)
    return value


def _validate_routing_signal_intent(declaration: ConstraintDecl,
                                    parameters: Mapping[str, object]) -> None:
    """Located checks for the signal-integrity and tuning routing properties (plan L2/L5/L6/R10)."""
    location = declaration.location
    target = parameters.get("target_single_ended_ohms")
    if target is not None and (isinstance(target, bool) or not isinstance(target, int) or target <= 0):
        _error("CMP110", "routing parameter 'target_single_ended_ohms' must be a positive integer", location)
    tolerance = parameters.get("impedance_tolerance_percent")
    if tolerance is not None:
        if isinstance(tolerance, bool) or not isinstance(tolerance, (int, float)) or not 0 < tolerance < 100:
            _error("CMP110", "routing parameter 'impedance_tolerance_percent' must be a number "
                   "greater than 0 and below 100", location)
        if "target_impedance_ohms" not in parameters and target is None:
            _error("CMP110", "'impedance_tolerance_percent' requires 'target_impedance_ohms' or "
                   "'target_single_ended_ohms'", location)
    group = parameters.get("layer_group")
    if group is not None and (not isinstance(group, str) or not group.strip()):
        _error("CMP110", "routing parameter 'layer_group' must be a nonempty name", location)
    breakout_length = _positive_length(parameters, "breakout_length", "routing", location)
    relaxed = [name for name in _BREAKOUT_RELAXES if name in parameters]
    for name in relaxed:
        value = _positive_length(parameters, name, "routing", location)
        base_name = _BREAKOUT_RELAXES[name]
        base = parameters.get(base_name)
        if name == "breakout_gap" and base is None:
            _error("CMP110", "'breakout_gap' requires a differential 'pair_gap'", location)
        if isinstance(base, Length) and value.base_value > base.base_value:
            _error("CMP110", f"'{name}' may only relax '{base_name}', never tighten it "
                   f"({name} exceeds {base_name})", location)
    if relaxed and breakout_length is None:
        _error("CMP110", "breakout properties require a positive 'breakout_length'", location)
    if breakout_length is not None and not relaxed:
        _error("CMP110", "'breakout_length' requires 'breakout_width', 'breakout_gap' or "
               "'breakout_clearance'", location)
    style = parameters.get("tuning_style")
    if style is not None and style not in ("bumps", "serpentine"):
        _error("CMP110", "routing parameter 'tuning_style' must be \"bumps\" or \"serpentine\"", location)
    if _positive_length(parameters, "tuning_spacing", "routing", location) is not None and style != "serpentine":
        _error("CMP110", "'tuning_spacing' requires tuning_style = \"serpentine\"", location)


def _validate_length_match(declaration: ConstraintDecl, parameters: Mapping[str, object]) -> None:
    location = declaration.location
    if len(declaration.targets) < 2:
        _error("CMP111", "length_match requires at least two nets", location)
    duplicates = sorted(name for name, count in Counter(declaration.targets).items() if count > 1)
    if duplicates:
        _error("CMP111", f"length_match lists net {duplicates[0]!r} more than once", location)
    pins = [target for target in declaration.targets if "." in target]
    if pins:
        _error("CMP111", f"length_match targets must be nets, not pins ({pins[0]!r})", location)
    unknown = sorted(set(parameters) - {"max_skew"})
    if unknown:
        _error("CMP111", f"unknown length_match parameter {unknown[0]!r}", location)
    if "max_skew" not in parameters:
        _error("CMP111", "length_match requires 'max_skew'", location)
    _positive_length(parameters, "max_skew", "length_match", location)


def _validate_hole_clearance(declaration: ConstraintDecl, parameters: Mapping[str, object]) -> None:
    """Located checks for one component's scoped pad-to-own-NPTH clearance."""
    location = declaration.location
    if len(declaration.targets) != 1:
        _error("CMP112", "hole_clearance requires exactly one component", location)
    if "." in declaration.targets[0]:
        _error("CMP112", f"hole_clearance targets a component, not a pin ({declaration.targets[0]!r})",
               location)
    unknown = sorted(set(parameters) - {"clearance", "reason"})
    if unknown:
        _error("CMP112", f"unknown hole_clearance parameter {unknown[0]!r}", location)
    clearance = parameters.get("clearance")
    if clearance is None:
        _error("CMP112", "hole_clearance requires 'clearance'", location)
    if not isinstance(clearance, Length) or clearance.base_value <= 0:
        _error("CMP112", "hole_clearance parameter 'clearance' must be a positive length", location)
    reason = parameters.get("reason")
    if reason is None:
        _error("CMP112", "hole_clearance requires 'reason'", location)
    if not isinstance(reason, str) or not reason.strip():
        _error("CMP112", "hole_clearance parameter 'reason' must be a nonempty string", location)


def _validate_hole_clearance_targets(
    document: Document,
    body: Board,
    instances: tuple[ModuleInstance, ...],
    modules: Mapping[str, ModuleDefinition],
) -> None:
    """Resolve each ``hole_clearance`` target and reject a second rule for one component.

    A target is a local component or a module-instance path (``MOD/J``) to a
    descendant component. Rules declared inside instantiated modules count
    too, under their instance-qualified reference.
    """
    covered = {
        f"{instance.ref}/{reference}"
        for instance in instances
        for reference in _module_hole_clearances(modules[instance.module], modules)
    }
    for declaration in document.declarations:
        if not isinstance(declaration, ConstraintDecl) or declaration.kind != ConstraintKind.HOLE_CLEARANCE.value:
            continue
        target = declaration.targets[0]
        if not _component_path_exists(target, body.components, instances, modules):
            _error("CMP113", f"hole_clearance target {target!r} is not a component", declaration.location)
        if target in covered:
            _error("CMP114", f"component {target!r} already has a hole_clearance constraint",
                   declaration.location)
        covered.add(target)


def _module_hole_clearances(definition: ModuleDefinition,
                            modules: Mapping[str, ModuleDefinition]) -> set[str]:
    """Module-relative references owning a ``hole_clearance``, including nested ones."""
    result = {constraint.targets[0] for constraint in definition.constraints
              if constraint.kind is ConstraintKind.HOLE_CLEARANCE}
    for instance in definition.module_instances:
        result.update(f"{instance.ref}/{reference}"
                      for reference in _module_hole_clearances(modules[instance.module], modules))
    return result


def _component_path_exists(target: str, components: tuple[ComponentInstance, ...],
                           instances: tuple[ModuleInstance, ...],
                           modules: Mapping[str, ModuleDefinition]) -> bool:
    *path, leaf = target.split("/")
    for segment in path:
        module = next((instance.module for instance in instances if instance.ref == segment), None)
        if module is None:
            return False
        components, instances = modules[module].components, modules[module].module_instances
    return any(component.ref == leaf for component in components)


def _quantity(value: Scalar, location: SourceLocation) -> Quantity:
    if not isinstance(value, RawQuantity):
        _error("CMP015", "expected a quantity with a unit", location)
    quantity_type = QUANTITY_TYPES.get(value.unit)
    if quantity_type is None:
        allowed = ", ".join(sorted(QUANTITY_TYPES))
        _error("CMP016", f"unknown unit {value.unit!r}; expected one of: {allowed}", location)
    return quantity_type.of(value.value, value.unit)


def _required(attributes: dict[str, Scalar], name: str, location: SourceLocation) -> Scalar:
    if name not in attributes:
        _error("CMP017", f"missing required property {name!r}", location)
    return attributes[name]


def _reject_unknown_attributes(
    attributes: dict[str, Scalar], allowed: set[str], location: SourceLocation
) -> None:
    unknown = sorted(set(attributes) - allowed)
    if unknown:
        _error("CMP018", f"unknown property {unknown[0]!r}", location)


def _error(code: str, message: str, location: SourceLocation):
    raise CopperScriptError(code, message, location)
