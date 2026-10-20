"""Stage checkpoints and failure records that shorten routing iterations.

A placement checkpoint lets the next ``route-board`` run skip placement and
global routing (``--reuse-placement``). It is keyed on the physical input board,
the placement/global/feedback options and the compiler code. Reuse is refused
when the board or options differ; a code change is allowed but recorded in the
report, because the caller is deliberately holding placement fixed while
changing later stages.

Checkpoints and error records are working artifacts, never evidence.
"""

from __future__ import annotations

import hashlib
import json
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

from .state_codec import describe_state, encode, load_state, save_state

ERROR_SCHEMA = "copperscript-route-error/v1"


def code_digest() -> str:
    """Digest of the pcbir sources (line endings normalised), identifying the algorithms."""

    root = Path(__file__).resolve().parent
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*.py")):
        if "__pycache__" in path.parts:
            continue
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(path.read_bytes().replace(b"\r\n", b"\n") + b"\0")
    return "sha256:" + digest.hexdigest()


def _options_digest(*options) -> str:
    def canonical(value):
        if isinstance(value, list):
            return [canonical(item) for item in value]
        if isinstance(value, dict):
            result = {key: canonical(item) for key, item in value.items()}
            # State files retain mapping insertion order for round trips; option
            # identity depends on contents. Tuple/list order remains significant.
            for tag in ("$map", "$set", "$frozenset"):
                if tag in result:
                    result[tag] = sorted(result[tag], key=lambda item: json.dumps(item, sort_keys=True))
            return result
        return value

    body = json.dumps(canonical(encode(options)), sort_keys=True, separators=(",", ":"))
    return "sha256:" + hashlib.sha256(body.encode()).hexdigest()


def placement_key(board, placement_options, global_options, feedback_options, power_area_options=None,
                  extra=None) -> dict:
    """``extra`` holds any further inputs placement saw (fanout settings for support-pattern trials)."""
    from .drc import physical_board_digest
    return {"stage": "placement", "board": physical_board_digest(board),
            "options": _options_digest(placement_options, global_options, feedback_options, power_area_options,
                                       extra),
            "code": code_digest()}


def save_placement_checkpoint(path: str | Path, placement, key: dict) -> Path:
    return save_state(path, placement, key=key, created_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))


def load_placement_checkpoint(path: str | Path, key: dict):
    """Return ``(placement, provenance)``; raise ValueError when it does not apply."""

    from .routeflow import PlacementRoutingResult
    try:
        value, metadata = load_state(path, with_metadata=True)
    except OSError as exc:
        raise ValueError(f"cannot read placement checkpoint {path}: {exc.strerror or exc}") from exc
    saved = metadata.get("key") or {}
    if not isinstance(value, PlacementRoutingResult) or saved.get("stage") != "placement":
        raise ValueError(f"{path} is not a placement checkpoint")
    if saved.get("board") != key["board"]:
        raise ValueError(f"{path} was made for a different physical board (source, footprints, layers or "
                         "fabrication profile changed); run without --reuse-placement")
    if saved.get("options") != key["options"]:
        raise ValueError(f"{path} was made with different placement/global/feedback options; run without "
                         "--reuse-placement or restore those options")
    return value, {"path": str(path), "content_digest": describe_state(path)["content_digest"],
                   "created_utc": metadata.get("created_utc"),
                   "code_changed": saved.get("code") != key["code"],
                   "candidate": value.placement_candidate}


@dataclass
class StageTracker:
    """Wraps a progress callback and remembers where a run got to."""

    callback: object = None
    started: list[str] = field(default_factory=list)
    finished: list[str] = field(default_factory=list)
    origin: float = field(default_factory=time.monotonic)

    def __call__(self, phase, event, details):
        if event == "started":
            self.started.append(phase)
        elif event in {"finished", "reused", "stopped", "blocked", "failed"}:
            self.finished.append(phase)
        if self.callback is not None:
            self.callback(phase, event, details)

    def open_stages(self) -> list[str]:
        remaining = list(self.started)
        for phase in self.finished:
            if phase in remaining:
                remaining.remove(phase)
        return remaining


def write_error_record(report_path: Path, *, board_name: str, error: BaseException, tracker: StageTracker | None,
                       args_summary: dict) -> Path:
    """Persist a failed run's message and stage, so `copper issues` can show it."""

    message = str(error)
    components = sorted(set(re.findall(r"component '([^']+)'", message)))
    document = {
        "schema": ERROR_SCHEMA, "status": "error", "board": board_name,
        "error_type": type(error).__name__, "message": message[:4000],
        "components": components,
        "open_stages": tracker.open_stages() if tracker else None,
        "completed_stages": list(dict.fromkeys(tracker.finished)) if tracker else None,
        "elapsed_seconds": round(time.monotonic() - tracker.origin, 3) if tracker else None,
        "options": args_summary,
        "note": "Working record of a failed run; not routing or signoff evidence.",
    }
    path = report_path.with_name(report_path.stem + ".error.json")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path
