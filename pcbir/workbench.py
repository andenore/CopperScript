"""One import for routing/placement iteration scripts.

Prefer the CLI (``copper issues``, ``copper view``). When a script is still
needed, start from these helpers instead of re-implementing loaders, codecs or
plotting::

    from pcbir.workbench import find_issue, load_run, run_issues, view, save_state, load_state

    run = load_run("build/CopperProbe/runs/<stamp>")
    issue = find_issue(run_issues(run), "P1")
    path, legend = view(run.scene(), window=issue.window(), nets=issue.nets,
                        markers=issue.points, output="build/views/P1.png")
    save_state("build/trials/d18.state.json", candidate_board, note="D18 alternate context")

Everything here is read-only with respect to runs and evidence.
"""

from __future__ import annotations

from pathlib import Path

from .run_issues import Run, RunIssue, filter_issues, find_issue, load_run, run_issues  # noqa: F401
from .scene import Scene, scene_from_kicad_pcb, scene_from_physical  # noqa: F401
from .state_codec import describe_state, load_state, save_state  # noqa: F401
from .view import Alignment, ViewRequest, align_reference, layers_in_window, render, window_around  # noqa: F401


def scene_of(target) -> Scene:
    """Scene from a PhysicalBoard, a Run, a .kicad_pcb path or a state file holding a board."""

    from .physical import PhysicalBoard
    if isinstance(target, Scene):
        return target
    if isinstance(target, PhysicalBoard):
        return scene_from_physical(target)
    if isinstance(target, Run):
        return target.scene()
    path = Path(target)
    if path.suffix == ".kicad_pcb":
        return scene_from_kicad_pcb(path)
    if path.is_dir():
        return load_run(path).scene()
    value = load_state(path)
    board = value if isinstance(value, PhysicalBoard) else getattr(value, "board", None) or \
        getattr(value, "source_board", None)
    if not isinstance(board, PhysicalBoard):
        raise ValueError(f"{path}: state does not hold a PhysicalBoard (found {type(value).__name__})")
    return scene_from_physical(board)


def view(target, *, window=None, nets=(), markers=(), layers=None, output="build/views/view.png",
         title="", reference=None, align=None, margin: float = 1.5):
    """Render ``target`` and return ``(path, legend_lines)``.

    ``window`` defaults to the markers, else the highlighted nets' pads.
    ``reference`` (a scene or .kicad_pcb) is aligned on ``align=("U1", "U1_REF")``.
    """

    scene = scene_of(target)
    markers = list(markers)
    if window is None:
        points = markers or [p.center for p in scene.pads if p.net in set(nets)]
        if not points:
            raise ValueError("give a window, markers or nets with pads")
        window = window_around(points, margin)
    window = tuple(window)
    if layers is None:
        layers = layers_in_window(scene, window, nets)
    overlay = alignment = None
    if reference is not None:
        if align is None:
            raise ValueError("reference overlays need align=(component, reference_component)")
        overlay, alignment = align_reference(scene_of(reference), align[1], scene, align[0])
    request = ViewRequest(window, tuple(layers), tuple(nets), markers, title)
    return render(scene, request, output, reference=overlay, alignment=alignment)
