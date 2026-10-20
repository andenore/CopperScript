"""Compact, read-only questions about a compiled design.

Queries run on the derived flat electrical view, so hierarchical names use
``INSTANCE/REF``. They report declared intent and connectivity only; they do not
run ERC, placement or routing, and they never qualify a design.
"""

from __future__ import annotations

from difflib import get_close_matches

from .erc import _Context, build_context, no_connect_endpoints, unmarked_pins
from .model import Board, ConnectionPolicy, Endpoint, FlatElectricalView

QUERY_SCHEMA = "copperscript.query/v1"
QUERY_KINDS = ("components", "component", "pins", "nets", "net", "unconnected")


class QueryError(ValueError):
    pass


_FACTS: dict[int, tuple] = {}


def _facts(context: _Context) -> tuple:
    """Per-query derived indexes (open pins, unmarked pins, decoupling, functions)."""
    key = id(context)
    if key not in _FACTS:
        _FACTS[key] = (no_connect_endpoints(context), unmarked_pins(context),
                       _decoupling(context), _functions(context))
    return _FACTS[key]


def run_query(board: Board | FlatElectricalView, kind: str, name: str | None = None) -> dict[str, object]:
    context = build_context(board)
    _FACTS.clear()
    try:
        return _run(context, kind, name)
    finally:
        _FACTS.clear()


def _run(context: _Context, kind: str, name: str | None) -> dict[str, object]:
    if kind in {"component", "pins", "net"} and not name:
        raise QueryError(f"`query {kind}` needs a {'net name' if kind == 'net' else 'component reference'}")
    handlers = {
        "components": lambda: {"components": [_component_row(context, ref) for ref in _refs(context)]},
        "component": lambda: {"component": _component_detail(context, _component(context, name))},
        "pins": lambda: {"component": _component(context, name),
                         "pins": _pin_rows(context, _component(context, name))},
        "nets": lambda: {"nets": [_net_row(context, net.name) for net in context.board.nets]},
        "net": lambda: {"net": _net_detail(context, _net(context, name))},
        "unconnected": lambda: {"components": _unconnected(context)},
    }
    if kind not in handlers:
        raise QueryError(f"unknown query {kind!r}; expected one of: {', '.join(QUERY_KINDS)}")
    return {"schema": QUERY_SCHEMA, "board": context.board.name, "query": kind, **handlers[kind]()}


# --- lookups ----------------------------------------------------------------------------

def _refs(context: _Context) -> list[str]:
    return list(dict.fromkeys(component.ref for component in context.board.components))


def _component(context: _Context, ref: str) -> str:
    if ref in context.components:
        return ref
    raise QueryError(_not_found("component", ref, context.components))


def _net(context: _Context, name: str) -> str:
    aliases = context.board.net_aliases
    resolved = aliases.get(name, name)
    if resolved in context.nets:
        return resolved
    raise QueryError(_not_found("net", name, context.nets))


def _not_found(label: str, name: str, candidates) -> str:
    match = get_close_matches(name, sorted(candidates), n=1, cutoff=0.6)
    hint = f"; did you mean {match[0]!r}?" if match else f"; `copper query BOARD {label}s` lists them"
    return f"no {label} {name!r}{hint}"


def _decoupling(context: _Context) -> dict[Endpoint, list[str]]:
    result: dict[Endpoint, list[str]] = {}
    for component in context.board.components:
        target = component.properties.get("decouples")
        if not target:
            continue
        try:
            endpoint = Endpoint.parse(target)
        except ValueError:
            continue
        result.setdefault(context.canonical_endpoint(endpoint) or endpoint, []).append(component.ref)
    return result


def _functions(context: _Context) -> dict[Endpoint, list[str]]:
    result: dict[Endpoint, list[str]] = {}
    for selection in context.board.peripheral_selections:
        for signal, chosen in selection.signals.items():
            endpoint = Endpoint(selection.component, chosen.pin)
            result.setdefault(context.canonical_endpoint(endpoint) or endpoint, []).append(
                f"{selection.peripheral}.{signal}")
    return result


def _profile_text(profile) -> str | None:
    if profile is None:
        return None
    domains = "/".join(sorted(item.value for item in profile.domains))
    directions = "/".join(sorted(item.value for item in profile.directions))
    return " ".join(part for part in (domains, directions) if part) or None


def _value_text(value) -> str | None:
    return None if value is None else str(value)


# --- rows -------------------------------------------------------------------------------

def _pin_rows(context: _Context, ref: str) -> list[dict[str, object]]:
    component = context.components[ref]
    part = context.board.library.get(component.part)
    if part is None:
        raise QueryError(f"component {ref} uses unknown part {component.part!r}; run `copper check`")
    open_pins, all_unmarked, decoupling, functions = _facts(context)
    unmarked = set(all_unmarked.get(ref, ()))
    rows = []
    for pin in part.pins.values():
        endpoint = Endpoint(ref, pin.name)
        nets = context.pin_to_nets.get(endpoint, [])
        if nets:
            status = "connected"
        elif endpoint in open_pins:
            status = "no_connect"
        elif pin.connection_policy is ConnectionPolicy.DO_NOT_CONNECT:
            status = "part_do_not_connect"
        elif pin.name in unmarked:
            status = "unmarked"
        else:
            status = _open_status(context, component, part, pin)
        row: dict[str, object] = {
            "pin": pin.name, "number": pin.number, "nets": list(nets), "status": status,
            "type": _profile_text(context.pin_profile(component, part, pin)),
        }
        if pin.connection_policy is not ConnectionPolicy.NORMAL:
            row["policy"] = pin.connection_policy.value
        if endpoint in open_pins and open_pins[endpoint]:
            row["reason"] = open_pins[endpoint]
        if decoupling.get(endpoint):
            row["decoupled_by"] = decoupling[endpoint]
        if functions.get(endpoint):
            row["functions"] = functions[endpoint]
        rows.append(row)
    return rows


def _open_status(context: _Context, component, part, pin) -> str:
    """Why an unconnected pin is not counted as unmarked (see erc.unmarked_pins)."""
    from .erc import _is_power_input
    if pin.connection_policy is ConnectionPolicy.OPTIONAL:
        return "optional"
    if pin.connection_policy is ConnectionPolicy.REQUIRED:
        return "required_unconnected"
    profile = context.pin_profile(component, part, pin)
    if profile is None:
        return "unmodeled"
    if _is_power_input(profile):
        return "power_input_unconnected"
    return "open"


def _component_row(context: _Context, ref: str) -> dict[str, object]:
    component = context.components[ref]
    row: dict[str, object] = {"ref": ref, "part": component.part, "value": _value_text(component.value)}
    if component.part in context.board.library:
        statuses = [item["status"] for item in _pin_rows(context, ref)]
        row["pins"] = {"total": len(statuses), "connected": statuses.count("connected"),
                       "no_connect": statuses.count("no_connect"), "unmarked": statuses.count("unmarked")}
    return row


def _component_detail(context: _Context, ref: str) -> dict[str, object]:
    component = context.components[ref]
    detail = _component_row(context, ref)
    detail.update({"footprint": component.footprint, "properties": dict(component.properties),
                   "modes": dict(component.modes)})
    if component.part in context.board.library:
        detail["pin_list"] = _pin_rows(context, ref)
    return detail


def _net_row(context: _Context, name: str) -> dict[str, object]:
    net = context.nets[name]
    supply = next((item for item in context.board.supplies if item.net == name), None)
    row: dict[str, object] = {"net": name, "endpoints": len(net.endpoints)}
    if supply is not None:
        row["supply"] = {"name": supply.name, "voltage": str(supply.voltage),
                         "source": str(supply.source) if supply.source else None,
                         "external": supply.externally_driven}
    return row


def _net_detail(context: _Context, name: str) -> dict[str, object]:
    net = context.nets[name]
    detail = _net_row(context, name)
    decoupling = _facts(context)[2]
    members = []
    for endpoint in net.endpoints:
        resolved = context.resolve_pin(endpoint)
        member: dict[str, object] = {"endpoint": str(endpoint)}
        component = context.components.get(endpoint.component)
        if component is not None:
            member["part"] = component.part
        if resolved is not None:
            member["type"] = _profile_text(context.pin_profile(*resolved))
            canonical = Endpoint(endpoint.component, resolved[2].name)
            if decoupling.get(canonical):
                member["decoupled_by"] = decoupling[canonical]
        else:
            member["unresolved"] = True
        members.append(member)
    detail["members"] = members
    interfaces = sorted(item.name for item in context.board.interfaces if name in item.signals.values())
    if interfaces:
        detail["interfaces"] = interfaces
    members = {context.canonical_endpoint(e) or e for e in net.endpoints}
    caps = sorted({ref for target, refs in decoupling.items() if target in members for ref in refs})
    if caps:
        detail["decoupling_capacitors"] = caps
    return detail


def _unconnected(context: _Context) -> list[dict[str, object]]:
    rows = []
    for ref in _refs(context):
        if context.components[ref].part not in context.board.library:
            continue
        pins = _pin_rows(context, ref)
        groups: dict[str, list[object]] = {}
        for pin in pins:
            if pin["status"] == "connected":
                continue
            entry = {"pin": pin["pin"], "reason": pin["reason"]} if pin.get("reason") else pin["pin"]
            groups.setdefault(pin["status"], []).append(entry)
        if groups:
            rows.append({"ref": ref, **groups})
    return rows


# --- text rendering ---------------------------------------------------------------------

def _table(headers: list[str], rows: list[list[str]]) -> str:
    widths = [max(len(headers[i]), *(len(row[i]) for row in rows)) if rows else len(headers[i])
              for i in range(len(headers))]
    lines = ["  ".join(h.ljust(w) for h, w in zip(headers, widths)).rstrip()]
    lines.extend("  ".join(c.ljust(w) for c, w in zip(row, widths)).rstrip() for row in rows)
    return "\n".join(lines)


def _pin_table(pins: list[dict[str, object]]) -> str:
    rows = []
    for pin in pins:
        notes = []
        if pin.get("reason"):
            notes.append(f"reason: {pin['reason']}")
        if pin.get("decoupled_by"):
            notes.append("decoupled by " + ", ".join(pin["decoupled_by"]))
        if pin.get("functions"):
            notes.append(", ".join(pin["functions"]))
        if pin.get("policy"):
            notes.append(f"policy {pin['policy']}")
        rows.append([str(pin["pin"]), str(pin["number"]), ", ".join(pin["nets"]) or "-",
                     str(pin["status"]), str(pin["type"] or "-"), "; ".join(notes)])
    return _table(["PIN", "NO", "NET", "STATUS", "TYPE", "NOTES"], rows)


def render_query(result: dict[str, object]) -> str:
    kind = result["query"]
    if kind == "pins":
        return f"{result['component']} pins\n" + _pin_table(result["pins"])
    if kind == "component":
        c = result["component"]
        head = [f"{c['ref']}: {c['part']}" + (f" = {c['value']}" if c.get("value") else "")]
        if c.get("footprint"):
            head.append(f"footprint: {c['footprint']}")
        for key, value in sorted(c.get("properties", {}).items()):
            head.append(f"{key}: {value}")
        return "\n".join(head + ([_pin_table(c["pin_list"])] if "pin_list" in c else []))
    if kind == "components":
        rows = []
        for c in result["components"]:
            pins = c.get("pins")
            pin_text = (f"{pins['connected']}/{pins['total']} connected, {pins['no_connect']} nc, "
                        f"{pins['unmarked']} unmarked") if pins else "unknown part"
            rows.append([c["ref"], c["part"], c.get("value") or "-", pin_text])
        return _table(["REF", "PART", "VALUE", "PINS"], rows)
    if kind == "nets":
        rows = []
        for n in result["nets"]:
            supply = n.get("supply")
            rows.append([n["net"], str(n["endpoints"]),
                         f"{supply['voltage']}" + (" external" if supply["external"] else "") if supply else "-"])
        return _table(["NET", "ENDPOINTS", "SUPPLY"], rows)
    if kind == "net":
        n = result["net"]
        head = [f"net {n['net']} ({n['endpoints']} endpoints)"]
        if n.get("supply"):
            s = n["supply"]
            head.append(f"supply {s['name']} = {s['voltage']}" + (f", source {s['source']}" if s["source"] else "")
                        + (", external" if s["external"] else ""))
        if n.get("interfaces"):
            head.append("interfaces: " + ", ".join(n["interfaces"]))
        rows = [[m["endpoint"], str(m.get("part", "?")), str(m.get("type") or ("unresolved" if m.get("unresolved") else "-")),
                 ("decoupled by " + ", ".join(m["decoupled_by"])) if m.get("decoupled_by") else ""]
                for m in n["members"]]
        return "\n".join(head) + "\n" + _table(["ENDPOINT", "PART", "TYPE", "NOTES"], rows)
    if kind == "unconnected":
        if not result["components"]:
            return "Every modeled pin is on a net."
        lines = []
        for c in result["components"]:
            parts = []
            for status in ("unmarked", "required_unconnected", "power_input_unconnected", "no_connect",
                           "part_do_not_connect", "optional", "unmodeled", "open"):
                if status in c:
                    names = [item if isinstance(item, str) else f"{item['pin']} ({item['reason']})" for item in c[status]]
                    parts.append(f"{status}: {', '.join(names)}")
            lines.append(f"{c['ref']}  " + " | ".join(parts))
        return "\n".join(lines)
    raise QueryError(f"cannot render {kind!r}")
