"""Source-selected physical realizations; no electrical rewiring or geometry inference."""
from dataclasses import dataclass, replace
from hashlib import sha256
import json
import os
from pathlib import Path

from .syntax import CopperScriptError, MacroMappingDecl, PhysicalMacroDecl, SourceLocation


def is_macro_import(path):
    return path.split('?', 1)[0].split('#', 1)[0].lower().endswith('.json')


@dataclass(frozen=True, slots=True)
class MacroAssetReference:
    asset: str
    base_directory: str
    location: SourceLocation
    locked: bool = False
    offline: bool = False


@dataclass(frozen=True, slots=True)
class MacroBinding:
    name: str
    asset: MacroAssetReference
    scope: str
    mappings: tuple[MacroMappingDecl, ...]
    location: SourceLocation


@dataclass(frozen=True, slots=True)
class PreparedMacroBinding:
    binding: MacroBinding
    path: Path
    digest: str
    provenance: dict[str, str]


class MacroPreparationError(ValueError):
    def __init__(self, binding, message):
        self.location = binding.location
        self.message = f"macro {binding.name!r} ({binding.asset.asset}): {message}"
        super().__init__(f"{self.location}: MAC005: {self.message}")


def compile_macro_bindings(document, assets):
    result = []
    names = set()
    for decl in document.declarations:
        if not isinstance(decl, PhysicalMacroDecl):
            continue
        if decl.name in names:
            raise CopperScriptError("MAC001", f"duplicate physical macro {decl.name!r}", decl.location)
        names.add(decl.name)
        asset = assets.get(decl.asset_alias)
        if asset is None:
            raise CopperScriptError("MAC001", f"{decl.asset_alias!r} is not a macro asset import", decl.location)
        result.append(MacroBinding(decl.name, asset, decl.scope, decl.mappings, decl.location))
    return tuple(result)


def resolve_macro_bindings(flat):
    """Resolve through elaborated aliases, including boundary nets merged into parents."""
    components = {c.ref for c in flat.components}
    modules = {m.path: m for m in flat.module_instances}
    aliases = {n.name: n.name for n in flat.nets}
    aliases.update(flat.net_aliases)
    names, owners, result = set(), {}, []
    for binding in flat.physical_macros:
        if binding.name in names:
            raise CopperScriptError("MAC001", f"duplicate physical macro {binding.name!r}", binding.location)
        names.add(binding.name)
        if binding.scope and binding.scope not in modules:
            raise CopperScriptError("MAC002", f"unknown macro module scope {binding.scope!r}", binding.location)
        mappings = []
        members, nets = set(), set()
        for mapping in binding.mappings:
            target = mapping.target
            qualified = f"{binding.scope}/{target}" if binding.scope else target
            if mapping.kind == "member":
                if qualified not in components:
                    raise CopperScriptError("MAC002", f"unknown macro component {qualified!r}", mapping.location)
                if qualified in members or qualified in owners:
                    previous = owners.get(qualified, binding)
                    raise CopperScriptError("MAC003", f"component {qualified!r} already owned by macro {previous.name!r} at {previous.location}", mapping.location)
                members.add(qualified)
                owners[qualified] = binding
                value = qualified
            else:
                if "." in target:
                    owner, port = target.split(".", 1)
                    if owner == "port":
                        if binding.scope:
                            value = modules[binding.scope].connections.get(port)
                        else:
                            found = [n.name for n in flat.nets if any(e.component == "port" and e.pin == port for e in n.endpoints)]
                            value = found[0] if len(found) == 1 else None
                    else:
                        module = modules.get(f"{binding.scope}/{owner}" if binding.scope else owner)
                        value = module.connections.get(port) if module else None
                else:
                    value = aliases.get(qualified)
                if value is None:
                    raise CopperScriptError("MAC002", f"unknown macro net or module port {qualified!r}", mapping.location)
                if value in nets:
                    raise CopperScriptError("MAC003", f"distinct macro net roles resolve to {value!r}", mapping.location)
                nets.add(value)
            mappings.append(replace(mapping, target=value))
        result.append(replace(binding, scope="", mappings=tuple(mappings)))
    return tuple(result)


def prepare_macro_asset(ref, *, locked=None, offline=None):
    """Read one verified data contract; callers may cache this immutable snapshot."""
    from .packages import PackageResolver
    from .hard_macros import validate_macro_asset_data
    use_locked = ref.locked if locked is None else locked
    use_offline = ref.offline if offline is None else offline
    provenance = {"asset": ref.asset}
    if ref.asset.startswith(("github.com/", "gitlab.com/", "https://")):
        resolver = PackageResolver.for_source(Path(ref.base_directory), ref.location,
                                              locked=use_locked, offline=use_offline)
        asset = resolver.resolve_asset(ref.asset, ref.location)
        path, digest = asset.path, asset.sha256
        provenance.update(module=asset.module_path, version=asset.version,
                          inventory_checksum=asset.checksum, resolution="managed")
    else:
        unresolved = Path(ref.base_directory) / ref.asset
        if any(p.is_symlink() for p in (unresolved, *unresolved.parents)):
            raise ValueError("local macro assets may not be symlinks")
        path = unresolved.resolve()
        digest = sha256(path.read_bytes()).hexdigest()
        provenance["resolution"] = "local_input"
    raw = path.read_bytes()
    if sha256(raw).hexdigest() != digest:
        raise ValueError("macro changed after inventory verification")
    data = json.loads(raw)
    validate_macro_asset_data(data)
    provenance.update(source_path=str(path), asset_sha256=digest)
    return path, digest, data, provenance


def prepare_macro_bindings(board, *, locked=None, offline=None):
    """Prepare verified asset snapshots, without claiming physical connectivity."""
    from .design import Design
    from .elaborate import elaborate
    from .model import FlatElectricalView
    if isinstance(board, Design):
        board = board.electrical
    bindings = resolve_macro_bindings(board if isinstance(board, FlatElectricalView) else elaborate(board))
    snapshots, result = {}, []
    for binding in bindings:
        ref = binding.asset
        use_locked = ref.locked if locked is None else locked
        use_offline = ref.offline if offline is None else offline
        key = (ref.asset, ref.base_directory, use_locked, use_offline)
        try:
            if key not in snapshots:
                snapshots[key] = prepare_macro_asset(ref, locked=use_locked, offline=use_offline)
            path, digest, data, provenance = snapshots[key]
            expected_members = [row["reference"] for row in data["members"]]
            expected_nets = {row[2] for row in data["pad_nets"]}
            actual_members = {m.key for m in binding.mappings if m.kind == "member"}
            actual_nets = {m.key for m in binding.mappings if m.kind == "net"}
            if len(set(expected_members)) != len(expected_members) or actual_members != set(expected_members):
                raise ValueError(f"member mappings must cover asset members exactly; expected {sorted(expected_members)}, got {sorted(actual_members)}")
            if actual_nets != expected_nets:
                raise ValueError(f"net mappings must cover asset roles exactly; expected {sorted(expected_nets)}, got {sorted(actual_nets)}")
            result.append(PreparedMacroBinding(binding, path, digest, provenance))
        except (CopperScriptError, OSError, ValueError, KeyError, TypeError, IndexError) as exc:
            raise MacroPreparationError(binding, str(exc)) from exc
    return tuple(result)


def apply_source_macros(physical, board, *, locked=None, offline=None):
    from .hard_macros import bind_hard_macro
    provenance = []
    for prepared in prepare_macro_bindings(board, locked=locked, offline=offline):
        binding = prepared.binding
        try:
            physical = bind_hard_macro(physical, prepared.path, expected_sha256=prepared.digest,
                                      name=binding.name,
                                      bindings={m.key: m.target for m in binding.mappings if m.kind == "member"},
                                      net_bindings={m.key: m.target for m in binding.mappings if m.kind == "net"})
        except (ValueError, KeyError) as exc:
            raise ValueError(f"{binding.location}: MAC006: macro {binding.name!r} ({binding.asset.asset}): {exc}") from exc
        provenance.append({**prepared.provenance, "name": binding.name,
                           "declaration": str(binding.location)})
    if provenance:
        physical = replace(physical, metadata={**physical.metadata,
                           "source_physical_macros": json.dumps(provenance, sort_keys=True)})
    return physical


def macro_binding_document(binding):
    def origin(location):
        path = Path(os.path.relpath(location.filename, binding.asset.base_directory)).as_posix()
        return f"{path}:{location.line}:{location.column}"
    return {"name": binding.name, "asset": binding.asset.asset,
            "base_directory": ".", "scope": binding.scope,
            "origin": origin(binding.location), "asset_origin": origin(binding.asset.location),
            "asset_validation": "pending_physical_preparation",
            "mappings": [{"kind": m.kind, "key": m.key, "target": m.target,
                          "origin": origin(m.location)} for m in binding.mappings]}


def macro_binding_identity(binding):
    """Ignore only diagnostic offsets when comparing editor electrical intent."""
    def neutral(location):
        return replace(location, offset=0, line=0, column=0)
    return replace(binding, location=neutral(binding.location),
                   asset=replace(binding.asset, location=neutral(binding.asset.location)),
                   mappings=tuple(replace(m, location=neutral(m.location)) for m in binding.mappings))


def source_macro_input_paths(physical):
    return tuple(Path(row["source_path"]) for row in
                 json.loads(physical.metadata.get("source_physical_macros", "[]")))


def verify_source_macros(flat, physical, *, stage, stale_drc=False):
    """Verify source selection and owner preservation; no RF/power qualification."""
    if not flat.physical_macros:
        return []
    from .hard_macros import validate_hard_macros
    rows = []
    try:
        prepared = {p.binding.name: p for p in prepare_macro_bindings(flat, locked=True, offline=True)}
        preparation_error = None
    except ValueError as exc:
        prepared, preparation_error = {}, str(exc)
    macros = {m.cluster: m for m in physical.hard_macros}
    clusters = {c.name: c for c in physical.rigid_clusters}
    for binding in resolve_macro_bindings(flat):
        status, detail = 'blocked', 'physical macro copper is not materialized'
        snapshot = prepared.get(binding.name)
        if stale_drc:
            detail = 'DRC evidence belongs to different physical geometry'
        elif preparation_error:
            detail = preparation_error
        elif binding.name not in macros or binding.name not in clusters:
            status, detail = 'fail', 'source-selected physical macro is missing'
        elif snapshot is not None:
            try:
                macro = macros[binding.name]
                raw = snapshot.path.read_bytes()
                if sha256(raw).hexdigest() != snapshot.digest or macro.asset_sha256 != snapshot.digest:
                    raise ValueError('physical macro asset differs from the current source-selected asset')
                data = json.loads(raw)
                members = {m.key: m.target for m in binding.mappings if m.kind == 'member'}
                nets = {m.key: m.target for m in binding.mappings if m.kind == 'net'}
                expected_pads = {(members[member], pad, nets[role]) for member, pad, role in data['pad_nets']}
                actual_pads = {(p.pad.component, p.pad.pad, p.net) for p in macro.pad_bindings}
                if expected_pads != actual_pads or set(members.values()) != {m.reference for m in clusters[binding.name].members}:
                    raise ValueError('physical macro member or pad/net mapping differs from source')
                validate_hard_macros(physical)
                if binding.name in physical.materialized_macros:
                    status, detail = 'pass', 'source-selected asset, exact bindings and immutable owner copper verified; electrical/RF/power and native-fill qualification remain separate'
                elif stage == 'placement':
                    detail = 'binding is present; owner copper acceptance requires materialization'
            except (ValueError, OSError, KeyError, TypeError) as exc:
                status, detail = 'fail', str(exc)
        rows.append({'id': f'physical_macro:{binding.name}', 'kind': 'physical_macro',
                     'targets': [m.target for m in binding.mappings if m.kind == 'member'],
                     'mode': 'require', 'required': True, 'origins': [str(binding.location)],
                     'parameters': {'asset': binding.asset.asset}, 'status': status,
                     'detail': detail, 'measurements': [],
                     'verifier': 'copperscript-source-macro/v0.1',
                     'declared_consumers': ['physicalizer', 'router'], 'declared_verifier': None})
    return rows
