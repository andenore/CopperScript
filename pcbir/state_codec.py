"""Deterministic JSON save/load for CopperScript objects (boards, plans, snapshots).

Iteration scripts used to ship their own codec to keep intermediate solver
states between runs. This module is the shared one: any frozen dataclass, enum,
tuple, frozenset, mapping, Decimal or Fraction made of ``pcbir`` types round-trips
exactly, and the file carries a content digest.

Decoding only constructs dataclasses and enums that are *defined* in ``pcbir``
modules (looked up by their own module and qualified name, never through other
modules' imports) and never executes code from the file. Plain classes,
``init=False`` fields and other non-dataclass objects are not supported. A state file is a working artifact, not evidence: it does
not replace a route report, a native check or a lock.
"""

from __future__ import annotations

import enum
import hashlib
import importlib
import json
from dataclasses import fields, is_dataclass
from decimal import Decimal
from fractions import Fraction
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping

STATE_SCHEMA = "copperscript.state/v1"
_ALLOWED_PREFIX = "pcbir"


class StateError(ValueError):
    pass


def encode(value: Any) -> Any:
    if isinstance(value, enum.Enum):  # before str/int: many pcbir enums subclass str
        return {"$enum": _qualname(type(value)), "value": encode(value.value)}
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, Decimal):
        return {"$decimal": str(value)}
    if isinstance(value, Fraction):
        return {"$fraction": [value.numerator, value.denominator]}
    if is_dataclass(value) and not isinstance(value, type):
        return {"$type": _qualname(type(value)),
                "fields": {f.name: encode(getattr(value, f.name)) for f in fields(value) if f.init}}
    if isinstance(value, tuple):
        return {"$tuple": [encode(item) for item in value]}
    if isinstance(value, (frozenset, set)):
        items = [encode(item) for item in value]
        return {"$frozenset" if isinstance(value, frozenset) else "$set":
                sorted(items, key=lambda item: json.dumps(item, sort_keys=True))}
    if isinstance(value, list):
        return [encode(item) for item in value]
    if isinstance(value, (dict, MappingProxyType)) or isinstance(value, Mapping):
        return {"$map": [[encode(k), encode(v)] for k, v in value.items()],
                "proxy": isinstance(value, MappingProxyType)}
    raise StateError(f"cannot encode {type(value).__module__}.{type(value).__qualname__}")


def decode(value: Any) -> Any:
    if isinstance(value, list):
        return [decode(item) for item in value]
    if not isinstance(value, dict):
        return value
    if "$type" in value:
        cls = _resolve(value["$type"])
        if not is_dataclass(cls):
            raise StateError(f"{value['$type']} is not a dataclass")
        allowed = {f.name for f in fields(cls) if f.init}
        unknown = set(value["fields"]) - allowed
        if unknown:
            raise StateError(f"{value['$type']} has no init field(s) {sorted(unknown)}")
        return cls(**{name: decode(item) for name, item in value["fields"].items()})
    if "$enum" in value:
        cls = _resolve(value["$enum"])
        if not issubclass(cls, enum.Enum):
            raise StateError(f"{value['$enum']} is not an enum")
        return cls(decode(value["value"]))
    if "$decimal" in value:
        return Decimal(value["$decimal"])
    if "$fraction" in value:
        return Fraction(*value["$fraction"])
    if "$tuple" in value:
        return tuple(decode(item) for item in value["$tuple"])
    if "$frozenset" in value:
        return frozenset(decode(item) for item in value["$frozenset"])
    if "$set" in value:
        return {decode(item) for item in value["$set"]}
    if "$map" in value:
        mapping = {decode(k): decode(v) for k, v in value["$map"]}
        return MappingProxyType(mapping) if value.get("proxy") else mapping
    return {key: decode(item) for key, item in value.items()}


def save_state(path: str | Path, value: Any, **metadata: Any) -> Path:
    """Write ``value`` and JSON-serializable ``metadata`` to a new state file."""

    path = Path(path)
    payload = encode(value)
    body = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    # "schema" is written first so readers can recognise a state file from its first bytes.
    head = json.dumps({"schema": STATE_SCHEMA}, separators=(",", ":"))[:-1]
    rest = json.dumps({"kind": _qualname(type(value)), "metadata": metadata,
                       "content_digest": "sha256:" + hashlib.sha256(body.encode()).hexdigest(), "value": payload},
                      sort_keys=True, separators=(",", ":"))[1:]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(head + "," + rest + "\n", encoding="utf-8")
    return path


def is_state_file(path: str | Path) -> bool:
    try:
        with Path(path).open(encoding="utf-8") as handle:
            return handle.read(64).startswith('{"schema":"' + STATE_SCHEMA + '"')
    except (OSError, UnicodeDecodeError):
        return False


def load_state(path: str | Path, *, with_metadata: bool = False):
    document = json.loads(Path(path).read_text(encoding="utf-8"))
    if document.get("schema") != STATE_SCHEMA:
        raise StateError(f"{path}: not a {STATE_SCHEMA} file")
    body = json.dumps(document["value"], sort_keys=True, separators=(",", ":"))
    if "sha256:" + hashlib.sha256(body.encode()).hexdigest() != document.get("content_digest"):
        raise StateError(f"{path}: content digest mismatch")
    value = decode(document["value"])
    return (value, document.get("metadata", {})) if with_metadata else value


def describe_state(path: str | Path) -> dict[str, Any]:
    document = json.loads(Path(path).read_text(encoding="utf-8"))
    if document.get("schema") != STATE_SCHEMA:
        raise StateError(f"{path}: not a {STATE_SCHEMA} file")
    return {"kind": document.get("kind"), "metadata": document.get("metadata", {}),
            "content_digest": document.get("content_digest"), "bytes": Path(path).stat().st_size}


def _qualname(cls: type) -> str:
    return f"{cls.__module__}:{cls.__qualname__}"


def _resolve(name: str) -> type:
    """A class defined in a pcbir module, found by its own module and qualified name."""

    module_name, _, qualname = name.partition(":")
    if not (module_name == _ALLOWED_PREFIX or module_name.startswith(_ALLOWED_PREFIX + ".")) or not qualname:
        raise StateError(f"refusing to construct {name!r}: only pcbir types are allowed")
    try:
        target: Any = importlib.import_module(module_name)
        for part in qualname.split("."):
            target = target.__dict__[part] if isinstance(target, type) or hasattr(target, "__dict__") else None
    except (ImportError, KeyError, TypeError) as exc:
        raise StateError(f"unknown type {name!r}") from exc
    # Re-exported names (e.g. a module's imports) are rejected: the class must live here.
    if (not isinstance(target, type) or getattr(target, "__module__", None) != module_name
            or getattr(target, "__qualname__", None) != qualname):
        raise StateError(f"refusing to construct {name!r}: not a class defined in {module_name}")
    if not (is_dataclass(target) or issubclass(target, enum.Enum)):
        raise StateError(f"refusing to construct {name!r}: only dataclasses and enums are allowed")
    return target
