"""KiCad-backed evidence for actual filled-plane connectivity.

The zone declaration and provisional pad escapes are never treated as proof of
electrical continuity. This check refills a disposable exported board and
parses KiCad's independent connectivity/DRC report.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import asdict, dataclass, replace
from contextlib import nullcontext
from hashlib import sha256
import json
from pathlib import Path
import re
import tempfile

from .backends import KiCadPcbBackend
from .backends.kicad_project import kicad_export_digest, write_kicad_project
from .drc import physical_board_digest
from .decoupling_fill import DecouplingFillAssessment
from .manufacturing import CommandRunner, _subprocess_runner
from .physical import BoardSide, CopperLayer, PadKind, PhysicalBoard, RouteKind, TrackSegment


@dataclass(frozen=True, slots=True)
class NativeCopperItem:
    """Exact export UUID binding; unknown objects never become repair authority."""
    uuid: str
    kind: str = "unknown"
    net: str | None = None
    component: str | None = None
    pad: str | None = None
    layers: tuple[str, ...] = ()
    positions_nm: tuple[tuple[int, int], ...] = ()
    region: str | None = None
    index: int | None = None


@dataclass(frozen=True, slots=True)
class NativeFinding:
    category: str
    type: str
    description: str
    items: tuple[NativeCopperItem, ...]


@dataclass(frozen=True, slots=True)
class NativeFilledRegion:
    zone_uuid: str
    fragment_digest: str
    net: str
    layer: str
    vertices_nm: tuple[tuple[int, int], ...]


def _filled_regions(pcb_text: str, objects: dict[str, NativeCopperItem]):
    from .importers.kicad_mod import _parse_sexpr, _children, _first, _atom
    from .physical import nm_from_mm
    root = _parse_sexpr(pcb_text, "native filled board")
    regions = []
    for zone in _children(root, "zone"):
        uuid = _atom(_first(zone, "uuid"), 1)
        binding = objects.get(uuid)
        for polygon in _children(zone, "filled_polygon"):
            if binding is None or binding.kind != "zone" or binding.net is None:
                raise RuntimeError("native filled polygon has no exact source-zone UUID binding")
            layer = _atom(_first(polygon, "layer"), 1)
            if layer not in binding.layers:
                raise RuntimeError("native filled polygon has an unexpected layer")
            points = _first(polygon, "pts")
            if points is None:
                raise RuntimeError("native filled polygon lacks vertices")
            vertices = tuple((nm_from_mm(_atom(point, 1)), nm_from_mm(_atom(point, 2)))
                             for point in _children(points, "xy"))
            if len(vertices) < 3:
                raise RuntimeError("native filled polygon has insufficient vertices")
            regions.append(NativeFilledRegion(uuid, sha256(repr((uuid, layer, vertices)).encode()).hexdigest(),
                binding.net, layer, vertices))
    return tuple(regions)


def native_object_map(board: PhysicalBoard) -> dict[str, NativeCopperItem]:
    from .backends.kicad_pcb import _stable_uuid
    from .placement import transformed_local_point
    assigned = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
    objects = {}
    def add(kind, parts, **kwargs):
        uuid = _stable_uuid(board.name, kind, *parts)
        objects[uuid] = NativeCopperItem(uuid, kind, **kwargs)
    for pose in board.placements:
        for index, pad in enumerate(board.footprints[pose.footprint].pads):
            position = transformed_local_point(pose, pad.position)
            layers = ((CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK,)
                      if pad.kind is PadKind.SMD else board.stackup.copper_layers)
            add("pad", (pose.reference, str(index), pad.number),
                net=assigned.get((pose.reference, pad.number)), component=pose.reference, pad=pad.number,
                layers=tuple(layer.value for layer in layers),
                positions_nm=((position.x_nm, position.y_nm),), index=index)
    for index, track in enumerate(board.tracks):
        add("segment", (str(index),), net=track.net, layers=(track.layer.value,), index=index,
            positions_nm=((track.start.x_nm, track.start.y_nm), (track.end.x_nm, track.end.y_nm)))
    for index, via in enumerate(board.vias):
        first, last = sorted((board.stackup.copper_layers.index(via.from_layer),
                              board.stackup.copper_layers.index(via.to_layer)))
        add("via", (str(index),), net=via.net, index=index,
            layers=tuple(layer.value for layer in board.stackup.copper_layers[first:last+1]),
            positions_nm=((via.position.x_nm, via.position.y_nm),))
    for zone in board.zones:
        for layer in zone.layers:
            add("zone", (zone.id, layer.value), net=zone.net, layers=(layer.value,), region=zone.id,
                positions_nm=tuple((point.x_nm, point.y_nm) for point in zone.outline.outer.vertices))
    return objects


@dataclass(frozen=True, slots=True)
class PlaneVerification:
    passed: bool
    board_digest: str
    export_digest: str
    filled_board_digest: str
    report_digest: str
    kicad_version: str
    unconnected_count: int
    island_count: int
    other_violation_count: int
    findings: tuple[str, ...]
    dangling_track_uuids: tuple[str, ...] = ()
    native_findings: tuple[NativeFinding, ...] = ()
    artifact_directory: str | None = None
    filled_regions: tuple[NativeFilledRegion, ...] = ()
    decoupling_assessments: tuple[DecouplingFillAssessment, ...] = ()

    def matches(self, board: PhysicalBoard) -> bool:
        """Reject evidence reused after any source-board geometry change."""
        if self.board_digest != physical_board_digest(board):
            return False
        return self.export_digest == kicad_export_digest(KiCadPcbBackend().generate(board))

    def zone_connectivity_verified(self, board: PhysicalBoard) -> bool:
        """Separate connected fill from overall signoff, bound to exact export.

        Library-copy/lookup findings remain signoff failures, but do not erase
        the independent zero-open observation. Any other violation, island,
        malformed count, or stale source/export prevents reconciliation.
        """
        library_types = {"lib_footprint_issues", "lib_footprint_mismatch"}
        return (
            self.unconnected_count == self.island_count == 0
            and self.other_violation_count == len(self.findings)
            and all(item.split(":", 1)[0] in library_types for item in self.findings)
            and self.matches(board)
        )

    def to_json(self) -> str:
        return json.dumps({
            "schema": "copperscript-plane-verification/v0.1",
            "passed": self.passed,
            "board_digest": self.board_digest,
            "export_digest": self.export_digest,
            "filled_board_digest": self.filled_board_digest,
            "report_digest": self.report_digest,
            "kicad_version": self.kicad_version,
            "unconnected_count": self.unconnected_count,
            "island_count": self.island_count,
            "other_violation_count": self.other_violation_count,
            "findings": list(self.findings),
            "dangling_track_uuids": list(self.dangling_track_uuids),
            "native_findings": [asdict(item) for item in self.native_findings],
            "artifact_directory": self.artifact_directory,
            "filled_regions": [asdict(item) for item in self.filled_regions],
            "decoupling_assessments": [asdict(item) for item in self.decoupling_assessments],
        }, indent=2, sort_keys=True) + "\n"


def verify_filled_planes(
    board: PhysicalBoard,
    *,
    kicad_cli: Path,
    runner: CommandRunner | None = None,
    qualified_major: int = 10,
    artifact_directory: Path | None = None,
) -> PlaneVerification:
    """Refill a private KiCad board and require zero DRC/connectivity findings."""

    runner = runner or _subprocess_runner
    if artifact_directory is not None:
        artifact_directory = artifact_directory.resolve()
        # Never overwrite an existing candidate or production evidence directory.
        artifact_directory.mkdir(parents=True, exist_ok=False)
    context = (tempfile.TemporaryDirectory(prefix="copperscript-plane-")
               if artifact_directory is None else nullcontext(str(artifact_directory)))
    with context as directory:
        stage = Path(directory).resolve()
        version_result = runner((str(kicad_cli), "--version"), stage)
        if version_result.returncode:
            raise RuntimeError(f"KiCad version query failed: {version_result.stderr}")
        lines = version_result.stdout.strip().splitlines()
        version = lines[0] if lines else ""
        match = re.match(r"(\d+)", version)
        if match is None or int(match.group(1)) != qualified_major:
            raise RuntimeError(
                f"KiCad {version!r} is not the qualified major {qualified_major}"
            )
        manifest = KiCadPcbBackend().generate(board)
        if len(manifest.artifacts) < 2:
            raise RuntimeError("KiCad backend omitted the same-stem project")
        pcb_artifact = manifest.artifacts[0]
        pcb_path = stage / pcb_artifact.name
        # Preserve the backend's exact UTF-8 bytes on Windows as well as Unix.
        write_kicad_project(manifest, pcb_path)
        if artifact_directory is not None:
            original = stage / "input"
            original.mkdir()
            write_kicad_project(manifest, original / pcb_artifact.name)
        export_digest = kicad_export_digest(manifest)
        report_path = stage / "drc.json"
        command = [str(kicad_cli), "pcb", "drc", "--format", "json",
                   "--severity-all", "--output", str(report_path)]
        if board.zones:
            command += ["--refill-zones", "--save-board"]
        command.append(str(pcb_path))
        result = runner(tuple(command), stage)
        if result.returncode or not report_path.is_file():
            raise RuntimeError(
                "KiCad refill/DRC failed: "
                + (result.stderr.strip() or result.stdout.strip() or "no report")
            )
        report_bytes = report_path.read_bytes()
        try:
            report = json.loads(report_bytes)
        except (ValueError, UnicodeDecodeError) as exc:
            raise RuntimeError("KiCad produced an invalid JSON DRC report") from exc
        if not isinstance(report, dict):
            raise RuntimeError("KiCad produced a non-object DRC report")
        violations = report.get("violations")
        unconnected = report.get("unconnected_items")
        if not isinstance(violations, list) or not isinstance(unconnected, list):
            raise RuntimeError("KiCad DRC report lacks violations/unconnected_items")
        islands = [item for item in violations if _violation_type(item) == "isolated_copper"]
        others = [item for item in violations if _violation_type(item) != "isolated_copper"]
        findings = tuple(
            f"{_violation_type(item)}: {item.get('description', '')}"
            for item in (*unconnected, *islands, *others)
        )
        dangling = tuple(sorted({entry.get("uuid") for item in others
            if _violation_type(item) == "track_dangling"
            for entry in item.get("items", ()) if isinstance(entry, dict)
            and isinstance(entry.get("uuid"), str)}))
        objects = native_object_map(board)
        mapped = []
        for category, entries in (("unconnected", unconnected), ("violation", violations)):
            for item in entries:
                raw_items = item.get("items", [])
                if not isinstance(raw_items, list):
                    raise RuntimeError("malformed KiCad DRC objects")
                bindings = []
                for entry in raw_items:
                    if not isinstance(entry, dict) or not isinstance(entry.get("uuid"), str):
                        raise RuntimeError("malformed KiCad DRC object UUID")
                    uuid = entry["uuid"]
                    bindings.append(objects.get(uuid, NativeCopperItem(uuid)))
                mapped.append(NativeFinding(category, _violation_type(item),
                    str(item.get("description", "")), tuple(bindings)))
        try:
            filled_regions = _filled_regions(pcb_path.read_text(encoding="utf-8"), objects) if board.zones else ()
        except (ValueError, TypeError, IndexError, ArithmeticError) as exc:
            raise RuntimeError("unsupported or malformed native filled polygons") from exc
        evidence = PlaneVerification(
            not findings,
            physical_board_digest(board), export_digest,
            sha256(pcb_path.read_bytes()).hexdigest(),
            sha256(report_bytes).hexdigest(), version,
            len(unconnected), len(islands), len(others), findings, dangling,
            tuple(mapped), str(stage.resolve()) if artifact_directory is not None else None,
            filled_regions,
        )
        from .decoupling_fill import assess_decoupling_fill
        return replace(evidence, decoupling_assessments=assess_decoupling_fill(board, evidence))


def remove_native_dangling_tracks(
    board: PhysicalBoard, evidence: PlaneVerification, protected: Counter[TrackSegment],
    *, kicad_cli: Path,
    artifact_directory: Path | None = None,
) -> tuple[PhysicalBoard, PlaneVerification]:
    """Drop only KiCad-identified ordinary dead ends when fresh refill proves safe."""
    if (not evidence.matches(board) or evidence.unconnected_count or evidence.island_count
            or not evidence.dangling_track_uuids
            or len(evidence.findings) != len(evidence.dangling_track_uuids)
            or any(not item.startswith("track_dangling:") for item in evidence.findings)):
        return board, evidence
    from .native_stub_cleanup import native_stub_candidate
    candidate = native_stub_candidate(board, evidence, protected)
    if candidate is None:
        return board, evidence
    checked = verify_filled_planes(candidate, kicad_cli=kicad_cli, artifact_directory=artifact_directory)
    return (candidate, checked) if checked.passed and checked.matches(candidate) else (board, evidence)


def remove_native_dangling_copper(
    board: PhysicalBoard, evidence: PlaneVerification,
    protected_tracks: Counter, protected_vias: Counter, *,
    eligible_nets: frozenset[str], kicad_cli: Path,
    artifact_directory: Path | None = None, verifier=None,
) -> tuple[PhysicalBoard, PlaneVerification]:
    """Clean exact disposable ordinary native ends inside a repair transaction.

    Unrelated filled-zone opens may remain, but affected ordinary nets must
    already have actual explicit connectivity. Every removal needs fresh native
    proof: no remaining violations/islands, no new open net or increased opens.
    Macro, array and caller-supplied ownership is occurrence-counted.
    """
    from .drc import explicit_copper_connectivity
    from .hard_macros import resolved_macro_geometry
    from .pad_via_arrays import via_in_pad_array_vias
    from .zone_geometry import distribution_zone_nets
    if not evidence.matches(board) or evidence.island_count:
        return board, evidence
    findings = [item for item in evidence.native_findings if item.category == "violation"]
    if (not findings or len(findings) > 64 or len(findings) != evidence.other_violation_count
            or any(item.type not in {"track_dangling", "via_dangling"} for item in findings)):
        return board, evidence
    blocked = distribution_zone_nets(board) | {
        rule.net for rule in board.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    tracks, vias = protected_tracks.copy(), protected_vias.copy()
    vias |= Counter(via_in_pad_array_vias(board))
    macro_tracks, macro_vias = Counter(), Counter()
    for macro in board.hard_macros:
        if macro.cluster in board.materialized_macros:
            geometry = resolved_macro_geometry(board, macro)
            owner_tracks, owner_vias = geometry[:2]
            macro_tracks.update(owner_tracks)
            macro_vias.update(owner_vias)
    tracks |= macro_tracks
    vias |= macro_vias
    objects = native_object_map(board)
    indices = {"segment": set(), "via": set()}
    affected = set()
    for finding in findings:
        kind = "segment" if finding.type == "track_dangling" else "via"
        if len(finding.items) != 1:
            return board, evidence
        item = finding.items[0]
        if (objects.get(item.uuid) != item or item.kind != kind or item.index is None
                or item.net not in eligible_nets or item.net in blocked):
            return board, evidence
        indices[kind].add(item.index)
        affected.add(item.net)
    removed_tracks = Counter(board.tracks[index] for index in indices["segment"])
    removed_vias = Counter(board.vias[index] for index in indices["via"])
    if (removed_tracks - (Counter(board.tracks) - tracks)
            or removed_vias - (Counter(board.vias) - vias)):
        return board, evidence
    candidate = replace(board,
        tracks=tuple(item for index, item in enumerate(board.tracks) if index not in indices["segment"]),
        vias=tuple(item for index, item in enumerate(board.vias) if index not in indices["via"]))
    nets = tuple(net for net in board.nets if net.name in affected)
    for current in (board, candidate):
        graph = explicit_copper_connectivity(current, only_nets=frozenset(affected))
        if not all(graph.net_connected(net) for net in nets):
            return board, evidence
    def opens(checked):
        entries = [item for item in checked.native_findings if item.category == "unconnected"]
        if len(entries) != checked.unconnected_count or any(
                item.type != "unconnected_items" or not item.items or any(
                    obj.kind == "unknown" or obj.net is None for obj in item.items) for item in entries):
            return None
        return {obj.net for item in entries for obj in item.items}
    old_opens = opens(evidence)
    if old_opens is None:
        return board, evidence
    checked = (verifier(candidate) if verifier else verify_filled_planes(candidate,
        kicad_cli=kicad_cli, artifact_directory=artifact_directory))
    new_opens = opens(checked)
    if (checked.matches(candidate) and checked.island_count == checked.other_violation_count == 0
            and new_opens is not None and new_opens <= old_opens
            and checked.unconnected_count <= evidence.unconnected_count):
        return candidate, checked
    return board, evidence


def _violation_type(item: object) -> str:
    if not isinstance(item, dict):
        raise RuntimeError("malformed KiCad DRC finding")
    return str(item.get("type", "unknown"))
