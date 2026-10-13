"""Run editor browser regressions on disposable build/ copies with fresh services.

Use the project's Python, installed Playwright/Chromium and explicit footprint roots.
COPPER_PLAYWRIGHT_MODULE and COPPER_BROWSER_EXECUTABLE select local test tools.
"""
import argparse
import os
from pathlib import Path
from queue import Empty, Queue
import shutil
import subprocess
import sys
from threading import Thread

ROOT = Path(__file__).resolve().parents[2]
CASES = {
    "ui": ("mechanical_editor_demo", "mechanical-editor-ui", True),
    "save": ("mechanical_editor_demo", "mechanical-editor-save", True),
    "placement": ("mechanical_editor_demo", "mechanical-editor", False),
    "keyboard": ("mechanical_editor_demo", "mechanical-editor-keyboard", True),
    "curves": ("mechanical_curves", "mechanical-editor-curves", True),
    "anchors": ("mechanical_anchors", "mechanical-editor-anchors", True),
    "references": ("mechanical_reference", "mechanical-editor-references", True),
}


def run_case(name, roots):
    example, script, needs_source = CASES[name]
    output = ROOT / "build/editor-ui-smokes" / name
    output.mkdir(parents=True, exist_ok=True)
    original = ROOT / "examples" / example
    source = output / "board.copper"
    shutil.copyfile(original / "board.copper", source)
    for asset_directory in ("assets", "mechanical-assets"):
        if (original / asset_directory).is_dir():
            shutil.copytree(original / asset_directory, output / asset_directory, dirs_exist_ok=True)
    command = [sys.executable, "-u", "-c", "from pcbir.cli import main; raise SystemExit(main())",
               "edit-mechanical", str(source), "--locked", "--offline", "--no-browser"]
    for root in roots:
        command.extend(["--footprint-root", str(root)])
    hidden = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    server = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, **hidden)
    lines = Queue()
    def read_lines():
        for line in server.stdout:
            lines.put(line)
        lines.put(None)
    Thread(target=read_lines, daemon=True).start()
    try:
        while True:
            try:
                line = lines.get(timeout=30)
            except Empty:
                raise RuntimeError("Editor startup timed out")
            if line is None:
                raise RuntimeError("Editor exited: " + server.stderr.read())
            if "http://127.0.0.1:" in line:
                url = line[line.index("http://127.0.0.1:"):].strip()
                break
            print(line.strip(), flush=True)
        args = ["node", str(ROOT / "tests/browser" / (script + ".cjs")), url]
        if needs_source:
            args.append(str(source))
        args.append(str(output / "screenshot.png"))
        result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, timeout=150, **hidden)
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
        result.check_returncode()
        if source.read_bytes() != (original / "board.copper").read_bytes():
            raise RuntimeError("Browser test did not restore disposable source bytes")
    finally:
        server.terminate()
        server.wait(timeout=10)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cases", nargs="*", choices=tuple(CASES))
    parser.add_argument("--footprint-root", action="append", required=True, type=Path)
    args = parser.parse_args()
    for case in args.cases or CASES:
        run_case(case, args.footprint_root)
