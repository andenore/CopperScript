"""Content-bound routed reference geometry; never authoritative design intent.

Intent comes from the routing compiler, copper from a read-only native probe,
and connectivity evidence from the exact saved-board native DRC report. The
editor must recompile and match source/physical digests before granting credit.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal
from hashlib import sha256
import json
from pathlib import Path
import subprocess
import os
import tempfile

from ..drc import physical_board_digest
from ..physical import BoardSide, CopperLayer, Point, TrackSegment, Via

SCHEMA = "copperscript-editor-overlay/v0.1"
INTENT_SCHEMA = "copperscript-editor-route-intent/v0.1"
MAX_BYTES = 64 * 1024 * 1024


def _digest(data):
    return sha256(json.dumps(data, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def _json(path):
    raw = Path(path).read_bytes()
    if len(raw) > MAX_BYTES:
        raise ValueError("overlay input exceeds 64 MiB")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate overlay JSON field")
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite overlay JSON"))), raw


def projection_digest(board):
    # Exclude derived copper and execution metadata, not physical source rules.
    # Via-in-pad permissions are validated against the fabrication profile, so
    # boards that carry them keep it; all other digests are unchanged.
    metadata = ({"fabrication_profile": board.metadata["fabrication_profile"]}
                if board.via_in_pad_rules and "fabrication_profile" in board.metadata else {})
    return physical_board_digest(replace(board, tracks=(), vias=(), zone_fills=(),
                                          materialized_macros=(), metadata=metadata))


def electrical_digest(board):
    from ..serializer import board_to_dict
    from .transactions import electrical_identity
    return _digest(board_to_dict(electrical_identity(board)))


def pose_records(board):
    return [{"reference": p.reference, "position": [p.position.x_nm, p.position.y_nm],
             "rotation": str(p.rotation_degrees), "side": p.side.value}
            for p in sorted(board.placements, key=lambda p: p.reference)]


def _track(t):
    return {"net": t.net, "start": [t.start.x_nm, t.start.y_nm], "end": [t.end.x_nm, t.end.y_nm],
            "width_nm": t.width_nm, "layer": t.layer.value}


def _via(v):
    return {"net": v.net, "position": [v.position.x_nm, v.position.y_nm], "size_nm": v.size_nm,
            "drill_nm": v.drill_nm, "from_layer": v.from_layer.value, "to_layer": v.to_layer.value}


def write_intent(source, unrouted, routed, path, *, electrical=None, source_revision=None):
    """Called by the compiler, using source rules and final router poses."""
    posed = replace(unrouted, placements=routed.placements)
    from ..backends.kicad_pcb import kicad_reference
    native_references = {p.reference: kicad_reference(p.reference) for p in posed.placements}
    if len(set(native_references.values())) != len(native_references):
        raise ValueError("source references collide in the native PCB export")
    current_revision = sha256(Path(source).read_bytes()).hexdigest()
    if source_revision is not None and current_revision != source_revision:
        raise ValueError("source changed during routing; editor intent cannot be published")
    data = {"schema": INTENT_SCHEMA, "source_revision": current_revision,
            "electrical_digest": electrical_digest(electrical) if electrical is not None else None,
            "native_references": native_references,
            "physical_digest": projection_digest(posed), "poses": pose_records(posed),
            "tracks": [_track(t) for t in routed.tracks], "vias": [_via(v) for v in routed.vias]}
    Path(path).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _copper_key(item):
    item = dict(item)
    if "start" in item:
        item["start"], item["end"] = sorted((item["start"], item["end"]))
    elif "from_layer" in item:
        item["from_layer"], item["to_layer"] = sorted((item["from_layer"], item["to_layer"]))
    return json.dumps(item, sort_keys=True, separators=(",", ":"))


def export_overlay(run_path, kicad_python, output, *, runner=subprocess.run):
    """Export only fixed sibling artifacts of a user-chosen build manifest.

    No paths or executable commands are taken from the manifest. Failed routing
    runs can still show explicit copper, but missing/mismatched native evidence
    grants no plane connectivity. The supplied interpreter is an explicit host
    setting, never source-language or HTTP input.
    """
    run_path, output = Path(run_path).resolve(), Path(output)
    folder = run_path.parent
    if output.resolve().parent != folder or output.suffix != ".json" or output.name in {
            "run.json", "board.editor-intent.json", "kicad-drc.json", "route-report.json"}:
        raise ValueError("overlay must be a new JSON file beside run.json, not a build evidence file")
    pcb = folder / "board.kicad_pcb"
    intent, intent_raw = _json(folder / "board.editor-intent.json")
    run, run_raw = _json(run_path)
    if intent.get("schema") != INTENT_SCHEMA:
        raise ValueError("unsupported route intent")
    if run.get("editor_intent_sha256") != sha256(intent_raw).hexdigest():
        raise ValueError("routing manifest does not bind this editor intent; rerun the build")
    pcb_hash = sha256(pcb.read_bytes()).hexdigest()
    if run.get("filled_board_sha256", run.get("draft_board_sha256")) != pcb_hash:
        raise ValueError("PCB bytes differ from the routing manifest")
    probe = Path(__file__).with_name("native_overlay.py")
    result = runner([str(kicad_python), str(probe), str(pcb)], capture_output=True,
                    text=True, encoding="utf-8", timeout=60, check=False)
    if result.returncode:
        raise ValueError("native overlay probe failed: " + result.stderr.strip())
    if len(result.stdout.encode()) > MAX_BYTES:
        raise ValueError("native overlay exceeds 64 MiB")
    native = json.loads(result.stdout)
    poses = {p["reference"]: p for p in native["poses"]}
    for p in intent["poses"]:
        actual = poses.get(intent["native_references"][p["reference"]])
        # The backend exports mirrorX IR poses as mirrorY KiCad footprints.
        # Rear orientation therefore differs by 180 degrees, not by geometry.
        expected_rotation = Decimal(p["rotation"]) + (180 if p["side"] == "back" else 0)
        if (actual is None or actual["position"] != p["position"] or actual["side"] != p["side"]
                or min(abs(Decimal(actual["rotation"]) - expected_rotation) % 360,
                       360 - abs(Decimal(actual["rotation"]) - expected_rotation) % 360) > Decimal("0.000001")):
            raise ValueError("native PCB placement does not match the routed intent")
    for kind in ("tracks", "vias"):
        if sorted(map(_copper_key, native[kind])) != sorted(map(_copper_key, intent[kind])):
            raise ValueError("native PCB copper does not match the routed intent")
    evidence = {"verified": False, "reason": "No matching saved-board native DRC evidence", "remaining": []}
    report_path = folder / "kicad-drc.json"
    if report_path.is_file():
        report, raw = _json(report_path)
        if (run.get("filled_board_sha256") == pcb_hash
                and run.get("native_drc_sha256") == sha256(raw).hexdigest()):
            opens, violations = report.get("unconnected_items"), report.get("violations")
            if isinstance(opens, list) and isinstance(violations, list) and report.get("coordinate_units") == "mm":
                evidence = {"verified": True, "reason": "Native DRC bound to these exact saved PCB bytes",
                            "remaining": opens, "violations": violations,
                            "drc_sha256": sha256(raw).hexdigest()}
    data = {"schema": SCHEMA, "units": "nm", "source_revision": intent["source_revision"],
            "electrical_digest": intent["electrical_digest"],
            "physical_digest": intent["physical_digest"], "poses": intent["poses"],
            "pcb_sha256": pcb_hash, "native": native, "evidence": evidence}
    data["content_digest"] = _digest(data)
    # Validate limits/types before publishing a document the viewer will consume.
    RoutedOverlay.from_data(data)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(data, indent=2, sort_keys=True) + "\n")
    # Bind the derived capsule to the existing local build provenance as well.
    # Never accept arbitrary paths from that document or overwrite concurrent evidence.
    if run_path.read_bytes() != run_raw:
        raise ValueError("build manifest changed during overlay extraction; overlay not bound")
    run.setdefault("editor_overlays", {})[output.name] = sha256(output.read_bytes()).hexdigest()
    fd, temporary = tempfile.mkstemp(prefix=".editor-manifest-", dir=folder)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(json.dumps(run, indent=2) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        if run_path.read_bytes() != run_raw:
            raise ValueError("build manifest changed during overlay extraction; overlay not bound")
        os.replace(temporary, run_path)
    finally:
        if Path(temporary).exists():
            Path(temporary).unlink()
    return output


def _integer(value):
    if type(value) is not int or abs(value) > 10**12:
        raise ValueError("overlay dimensions require bounded integer nanometres")
    return value


def _point(value):
    if not isinstance(value, list) or len(value) != 2:
        raise ValueError("overlay point requires two coordinates")
    return Point(*map(_integer, value))


@dataclass(frozen=True)
class RoutedOverlay:
    data: dict

    @classmethod
    def load(cls, path):
        path = Path(path)
        data, raw = _json(path)
        result = cls.from_data(data)
        run, _ = _json(path.parent / "run.json")
        if run.get("editor_overlays", {}).get(path.name) != sha256(raw).hexdigest():
            raise ValueError("overlay is not bound to its sibling routing manifest")
        if sha256((path.parent / "board.kicad_pcb").read_bytes()).hexdigest() != data["pcb_sha256"]:
            raise ValueError("overlay's native PCB changed; regenerate the reference")
        if data["evidence"]["verified"]:
            report, report_raw = _json(path.parent / "kicad-drc.json")
            if (run.get("filled_board_sha256") != data["pcb_sha256"]
                    or run.get("native_drc_sha256") != sha256(report_raw).hexdigest()
                    or data["evidence"].get("drc_sha256") != sha256(report_raw).hexdigest()
                    or report.get("unconnected_items") != data["evidence"]["remaining"]):
                raise ValueError("overlay's native DRC evidence changed")
        return result

    @classmethod
    def from_data(cls, data):
        if not isinstance(data, dict) or data.get("schema") != SCHEMA or data.get("units") != "nm":
            raise ValueError("unsupported routed overlay")
        if data.get("content_digest") != _digest({k: v for k, v in data.items() if k != "content_digest"}):
            raise ValueError("overlay content digest does not match")
        try:
            native = data["native"]
            layers = {CopperLayer(l) for l in native["layers"]}
            if not layers or len(data["poses"]) > 5000:
                raise ValueError("overlay inventory exceeds editor limits")
            refs = set()
            for pose in data["poses"]:
                if not isinstance(pose["reference"], str) or pose["reference"] in refs:
                    raise ValueError("duplicate overlay reference")
                refs.add(pose["reference"])
                _point(pose["position"])
                if not Decimal(pose["rotation"]).is_finite():
                    raise ValueError("nonfinite overlay angle")
                BoardSide(pose["side"])
            for kind in ("tracks", "vias", "fills"):
                if not isinstance(native[kind], list) or len(native[kind]) > 250000:
                    raise ValueError("overlay copper exceeds editor limits")
            for t in native["tracks"]:
                TrackSegment(t["net"], _point(t["start"]), _point(t["end"]), _integer(t["width_nm"]), CopperLayer(t["layer"]))
                if CopperLayer(t["layer"]) not in layers:
                    raise ValueError("overlay track uses unavailable layer")
            for v in native["vias"]:
                Via(v["net"], _point(v["position"]), _integer(v["size_nm"]), _integer(v["drill_nm"]),
                    CopperLayer(v["from_layer"]), CopperLayer(v["to_layer"]))
                if not {CopperLayer(v["from_layer"]), CopperLayer(v["to_layer"])} <= layers:
                    raise ValueError("overlay via uses unavailable layer")
            total = 0
            for f in native["fills"]:
                if CopperLayer(f["layer"]) not in layers:
                    raise ValueError("overlay fill uses unavailable layer")
                for ring in [f["outer"], *f["holes"]]:
                    total += len(ring)
                    if len(ring) < 3 or total > 1000000:
                        raise ValueError("overlay fill vertex limit or degenerate ring")
                    for p in ring:
                        _point(p)
            for key in ("source_revision", "physical_digest", "pcb_sha256"):
                if len(data[key]) != 64 or any(c not in "0123456789abcdef" for c in data[key]):
                    raise ValueError("invalid overlay digest")
            if type(data["evidence"]["verified"]) is not bool or not isinstance(data["evidence"]["remaining"], list):
                raise ValueError("invalid native connectivity evidence")
        except (KeyError, TypeError, ArithmeticError) as exc:
            raise ValueError("malformed routed overlay") from exc
        return cls(data)

    def seed(self, board, source_revision, identity=None):
        if source_revision != self.data["source_revision"] or identity != self.data["electrical_digest"]:
            return board
        poses = {p["reference"]: p for p in self.data["poses"]}
        if set(poses) != {p.reference for p in board.placements}:
            return board
        candidate = replace(board, placements=tuple(replace(p, position=_point(poses[p.reference]["position"]),
                    rotation_degrees=Decimal(poses[p.reference]["rotation"]), side=BoardSide(poses[p.reference]["side"]))
                    for p in board.placements))
        return candidate if projection_digest(candidate) == self.data["physical_digest"] else board

    def fresh(self, board, source_revision, identity=None):
        return (source_revision == self.data["source_revision"] and identity == self.data["electrical_digest"]
                and projection_digest(board) == self.data["physical_digest"])

    def copper_board(self, board):
        # Native copper includes macro owner copper already. Do not materialize it twice.
        tracks = tuple(TrackSegment(t["net"], _point(t["start"]), _point(t["end"]), t["width_nm"], CopperLayer(t["layer"]))
                       for t in self.data["native"]["tracks"])
        # Preserve source via technology/finish validation outside the editor's
        # explicit-contact graph. Layer spans are what that graph consumes.
        vias = tuple(Via(v["net"], _point(v["position"]), v["size_nm"], v["drill_nm"],
                         CopperLayer(v["from_layer"]), CopperLayer(v["to_layer"])) for v in self.data["native"]["vias"])
        return replace(board, tracks=tracks, vias=vias, hard_macros=(), materialized_macros=(),
                       stackup=replace(board.stackup, via_technologies=()))

    def scene(self, board, source_revision, identity=None):
        fresh = self.fresh(board, source_revision, identity)
        return {**self.data, "stale": not fresh,
                "connectivity_credit": fresh and self.data["evidence"]["verified"] and not self.data["evidence"]["remaining"],
                "notice": ("Routed reference matches source and placement" if fresh else
                           "STALE routed reference: source, footprints, rules or placement changed; rebuild")}
