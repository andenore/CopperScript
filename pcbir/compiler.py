"""CopperScript compiler: parsing and lowering to hierarchical electrical IR."""

from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Mapping

from .library import tiny_library
from .model import (
    Board,
    ComponentInstance,
    Constraint,
    ConstraintKind,
    Dependency,
    DeviceDefinition,
    DevicePadDefinition,
    Interface,
    InterfaceKind,
    ModuleDefinition,
    ModuleInstance,
    MuxOption,
    Net,
    PartDefinition,
    PartKind,
    PinCapability,
    PeripheralDefinition,
    PeripheralSelection,
    PeripheralSignalDefinition,
    PeripheralSignalSelection,
    PinDefinition,
    PinType,
    PowerDomainDefinition,
    PowerRailState,
    PowerState,
    SelectionUsage,
    SourceReference,
    Supply,
    UnpoweredBehavior,
    ep,
)
from .parser import parse
from .packages import PackageResolver, ResolvedPackage
from .quantities import Capacitance, Inductance, Length, Quantity, Resistance, Voltage
from .syntax import (
    ComponentDecl,
    ConfigurationDecl,
    ConstraintDecl,
    CopperScriptError,
    Document,
    DevicePropertyDecl,
    InterfaceDecl,
    ModuleInstanceDecl,
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
    Scalar,
    SourceLocation,
    SupplyDecl,
)


LibraryFactory = Callable[[], dict[str, PartDefinition]]
LIBRARIES: dict[str, LibraryFactory] = {"tiny": tiny_library, "standard": tiny_library}

QUANTITY_TYPES: dict[str, type[Quantity]] = {
    unit: quantity_type
    for quantity_type in (Voltage, Resistance, Capacitance, Inductance, Length)
    for unit in quantity_type.UNITS
}


def compile_source(source: str, filename: str = "<memory>") -> Board:
    """Compile source into the authoritative hierarchical :class:`Board` IR."""

    document = parse(source, filename)
    if document.kind != "board":
        _error("CMP019", "compiler entry source must declare a board", document.location)
    if document.imports and filename.startswith("<"):
        _error("CMP020", "package imports require a source filename", document.location)
    if document.imports:
        resolver = PackageResolver.for_source(Path(filename).resolve(), document.location)
        imported = _load_imports(document, resolver, ())
    else:
        imported = PackageContents({}, {}, {}, ())
    return _compile_board(
        document, imported.modules, imported.parts, imported.devices, imported.dependencies
    )


def compile_file(path: str | Path) -> Board:
    source_path = Path(path).resolve()
    document = _read_document(source_path)
    if document.kind != "board":
        _error("CMP019", "compiler entry file must declare a board", document.location)
    if document.imports:
        resolver = PackageResolver.for_source(source_path, document.location)
        imported = _load_imports(document, resolver, ())
    else:
        imported = PackageContents({}, {}, {}, ())
    return _compile_board(
        document, imported.modules, imported.parts, imported.devices, imported.dependencies
    )


def lower(document: Document) -> Board:
    """Lower an import-free board syntax tree."""

    if document.kind != "board":
        _error("CMP019", "lowering entry must declare a board", document.location)
    if document.imports:
        _error("CMP020", "lower() cannot resolve package imports", document.location)
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


def _load_imports(
    document: Document,
    resolver: PackageResolver,
    stack: tuple[str, ...],
    namespace_prefix: str = "",
) -> PackageContents:
    parts: dict[str, PartDefinition] = {}
    devices: dict[str, DeviceDefinition] = {}
    modules: dict[str, ModuleDefinition] = {}
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
        dependencies.extend(package.dependencies)
        dependencies.append(
            Dependency(
                resolved.import_path,
                resolved.module_path,
                resolved.version,
                resolved.checksum,
            )
        )
    return PackageContents(parts, devices, modules, _unique_dependencies(dependencies))


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
        if document.kind not in {"device", "part", "module"}:
            _error(
                "CMP022",
                f"package source declares a {document.kind}, expected device, part, or module",
                document.location,
            )

    device_documents = {
        document.name: document for document in documents if document.kind == "device"
    }
    part_documents = {document.name: document for document in documents if document.kind == "part"}
    module_documents = {
        document.name: document for document in documents if document.kind == "module"
    }
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
    modules: dict[str, ModuleDefinition] = {}
    dependencies: list[Dependency] = []
    import_contexts: dict[str, PackageContents] = {}
    for name, document in module_documents.items():
        imported = _load_imports(document, resolver, stack, namespace)
        import_contexts[name] = imported
        _merge_library(parts, imported.parts, document.location)
        _merge_devices(devices, imported.devices, document.location)
        for definition in imported.modules.values():
            _register_module(modules, definition, document.location)
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
        _register_module(modules, definition, document.location)
        compiling.pop()
        return definition

    for module_name in module_documents:
        compile_local(module_name)
    return PackageContents(parts, devices, modules, _unique_dependencies(dependencies))


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
        factory = LIBRARIES.get(library_name)
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
    _reject_unknown_attributes(declaration.attributes, {"value", "footprint"}, declaration.location)
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
    return ComponentInstance(
        ref=declaration.ref,
        part=part_name or declaration.part,
        value=compiled_value,
        footprint=footprint,
    )


def _compile_device(document: Document, qualified_name: str) -> DeviceDefinition:
    properties: dict[str, str] = {}
    pads: dict[str, DevicePadDefinition] = {}
    power_domains: dict[str, PowerDomainDefinition] = {}
    peripherals: dict[str, PeripheralDefinition] = {}
    resources: list[str] = []
    mux_options: list[MuxOption] = []
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
                    "capabilities",
                    "role",
                    "power_domain",
                    "unpowered",
                    "voltage_min",
                    "voltage_max",
                },
                declaration.location,
            )
            capabilities = _capabilities(
                _required(declaration.attributes, "capabilities", declaration.location),
                declaration.location,
            )
            role = _string_value(
                declaration.attributes.get("role", "io"), "pad role", declaration.location
            )
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
                capabilities=capabilities,
                role=role,
                power_domain=power_domain,
                unpowered_behavior=unpowered,
                voltage_min=_optional_voltage(
                    declaration.attributes.get("voltage_min"), declaration.location
                ),
                voltage_max=_optional_voltage(
                    declaration.attributes.get("voltage_max"), declaration.location
                ),
            )
        elif isinstance(declaration, PowerDomainDecl):
            if declaration.name in power_domains:
                _error("CMP075", f"duplicate power domain {declaration.name!r}", declaration.location)
            _reject_unknown_attributes(
                declaration.attributes, {"supply_pads"}, declaration.location
            )
            supply_pads = _csv_names(
                _required(declaration.attributes, "supply_pads", declaration.location),
                "power-domain supply_pads",
                declaration.location,
            )
            power_domains[declaration.name] = PowerDomainDefinition(
                declaration.name, supply_pads
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
                try:
                    signal_type = PinType(signal.pin_type)
                except ValueError:
                    allowed = ", ".join(item.value for item in PinType)
                    _error(
                        "CMP060",
                        f"unknown signal type {signal.pin_type!r}; expected one of: {allowed}",
                        signal.location,
                    )
                signals[signal.name] = PeripheralSignalDefinition(
                    signal.name, signal_type, required
                )
            if not signals:
                _error("CMP061", "peripheral must declare at least one signal", declaration.location)
            peripherals[declaration.name] = PeripheralDefinition(
                declaration.name, declaration.kind, signals
            )
        elif isinstance(declaration, MuxDecl):
            _reject_unknown_attributes(
                declaration.attributes, {"selector", "resource", "setting"}, declaration.location
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
        if pad.power_domain is not None and pad.power_domain not in power_domains:
            _error("CMP077", f"pad {pad.name!r} references unknown power domain {pad.power_domain!r}", document.location)
    for domain in power_domains.values():
        unknown = sorted(set(domain.supply_pads) - set(pads))
        if unknown:
            _error("CMP078", f"power domain {domain.name!r} references unknown supply pad {unknown[0]!r}", document.location)
    mux_keys = [(mux.pad, mux.peripheral, mux.signal) for mux in mux_options]
    if len(mux_keys) != len(set(mux_keys)):
        _error("CMP070", "device contains a duplicate mux option", document.location)
    return DeviceDefinition(
        name=qualified_name,
        pads=pads,
        peripherals=peripherals,
        mux_options=tuple(mux_options),
        power_domains=power_domains,
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
    pins: dict[str, PinDefinition] = {}
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
                    "capabilities",
                    "role",
                    "bond",
                    "voltage_min",
                    "voltage_max",
                },
                declaration.location,
            )
            number = _required(declaration.attributes, "number", declaration.location)
            if not isinstance(number, str):
                _error("CMP044", "pin number must be a string", declaration.location)
            capabilities = _capabilities(
                declaration.attributes.get("capabilities", ""), declaration.location
            )
            role = _string_value(
                declaration.attributes.get("role", "io"), "pin role", declaration.location
            )
            bonds = _csv_names(
                declaration.attributes.get("bond", ""), "pin bond", declaration.location
            )
            voltage_min = _optional_voltage(
                declaration.attributes.get("voltage_min"), declaration.location
            )
            voltage_max = _optional_voltage(
                declaration.attributes.get("voltage_max"), declaration.location
            )
            pins[declaration.name] = PinDefinition(
                declaration.name,
                number,
                capabilities,
                role,
                bonds,
                voltage_min,
                voltage_max,
            )
        else:
            _error("CMP046", "part files may only contain properties and pins", document.location)

    _reject_unknown_attributes(
        properties,
        {
            "kind",
            "manufacturer",
            "footprint",
            "device",
            "source_document",
            "source_revision",
            "source_location",
            "source_url",
            "source_checksum",
        },
        document.location,
    )
    raw_kind = properties.get("kind", PartKind.GENERIC.value)
    if not isinstance(raw_kind, str):
        _error("CMP047", "part kind must be an identifier", document.location)
    try:
        kind = PartKind(raw_kind)
    except ValueError:
        allowed = ", ".join(item.value for item in PartKind)
        _error("CMP047", f"unknown part kind {raw_kind!r}; expected one of: {allowed}", document.location)
    manufacturer = properties.get("manufacturer")
    if manufacturer is not None and not isinstance(manufacturer, str):
        _error("CMP048", "part manufacturer must be a string", document.location)
    footprint = properties.get("footprint")
    if footprint is not None and not isinstance(footprint, str):
        _error("CMP049", "part footprint must be a string", document.location)
    device = properties.get("device")
    if device is not None and not isinstance(device, str):
        _error("CMP071", "part device must be an identifier", document.location)
    resolved_device = resolve_device(device) if device else None
    device_definition = (devices or {}).get(resolved_device or "")
    for pin in pins.values():
        if resolved_device and not pin.bonded_pads:
            _error("CMP079", f"device-backed pin {pin.name!r} must declare bond", document.location)
        if not resolved_device and not pin.capabilities:
            _error("CMP080", f"standalone pin {pin.name!r} must declare capabilities", document.location)
        if device_definition is not None:
            unknown = sorted(set(pin.bonded_pads) - set(device_definition.pads))
            if unknown:
                _error("CMP081", f"pin {pin.name!r} bonds unknown device pad {unknown[0]!r}", document.location)
    if not pins:
        _error("CMP050", "part must declare at least one pin", document.location)
    pin_numbers = [pin.number for pin in pins.values()]
    if len(pin_numbers) != len(set(pin_numbers)):
        _error("CMP087", "part contains duplicate physical pin numbers", document.location)
    return PartDefinition(
        name=qualified_name,
        pins=pins,
        kind=kind,
        footprints=(footprint,) if footprint else (),
        manufacturer=manufacturer,
        device=resolved_device,
        source=_source_reference(properties, document.location),
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
    for signal, pin in raw_signals.items():
        package_pin = part.pins.get(pin) if part is not None else None
        bonded_pads = package_pin.bonded_pads if package_pin is not None else ()
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
        signals[signal] = PeripheralSignalSelection(
            pin=pin,
            selector=option.selector if option else None,
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


def _capabilities(value: Scalar, location: SourceLocation) -> frozenset[PinCapability]:
    names = _csv_names(value, "capabilities", location)
    capabilities: set[PinCapability] = set()
    for name in names:
        try:
            capabilities.add(PinCapability(name))
        except ValueError:
            allowed = ", ".join(item.value for item in PinCapability)
            _error("CMP084", f"unknown pin capability {name!r}; expected: {allowed}", location)
    return frozenset(capabilities)


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
    if declaration.kind != InterfaceKind.I2C.value:
        _error("CMP010", f"unsupported interface kind {declaration.kind!r}", declaration.location)
    _reject_unknown_attributes(
        declaration.attributes, {"sda", "scl", "pullup"}, declaration.location
    )
    signals: dict[str, str] = {}
    for signal in ("sda", "scl"):
        value = _required(declaration.attributes, signal, declaration.location)
        if not isinstance(value, str):
            _error("CMP011", f"I2C {signal} must be a net name", declaration.location)
        signals[signal] = value
    pullup = declaration.attributes.get("pullup")
    if pullup is not None and not isinstance(pullup, str):
        _error("CMP012", "I2C pullup must be a supply name", declaration.location)
    bindings = {binding.component: binding.signals for binding in declaration.bindings}
    return Interface(
        name=declaration.name,
        kind=InterfaceKind.I2C,
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
    for name, value in declaration.parameters.items():
        if isinstance(value, RawQuantity):
            parameters[name] = _quantity(value, declaration.location)
        elif isinstance(value, (str, int, float)):
            parameters[name] = value
        else:
            _error("CMP014", f"unsupported constraint parameter {name!r}", declaration.location)
    return Constraint(kind, declaration.targets, parameters)


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
