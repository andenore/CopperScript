"""A stand-in for KiCad's Python running pcbir/editor/native_overlay.py.

The comparison harness has no KiCad. This reads the saved ``.kicad_pcb`` text
(the last argument, as the probe receives it from either implementation) and
prints the same JSON the pcbnew probe prints: enabled copper layers, footprint
poses, segments, vias and the ``filled_polygon`` rings zones carry. It never
fills, modifies or saves a board, and it is not native evidence of anything.
"""
from decimal import Decimal
import json
import sys


def tokens(text):
    index, length = 0, len(text)
    while index < length:
        character = text[index]
        if character.isspace():
            index += 1
        elif character in "()":
            yield character
            index += 1
        elif character == '"':
            end = index + 1
            while text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            yield ("string", text[index + 1:end])
            index = end + 1
        else:
            end = index
            while end < length and not text[end].isspace() and text[end] not in "()":
                end += 1
            yield ("atom", text[index:end])
            index = end


def read(stream):
    items = []
    for token in stream:
        if token == "(":
            items.append(read(stream))
        elif token == ")":
            return items
        else:
            items.append(token)
    return items


def nm(atom):
    return int((Decimal(atom[1]) * 1000000).to_integral_value())


def children(node, name):
    return [item for item in node if isinstance(item, list) and item and item[0] == ("atom", name)]


def child(node, name):
    found = children(node, name)
    return found[0] if found else None


def extract(filename):
    with open(filename, encoding="utf-8") as stream:
        board = read(tokens(stream.read()))[0]
    layers = [item[1][1] for item in child(board, "layers")[1:] if item[1][1].endswith(".Cu")]
    nets = {item[1][1]: item[2][1] for item in children(board, "net")}
    poses, tracks, vias, fills = [], [], [], []
    for footprint in children(board, "footprint"):
        at = child(footprint, "at")
        reference = next(p for p in children(footprint, "property") if p[1][1] == "Reference")[2][1]
        poses.append({"reference": reference, "position": [nm(at[1]), nm(at[2])],
                      "rotation": str(float(at[3][1]) if len(at) > 3 else 0.0),
                      "side": "back" if child(footprint, "layer")[1][1] == "B.Cu" else "front",
                      "library_id": footprint[1][1]})
    for segment in children(board, "segment"):
        start, end = child(segment, "start"), child(segment, "end")
        tracks.append({"net": nets[child(segment, "net")[1][1]], "start": [nm(start[1]), nm(start[2])],
                       "end": [nm(end[1]), nm(end[2])], "width_nm": nm(child(segment, "width")[1]),
                       "layer": child(segment, "layer")[1][1]})
    for via in children(board, "via"):
        at = child(via, "at")
        span = child(via, "layers")
        vias.append({"net": nets[child(via, "net")[1][1]], "position": [nm(at[1]), nm(at[2])],
                     "size_nm": nm(child(via, "size")[1]), "drill_nm": nm(child(via, "drill")[1]),
                     "via_type": "non-through" if any(item == ("atom", kind) for item in via for kind in ("blind", "micro")) else "through",
                     "from_layer": span[1][1], "to_layer": span[2][1]})
    for zone in children(board, "zone"):
        if child(zone, "keepout"):
            continue
        for polygon in children(zone, "filled_polygon"):
            points = [[nm(xy[1]), nm(xy[2])] for xy in children(child(polygon, "pts"), "xy")]
            fills.append({"net": child(zone, "net_name")[1][1], "layer": child(polygon, "layer")[1][1],
                          "outer": points, "holes": []})
    return {"kicad_version": "stand-in (no KiCad)", "layers": layers, "poses": poses, "tracks": tracks,
            "vias": vias, "fills": fills}


if __name__ == "__main__":
    try:
        if len(sys.argv) < 2:
            raise ValueError("one explicit board filename is required")
        print(json.dumps(extract(sys.argv[-1]), allow_nan=False))
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(2)
