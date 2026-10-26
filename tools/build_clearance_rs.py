"""Build the prototype Rust clearance mirror (pcbir/native/clearance_rs).

    python tools/build_clearance_rs.py

Needs cargo. The extension lands in pcbir/native/ as _clearance_rs.abi3.so
(Linux). RoutingClearanceIndex uses it when it imports and neither
COPPER_NO_RUST_CLEARANCE nor COPPER_NO_NATIVE is set; otherwise the index
answers every query in pure Python, with identical results.
"""
from __future__ import annotations

import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "pcbir" / "native" / "clearance_rs"
OUT = ROOT / "pcbir" / "native"


def main() -> int:
    cargo = shutil.which("cargo")
    if cargo is None:
        print("cargo not found; the index keeps its pure-Python path", file=sys.stderr)
        return 1
    subprocess.run([cargo, "build", "--release", "--manifest-path", str(CRATE / "Cargo.toml")], check=True)
    system = platform.system()
    built, target = {
        "Linux": ("lib_clearance_rs.so", "_clearance_rs.abi3.so"),
        "Darwin": ("lib_clearance_rs.dylib", "_clearance_rs.abi3.so"),
        "Windows": ("_clearance_rs.dll", "_clearance_rs.pyd"),
    }[system]
    shutil.copy2(CRATE / "target" / "release" / built, OUT / target)
    print(f"built {OUT / target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
