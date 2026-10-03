"""Install the pinned native engine or publish bounded example evidence in CI."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def install(prefix: Path):
    pin = json.loads((ROOT / ".github/simulation-toolchain.json").read_text())
    prefix = prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=True)
    request = Request(pin["source_url"], headers={"User-Agent": "CopperScript-CI/1.0", "Accept": "application/octet-stream"})
    with urlopen(request, timeout=60) as response:
        archive = response.read()
    if hashlib.sha256(archive).hexdigest() != pin["source_sha256"]:
        raise ValueError("ngspice source archive does not match the committed SHA-256; no build was attempted")
    with tempfile.TemporaryDirectory(prefix="copper-ngspice-build-") as temporary:
        folder = Path(temporary)
        source_archive = folder / "ngspice.tar.gz"; source_archive.write_bytes(archive)
        with tarfile.open(source_archive, "r:gz") as source:
            source.extractall(folder, filter="data")
        directory = folder / f"ngspice-{pin['ngspice_version']}"
        commands = [[str(directory / "configure"), f"--prefix={prefix}", *pin["configure"]],
                    ["make", f"-j{min(os.cpu_count() or 2, 4)}"], ["make", "install"]]
        with (prefix / "build.log").open("w", encoding="utf-8") as log:
            try:
                for command in commands:
                    print("Building ngspice: " + " ".join(command), flush=True)
                    subprocess.run(command, cwd=directory, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=600)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                log.flush()
                print((prefix / "build.log").read_text(encoding="utf-8", errors="replace")[-16000:], file=sys.stderr, flush=True)
                raise
    version = subprocess.run([str(prefix / "bin/ngspice"), "--version"], capture_output=True, text=True, check=True).stdout
    match = re.search(r"ngspice-(\d+)", version)
    if not match or int(match[1]) != pin["ngspice_version"]:
        raise ValueError("compiled ngspice does not match the pinned major version")
    (prefix / "toolchain.json").write_text(json.dumps({**pin, "version_output": version}, indent=2) + "\n")
    print(f"Installed ngspice {pin['ngspice_version']}: {prefix / 'bin/ngspice'}")


def examples():
    sys.path.insert(0, str(ROOT))
    from pcbir.loader import load_board
    from pcbir.simulation import load_plan, run_simulation
    failure = False
    for name in ("rc_filter", "inrush"):
        directory = ROOT / "examples/simulation"
        result = run_simulation(load_board(directory / f"{name}.copper"), load_plan(directory / f"{name}.json"),
                                ROOT / "build/sim" / name, timeout=30, source_path=directory / f"{name}.copper")
        print(f"{name}: {result['status']} ({result['report']})")
        failure |= result["status"] != "passed" or result["output_status"] != "complete"
    return int(failure)


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    setup = commands.add_parser("install"); setup.add_argument("--prefix", type=Path, required=True)
    commands.add_parser("examples")
    args = parser.parse_args()
    if args.command == "install":
        install(args.prefix)
        return 0
    return examples()


if __name__ == "__main__":
    raise SystemExit(main())
