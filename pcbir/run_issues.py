"""Located, grouped issues for a routing/layout run.

``copper summarize`` says *what* failed. This module adds *where*: every issue
gets a stable id (``N1``, ``P1`` ...), its nets, layers, objects and a window in
millimetres that ``copper view RUN --issue ID`` draws. Repeated findings with
one cause are grouped (e.g. 69 nets "blocked: package-access preflight
incomplete" become one issue) so an agent reads one line per cause.

Positions come from the report itself (native findings) or from the run's
board geometry (pads of pending exits and open nets). Grouping is a reading
aid; the full report and its gates are unchanged and remain authoritative.
"""

from __future__ import annotations

import json
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from .scene import Scene, scene_from_kicad_pcb

ISSUES_SCHEMA = "copperscript.run-issues/v1"
_NM = 1_000_000


@dataclass
class RunIssue:
    id: str
    stage: str
    code: str
    title: str
    count: int = 1
    nets: list[str] = field(default_factory=list)
    layers: list[str] = field(default_factory=list)
    objects: list[str] = field(default_factory=list)
    points: list[tuple[float, float]] = field(default_factory=list)
    caused_by: str | None = None

    def window(self, margin: float = 1.5, minimum: float = 4.0) -> list[float] | None:
        if not self.points:
            return None
        xs = [p[0] for p in self.points]
        ys = [p[1] for p in self.points]
        x0, x1, y0, y1 = min(xs) - margin, max(xs) + margin, min(ys) - margin, max(ys) + margin
        if x1 - x0 < minimum:
            c = (x0 + x1) / 2
            x0, x1 = c - minimum / 2, c + minimum / 2
        if y1 - y0 < minimum:
            c = (y0 + y1) / 2
            y0, y1 = c - minimum / 2, c + minimum / 2
        return [round(v, 3) for v in (x0, y0, x1, y1)]

    def document(self, *, limit: int = 8) -> dict[str, object]:
        def capped(values):
            return values[:limit] + ([f"+{len(values) - limit} more"] if len(values) > limit else [])
        result: dict[str, object] = {
            "id": self.id, "stage": self.stage, "code": self.code, "title": self.title, "count": self.count,
            "nets": capped(sorted(self.nets)), "layers": sorted(self.layers), "objects": capped(self.objects),
            "window_mm": self.window(), "markers": len(self.points),
        }
        if self.caused_by:
            result["caused_by"] = self.caused_by
        return result


@dataclass
class Run:
    """A route-board/plan-layout output directory, or a bare report/board pair."""

    directory: Path
    report_path: Path | None
    board_path: Path | None
    filled_board_path: Path | None
    _report: dict | None = None
    _scene: Scene | None = None
    _filled: Scene | None = None

    @property
    def report(self) -> dict:
        if self._report is None:
            if self.report_path is None:
                raise FileNotFoundError(f"{self.directory}: no route-report.json or layout report")
            self._report = json.loads(self.report_path.read_text(encoding="utf-8"))
        return self._report

    def scene(self, *, filled: bool = False) -> Scene:
        if filled and self.filled_board_path is not None:
            if self._filled is None:
                self._filled = scene_from_kicad_pcb(self.filled_board_path)
            return self._filled
        if self.board_path is None:
            raise FileNotFoundError(f"{self.directory}: no .kicad_pcb board in the run")
        if self._scene is None:
            self._scene = scene_from_kicad_pcb(self.board_path)
        return self._scene


def load_run(path: str | Path) -> Run:
    """Find the report, exported board and native refilled board of a run."""

    path = Path(path)
    if path.is_file() and path.suffix == ".json":
        directory, report = path.parent, path
    elif path.is_file() and path.suffix == ".kicad_pcb":
        return Run(path.parent, None, path, None)
    else:
        directory = path
        report = next((p for p in (path / "route-report.json", path / "layout-report.json") if p.is_file()), None)
        if report is None:
            candidates = sorted(p for p in path.glob("*.json") if not p.name.endswith(".summary.json"))
            report = next((p for p in candidates if _schema(p).startswith("copperscript-")), None)
    if report is not None and not report.name.endswith(".error.json"):
        # A later run into the same place that failed leaves only an error record.
        error = report.with_name(report.stem + ".error.json")
        if error.is_file() and error.stat().st_mtime >= report.stat().st_mtime:
            report = error
    board = directory / "board.kicad_pcb"
    if not board.is_file():
        board = next(iter(sorted(directory.glob("*.kicad_pcb"))), None)
    filled = _reported_filled_board(report) if report is not None else None
    return Run(directory, report, board, filled)


def _reported_filled_board(report: Path) -> Path | None:
    """Use the report's exact verification, never the last attempted candidate."""

    try:
        document = json.loads(report.read_text(encoding="utf-8"))
        native = document.get("plane_verification") if isinstance(document, dict) else None
        if not isinstance(native, dict):
            return None
        location, digest = native.get("artifact_directory"), native.get("filled_board_digest")
        if not isinstance(location, str) or not location or not isinstance(digest, str) or not digest:
            return None
        directory = Path(location)
        if not directory.is_absolute():
            directory = report.parent / directory
        # Includes custom evidence directories and the final native-cleanup result.
        # Missing/stale evidence is unavailable; a sibling trial is not a substitute.
        return next((path for path in sorted(directory.glob("*.kicad_pcb"))
                     if _sha256(path) == digest), None)
    except (OSError, ValueError):
        return None


def _schema(path: Path) -> str:
    try:
        with path.open(encoding="utf-8") as handle:
            head = handle.read(4096)
        start = head.find('"schema"')
        return head[start:start + 80].split('"')[3] if start >= 0 else ""
    except (OSError, IndexError):
        return ""


# --- extraction -----------------------------------------------------------------------

def _pad_points(scene: Scene | None, ref_pad: str) -> list[tuple[float, float]]:
    if scene is None or "." not in ref_pad:
        return []
    ref, _, pad = ref_pad.rpartition(".")
    return [item.center for item in scene.pad(ref, pad)]


def _net_points(scene: Scene | None, nets) -> list[tuple[float, float]]:
    if scene is None:
        return []
    wanted = set(nets)
    return [pad.center for pad in scene.pads if pad.net in wanted]


def _clusters(items: list, points_of, threshold: float) -> list[list]:
    """Single-linkage spatial clusters; items without positions form one cluster."""

    located = [(item, points_of(item)) for item in items]
    blind = [item for item, points in located if not points]
    located = [(item, points) for item, points in located if points]
    parent = list(range(len(located)))

    def root(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    limit = threshold * threshold
    for i in range(len(located)):
        for j in range(i + 1, len(located)):
            if root(i) == root(j):
                continue
            if any((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 <= limit
                   for a in located[i][1] for b in located[j][1]):
                parent[root(i)] = root(j)
    groups: dict[int, list] = defaultdict(list)
    for i, (item, _points) in enumerate(located):
        groups[root(i)].append(item)
    result = sorted(groups.values(), key=lambda group: -len(group))
    return result + ([blind] if blind else [])


def _section(report: dict, key: str) -> dict:
    value = report.get(key)
    return value if isinstance(value, dict) else {}


def _items(value) -> list:
    return value if isinstance(value, list) else []


def run_issues(run: Run, *, cluster_mm: float = 3.0) -> list[RunIssue]:
    report = run.report
    if not isinstance(report, dict):
        raise ValueError("report is not a JSON object")
    try:
        scene = run.scene()
    except (FileNotFoundError, OSError, ValueError):
        scene = None
    issues: list[RunIssue] = []
    if str(report.get("schema", "")).startswith("copperscript-layout-report"):
        return _layout_issues(report, scene)
    if str(report.get("schema", "")).startswith("copperscript-route-error"):
        stage = ", ".join(report.get("open_stages") or []) or "an unknown stage (run with --progress)"
        components = [str(c) for c in _items(report.get("components"))]
        points = [p.center for ref in components for p in (scene.pads if scene else []) if p.ref == ref]
        return [RunIssue("E1", "error", "RUN-ERROR",
                         f"{report.get('error_type', 'Error')} in {stage}: {str(report.get('message', ''))[:300]}",
                         objects=components, points=points)]

    access = _section(report, "package_access")
    pending = [str(p) for p in _items(access.get("pending_pads"))]
    access_id = None
    pending_nets: set[str] = set()
    if pending:
        pads_of = lambda ref_pad: _pad_points(scene, str(ref_pad))
        for index, pads in enumerate(_clusters([str(p) for p in pending], pads_of, cluster_mm), 1):
            components = sorted({pad.rpartition(".")[0] for pad in pads})
            title = (f"{len(pads)} pad(s) without a verified package exit: "
                     + ", ".join(components[:4]) + (f" +{len(components) - 4}" if len(components) > 4 else ""))
            issue = RunIssue(f"P{index}", "access", "PENDING-PACKAGE-EXIT", title, len(pads), objects=pads,
                             points=[p for pad in pads for p in pads_of(pad)])
            issue.nets = sorted({pad.net for ref_pad in pads for pad in _pads(scene, ref_pad) if pad.net})
            pending_nets.update(issue.nets)
            issues.append(issue)
        count = len(issues)
        access_id = "P1" if count == 1 else f"P1..P{count}"
    failed_critical = [str(n) for n in _items(access.get("failed_critical_nets"))]
    if failed_critical:
        issues.append(RunIssue("C1", "critical", "CRITICAL-BLOCKED",
                               f"{len(failed_critical)} critical net(s) blocked at package access",
                               len(failed_critical), nets=failed_critical,
                               points=_net_points(scene, failed_critical), caused_by=access_id))

    native = _section(report, "plane_verification")
    groups: dict[tuple, list[dict]] = defaultdict(list)
    for finding in _items(native.get("native_findings")):
        if not isinstance(finding, dict):
            continue
        kind = finding.get("type") or finding.get("category") or "native"
        # Gaps are grouped per net and place; other findings (dangling copper,
        # clearance) per type, since their cause is usually one stage.
        nets = (tuple(sorted({item.get("net") for item in finding.get("items", []) if item.get("net")}))
                if kind == "unconnected_items" else ())
        groups[(kind, nets)].append(finding)
    def finding_points(finding):
        # An unconnected pair is located at its gap: the closest points of the two items.
        per_item = [[(x / _NM, y / _NM) for x, y in item.get("positions_nm") or ()]
                    for item in finding.get("items", [])]
        per_item = [points for points in per_item if points]
        if finding.get("type") == "unconnected_items" and len(per_item) == 2:
            return list(min(((a, b) for a in per_item[0] for b in per_item[1]),
                            key=lambda pair: (pair[0][0] - pair[1][0]) ** 2 + (pair[0][1] - pair[1][1]) ** 2))
        return [point for points in per_item for point in points]

    clustered = [(key, cluster) for key, findings in groups.items()
                 for cluster in (_clusters(findings, finding_points, cluster_mm)
                                 if key[0] == "unconnected_items" else [findings])]
    ordered = sorted(clustered, key=lambda kv: (kv[0][0] != "unconnected_items", -len(kv[1]), kv[0]))
    for index, ((kind, nets), findings) in enumerate(ordered, 1):
        points, layers, objects = [], set(), []
        for finding in findings:
            points.extend(finding_points(finding))
            for item in finding.get("items", []):
                layers.update(item.get("layers") or ())
                label = (f"{item['component']}.{item['pad']}" if item.get("component") and item.get("pad")
                         else f"{item.get('kind', 'item')}@{_fmt(item.get('positions_nm'))}")
                if label not in objects:
                    objects.append(label)
        description = findings[0].get("description") or kind
        all_nets = list(nets) or sorted({item.get("net") for f in findings for item in f.get("items", [])
                                         if item.get("net")})
        issues.append(RunIssue(f"N{index}", "native", kind.upper(),
                               f"{len(findings)} x {description}" + (f" on {', '.join(nets)}" if nets else ""),
                               len(findings), nets=all_nets, layers=sorted(layers), objects=objects,
                               points=points))

    drc = [f for f in _items(_section(report, "drc").get("findings")) if isinstance(f, dict)]
    open_nets = sorted({net for f in drc if f.get("code") == "DRC-OPEN-NET" and f.get("severity") == "error"
                        for net in f.get("nets") or ()})
    if open_nets:
        # One line for all nets open by exact copper; `copper view RUN --net NET` shows each.
        # Linked to pending exits only when some open net has a pending pad.
        issues.append(RunIssue("O1", "physical", "DRC-OPEN-NET",
                               f"{len(open_nets)} net(s) not connected by exact copper", len(open_nets),
                               nets=open_nets,
                               caused_by=access_id if pending_nets & set(open_nets) else None))
    other = [f for f in drc if f.get("severity") == "error" and f.get("code") not in {"DRC-OPEN-NET",
                                                                                         "DRC-ROUTE-INCOMPLETE"}]
    for index, finding in enumerate(other, 1):
        objects = [o.split(":", 1)[1] for o in finding.get("objects", []) if o.startswith("pad:")]
        issues.append(RunIssue(f"D{index}", "physical", finding.get("code", "DRC"), finding.get("message", ""),
                               nets=list(finding.get("nets") or []), layers=list(finding.get("layers") or []),
                               objects=objects, points=[p for o in objects for p in _pad_points(scene, o)]))

    detailed = [n for n in _items(_section(report, "detailed").get("nets")) if isinstance(n, dict)]
    reasons: dict[str, list[str]] = defaultdict(list)
    for net in detailed:
        if net.get("connected") is False:
            reason = "; ".join(net.get("diagnostics") or []) or "not connected"
            reasons[reason].append(net.get("net", "?"))
    for index, (reason, nets) in enumerate(sorted(reasons.items(), key=lambda kv: -len(kv[1])), 1):
        caused = access_id if access_id and "package-access" in reason else None
        issues.append(RunIssue(f"R{index}", "routing", "NET-UNROUTED", f"{len(nets)} net(s): {reason}",
                               len(nets), nets=nets, points=_net_points(scene, nets) if len(nets) <= 6 else [],
                               caused_by=caused))
    if not native.get("native_findings"):
        # Without a native fill check, the raster plane estimate is the only plane evidence.
        estimated = _section(_section(report, "summary"), "plane").get("estimated_open_pads")
        for index, (net, pads) in enumerate(sorted((estimated or {}).items()), 1):
            pads = [str(p) for p in _items(pads)]
            issues.append(RunIssue(f"E{index}", "plane", "PLANE-ESTIMATED-OPEN",
                                   f"{len(pads)} {net} pad(s) cut off from the rest of the net "
                                   f"(estimate, zones unfilled): " + ", ".join(pads[:6])
                                   + (f" +{len(pads) - 6}" if len(pads) > 6 else ""),
                                   len(pads), nets=[net], objects=pads,
                                   points=[p for pad in pads for p in _pad_points(scene, pad)]))
    issues.extend(_placement_issues(report, scene))
    return issues


def _placement_issues(report: dict, scene: Scene | None) -> list[RunIssue]:
    """Placement intent warnings (fixed parts far from their partners, parts in pair corridors).

    Not failures: they point at authored poses that likely caused routing issues above.
    """
    issues = []
    for finding in _items(report.get("placement_findings")):
        if not isinstance(finding, dict) or finding.get("status") in {"pass", "passed", "ok"}:
            continue
        subject = str(finding.get("subject") or "")
        points = ([p.center for p in scene.pads if p.ref == subject]
                  if scene is not None and subject else [])
        issues.append(RunIssue(f"W{len(issues) + 1}", "placement", str(finding.get("code", "PLACEMENT")),
                               f"{subject + ': ' if subject else ''}{finding.get('message', '')}",
                               objects=[subject] if subject else [], points=points))
    return issues


def _layout_issues(report: dict, scene: Scene | None) -> list[RunIssue]:
    """plan-layout reports: failed gates and non-passing findings, located by subject."""

    issues: list[RunIssue] = []
    for gate in _items(report.get("gates")):
        if isinstance(gate, dict) and gate.get("status") in {"fail", "failed", "blocked", "not_run"}:
            issues.append(RunIssue(f"L{len(issues) + 1}", "layout", "LAYOUT-" + str(gate["status"]).upper(),
                                   f"{gate.get('stage')}: {gate.get('summary', '')}"))
    index = 0
    for finding in _items(report.get("findings")):
        if not isinstance(finding, dict) or finding.get("status") in {"pass", "passed", "ok"}:
            continue
        index += 1
        subject = str(finding.get("subject") or "")
        points = []
        if scene is not None and subject:
            points = [p.center for p in scene.pads if p.ref == subject] or \
                [p.center for p in scene.pads if p.net == subject] or _pad_points(scene, subject)
        issues.append(RunIssue(f"F{index}", "layout", str(finding.get("code", "LAYOUT")),
                               f"{subject + ': ' if subject else ''}{finding.get('message', '')}",
                               objects=[subject] if subject else [], points=points))
    return issues


def _pads(scene: Scene | None, ref_pad: str):
    if scene is None:
        return []
    ref, _, pad = ref_pad.rpartition(".")
    return scene.pad(ref, pad)


def _fmt(positions) -> str:
    if not positions:
        return "?"
    x, y = positions[0]
    return f"({x / _NM:.3f},{y / _NM:.3f})"


def filter_issues(issues: list[RunIssue], *, net: str | None = None, stage: str | None = None,
                  code: str | None = None) -> list[RunIssue]:
    result = issues
    if net:
        result = [i for i in result if net in i.nets]
    if stage:
        result = [i for i in result if i.stage == stage]
    if code:
        result = [i for i in result if i.code.lower() == code.lower()]
    return result


def _balanced(issues: list[RunIssue], limit: int) -> list[RunIssue]:
    """Up to ``limit`` issues, taking from every stage in turn so one stage cannot hide another."""

    stages: dict[str, list[RunIssue]] = {}
    for issue in issues:
        stages.setdefault(issue.stage, []).append(issue)
    chosen: set[int] = set()
    queues = [list(items) for items in stages.values()]
    while len(chosen) < min(limit, len(issues)):
        for queue in queues:
            if queue and len(chosen) < limit:
                chosen.add(id(queue.pop(0)))
    return [issue for issue in issues if id(issue) in chosen]


def issues_document(run: Run, issues: list[RunIssue], *, limit: int | None = None) -> dict[str, object]:
    shown = issues if limit is None or limit >= len(issues) else _balanced(issues, limit)
    report = run.report
    counts: dict[str, int] = {}
    for issue in issues:
        counts[issue.code] = counts.get(issue.code, 0) + issue.count
    status = report.get("status")
    if status is None and str(report.get("schema", "")).startswith("copperscript-layout-report"):
        failed = any(isinstance(g, dict) and g.get("status") in {"fail", "failed", "blocked"}
                     for g in _items(report.get("gates")))
        status = "layout-fail" if failed else "layout"
    return {"schema": ISSUES_SCHEMA, "run": str(run.directory),
            "report": str(run.report_path) if run.report_path else None,
            "board": str(run.board_path) if run.board_path else None,
            "filled_board": str(run.filled_board_path) if run.filled_board_path else None,
            # Lets a viewer (CopperReview) check the issues belong to the board it shows.
            "board_sha256": _sha256(run.board_path), "filled_board_sha256": _sha256(run.filled_board_path),
            "status": status, "routing_complete": report.get("routing_complete"),
            "issue_count": len(issues), "omitted": len(issues) - len(shown), "finding_counts": counts,
            "stopped_after": report.get("stopped_after"),
            "issues": [issue.document() for issue in shown]}


def _sha256(path: Path | None) -> str | None:
    if path is None or not path.is_file():
        return None
    import hashlib
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def render_issues(document: dict[str, object]) -> str:
    stopped = f"  stopped after {document['stopped_after']}" if document.get("stopped_after") else ""
    lines = [f"{document['status']}{stopped}  routing_complete={document['routing_complete']}  "
             f"{document['issue_count']} grouped issue(s)",
             "findings: " + ", ".join(f"{code} {count}" for code, count in document["finding_counts"].items())]
    for issue in document["issues"]:
        window = issue["window_mm"]
        where = (f"[{window[0]:.2f},{window[1]:.2f} .. {window[2]:.2f},{window[3]:.2f}]" if window else "[no position]")
        cause = f"  <- {issue['caused_by']}" if issue.get("caused_by") else ""
        layers = f" {'/'.join(issue['layers'])}" if issue["layers"] else ""
        lines.append(f"{issue['id']:<4} {issue['code']:<22} {where}{layers}  {issue['title']}{cause}")
    if document["omitted"]:
        lines.append(f"... {document['omitted']} more (use --limit)")
    lines.append("View one: copper view RUN --issue ID")
    return "\n".join(lines)


def find_issue(issues: list[RunIssue], issue_id: str) -> RunIssue:
    for issue in issues:
        if issue.id.lower() == issue_id.lower():
            return issue
    raise KeyError(f"no issue {issue_id!r}; known: {', '.join(i.id for i in issues[:20])}")
