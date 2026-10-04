"""Declarative mechanical-profile composition, ownership and explicit role binding."""
from dataclasses import dataclass, replace
from typing import Mapping

from .syntax import (CopperScriptError, MechanicalItemDecl, MechanicalProfileUseDecl,
                     SourceLocation)


@dataclass(frozen=True, slots=True)
class MechanicalProfileDefinition:
    name: str
    location: SourceLocation
    items: tuple[MechanicalItemDecl | MechanicalProfileUseDecl, ...]


@dataclass(frozen=True, slots=True)
class MechanicalProfileInstance:
    name: str
    profile: str
    location: SourceLocation
    bindings: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True, slots=True)
class MechanicalFeatureSource:
    kind: str
    name: str
    location: SourceLocation
    profile: str | None = None
    instance: str | None = None


def expand_mechanical_items(items, profiles: Mapping[str, MechanicalProfileDefinition]):
    """Return expanded items, exact owners and retained profile instance tree paths."""
    result, sources, instances = [], [], []

    def error(message, location):
        raise CopperScriptError("MEC004", message, location)

    def definition(use, stack):
        if use.profile not in profiles:
            error(f"unknown board profile {use.profile!r}", use.location)
        if use.profile in stack:
            error(f"cyclic board profile use: {' -> '.join((*stack, use.profile))}", use.location)
        return profiles[use.profile]

    def roles(use, stack=()):
        d = definition(use, stack)
        required = []
        for item in d.items:
            if isinstance(item, MechanicalProfileUseDecl):
                child = roles(item, (*stack, use.profile))
                if set(item.bindings) != child or any(not isinstance(v, str) or not v for v in item.bindings.values()):
                    error("nested profile bindings must map every child role to an outer role", item.location)
                required.extend(item.bindings.values())
            elif item.kind in {"connector", "attach"}:
                required.append(item.name)
        if len(required) != len(set(required)):
            error("profile contains duplicate connector roles", d.location)
        return set(required)

    def append(item, profile=None, path=None, binding=None):
        name = f"{path}/{item.name}" if path and item.name else item.name
        parameters = dict(item.parameters)
        if item.kind in {"connector", "attach"} and profile is not None:
            if "component" in parameters:
                error("profile connector components must be supplied through explicit bindings", item.location)
            parameters["component"] = binding
        if path:
            for key in ("relative_to", "target"):
                if key in parameters:
                    if not isinstance(parameters[key], str):
                        error("profile mechanical targets must be named references", item.location)
                    parameters[key] = f"{path}/{parameters[key]}"
        result.append(replace(item, name=name, parameters=parameters))
        sources.append(MechanicalFeatureSource(item.kind, name, item.location, profile, path))

    def expand(use, path, bindings, stack=()):
        d = definition(use, stack)
        required = roles(use, stack)
        if set(bindings) != required or any(not isinstance(v, str) or not v for v in bindings.values()):
            error(f"profile {use.profile!r} requires exactly these connector bindings: {', '.join(sorted(required)) or '(none)'}", use.location)
        instances.append(MechanicalProfileInstance(path, d.name, use.location, tuple(sorted(bindings.items()))))
        child_names = set()
        for item in d.items:
            if isinstance(item, MechanicalProfileUseDecl):
                if item.instance in child_names:
                    error("duplicate profile instance name", item.location)
                child_names.add(item.instance)
                expand(item, f"{path}/{item.instance}",
                       {key: bindings[value] for key, value in item.bindings.items()}, (*stack, use.profile))
            else:
                append(item, d.name, path, bindings.get(item.name))

    names = set()
    for item in items:
        if isinstance(item, MechanicalProfileUseDecl):
            if item.instance in names:
                error("duplicate profile instance name", item.location)
            names.add(item.instance)
            expand(item, item.instance, item.bindings)
        else:
            append(item)
    return tuple(result), tuple(sources), tuple(instances)


def mechanical_provenance(mechanical):
    location = lambda loc: {"filename": loc.filename, "line": loc.line, "column": loc.column}
    return {
        "profiles": [{"name": p.name, "profile": p.profile, "bindings": dict(p.bindings),
                      "source": location(p.location)} for p in mechanical.profiles],
        "features": [{"kind": s.kind, "name": s.name, "source": location(s.location),
                      "profile": s.profile, "instance": s.instance, "read_only": s.profile is not None}
                     for s in mechanical.sources],
    }
