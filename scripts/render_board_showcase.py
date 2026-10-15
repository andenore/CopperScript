"""Render native KiCad showcase views with visible pads and a fill-free stack.

Geometry is plotted by kicad-cli; only
temporary presentation copies lose tracks, zones or graphics. The input PCB
is never saved. This tool does not route, refill or qualify a board.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import xml.etree.ElementTree as ET

SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)
PAD_COLOR = "#D9A441"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def nodes(text):
    """Yield direct child expressions without rewriting any native geometry."""
    depth, quoted, escaped, start = 0, False, False, None
    for index, char in enumerate(text):
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char == "(":
            if depth == 1:
                start = index
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 1 and start is not None:
                value = text[start:index + 1]
                yield re.match(r"\(\s*([^\s()]+)", value).group(1), start, index + 1, value
    if depth or quoted:
        raise ValueError("Unbalanced native board expressions")


def omit(text, predicate):
    for tag, start, end, value in reversed(list(nodes(text))):
        if predicate(tag, value):
            text = text[:start] + text[end:]
    return text


def layer(value):
    for tag, _, _, child in nodes(value):
        if tag == "layer":
            return re.search(r'"([^"]+)"', child).group(1)
    return None


def native_geometry(text):
    children = list(nodes(text))
    footprints = [value for tag, _, _, value in children if tag == "footprint"]
    pads = [value for footprint in footprints for tag, _, _, value in nodes(footprint) if tag == "pad"]
    routes = [value for tag, _, _, value in children if tag in {"segment", "arc", "via"}]
    return {"footprints": len(footprints), "pads": len(pads), "tracks_and_vias": len(routes),
            "pads_sha256": hashlib.sha256("".join(pads).encode()).hexdigest(),
            "tracks_and_vias_sha256": hashlib.sha256("".join(routes).encode()).hexdigest()}


def hide_package_labels(text):
    """Hide overlapping reference/value text, keeping native package outlines."""
    for tag, start, end, value in reversed(list(nodes(text))):
        if tag != "footprint":
            continue
        for child_tag, left, right, child in reversed(list(nodes(value))):
            if child_tag == "property" and re.match(r'\(property\s+"(?:Reference|Value)"', child) and "(hide yes)" not in child:
                value = value[:left] + child[:-1] + " (hide yes))" + value[right:]
            elif child_tag == "fp_text" and re.match(r"\(fp_text\s+(?:reference|value)\b", child):
                value = value[:left] + value[right:]
        text = text[:start] + value + text[end:]
    return text


def plot(cli, source, layers, target):
    subprocess.run([str(cli), "pcb", "export", "svg", "--layers", layers,
                    "--mode-single", "--fit-page-to-board", "--exclude-drawing-sheet",
                    "--output", str(target), str(source)], check=True)
    text = target.read_text(encoding="utf-8")
    # Omit the export timestamp so the same native inputs yield the same asset.
    text = re.sub(r"<title>.*?</title>", "<title>Native KiCad board illustration</title>", text)
    target.write_text("\n".join(line.rstrip() for line in text.splitlines()) + "\n",
                      encoding="utf-8", newline="\n")


def highlight_pads(base, pads):
    artwork = ET.parse(base)
    overlay = ET.parse(pads).getroot()
    if artwork.getroot().get("viewBox") != overlay.get("viewBox"):
        raise ValueError("Pad overlay and board artwork have different bounds")
    group = ET.SubElement(artwork.getroot(), f"{{{SVG}}}g", {"class": "package-pads"})
    for child in overlay:
        if child.tag != f"{{{SVG}}}g":
            continue
        for element in child.iter():
            style = element.get("style")
            if style:
                # Only native copper colours change. White drill holes and the
                # grey board outline retain their native appearance.
                for color in ("#C83434", "#3434C8"):
                    style = style.replace(color, PAD_COLOR)
                element.set("style", style)
        group.append(child)
    base.write_bytes(ET.tostring(artwork.getroot(), encoding="utf-8", xml_declaration=True) + b"\n")


def render(pcb, output, cli):
    original = sha256(pcb)
    board = pcb.read_text(encoding="utf-8")
    layers_node = next(value for tag, _, _, value in nodes(board) if tag == "layers")
    copper_layers = re.findall(r'\(\d+\s+"([^"]+\.Cu)"', layers_node)
    inner_layer = "In2.Cu" if "In2.Cu" in copper_layers else next((name for name in copper_layers if name not in {"F.Cu", "B.Cu"}), "F.Cu")
    counts = native_geometry(board)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="copper-showcase-") as folder:
        work = Path(folder)
        # The all-layer view hides both native zones and solid copper graphics
        # used for macro fill areas. Pads, tracks and vias remain untouched.
        zones = [value for tag, _, _, value in nodes(board) if tag == "zone"]
        def copper_fill(tag, value):
            return tag.startswith("gr_") and (layer(value) or "").endswith(".Cu") and bool(re.search(r"\(fill\s+(?:yes|solid)\)", value))
        fills = [value for tag, _, _, value in nodes(board) if copper_fill(tag, value)]
        board = omit(board, lambda tag, value: tag == "zone" or copper_fill(tag, value))
        no_fills = work / "no-fills.kicad_pcb"
        no_fills.write_text(hide_package_labels(board), encoding="utf-8", newline="\n")
        if native_geometry(board) != counts:
            raise ValueError("Fill-free view changed pads, tracks or vias")

        # Native pad-only overlay: no inferred/reconstructed package geometry.
        board = omit(board, lambda tag, value: tag in {"segment", "arc", "via"} or
                     (tag.startswith("gr_") and layer(value) != "Edge.Cuts"))
        for tag, start, end, value in reversed(list(nodes(board))):
            if tag == "footprint":
                board = board[:start] + omit(value, lambda tag, _: tag.startswith("fp_")) + board[end:]
        if native_geometry(board)["pads_sha256"] != counts["pads_sha256"]:
            raise ValueError("Pad overlay changed native package pads")
        pads_only = work / "pads.kicad_pcb"
        pads_only.write_text(board, encoding="utf-8", newline="\n")
        placement = work / "placement.kicad_pcb"
        placement.write_text(hide_package_labels(pcb.read_text(encoding="utf-8")), encoding="utf-8", newline="\n")
        pad_plots = {}
        for side in ("F.Cu", "B.Cu"):
            target = work / (side + ".svg")
            plot(cli, pads_only, side + ",Edge.Cuts", target)
            pad_plots[side] = target

        views = {
            "placement.svg": (placement, "F.Fab,F.CrtYd,Edge.Cuts", ["F.Cu"]),
            "routing-top.svg": (pcb, "F.Cu,F.SilkS,Edge.Cuts", ["F.Cu"]),
            "routing-inner.svg": (pcb, inner_layer + ",Edge.Cuts", []),
            "routing-bottom.svg": (pcb, "B.Cu,Edge.Cuts", ["B.Cu"]),
            "routing-all.svg": (no_fills, ",".join(copper_layers + ["F.SilkS", "B.SilkS", "F.Fab", "B.Fab", "Edge.Cuts"]), ["B.Cu", "F.Cu"]),
        }
        for name, (source, layers, overlays) in views.items():
            target = output / name
            plot(cli, source, layers, target)
            for side in overlays:
                highlight_pads(target, pad_plots[side])
        metadata = {"pcb_sha256": original, "kicad_version": subprocess.check_output([str(cli), "version"], text=True).strip(),
                    "pad_color": PAD_COLOR, "native_geometry": counts,
                    "all_layers": copper_layers, "hidden_zones_in_all_layers": len(zones),
                    "hidden_copper_graphics_in_all_layers": len(fills),
                    "package_labels_hidden": ["placement.svg", "routing-all.svg"],
                    "views": {name: {"layers": layers.split(","), "highlighted_pad_layers": overlays,
                                     "copper_fills": name != "routing-all.svg" and name != "placement.svg"}
                              for name, (_, layers, overlays) in views.items()}}
        (output / "rendering.json").write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    if sha256(pcb) != original:
        raise ValueError("Rendering changed the authoritative PCB")
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pcb", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--kicad-cli", required=True, type=Path)
    args = parser.parse_args()
    render(args.pcb.resolve(), args.output.resolve(), args.kicad_cli)


if __name__ == "__main__":
    main()
