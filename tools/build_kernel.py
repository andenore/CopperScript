"""Build the native A* kernel for Linux (.so) and Windows (.dll).

The kernel has no C runtime dependency, so a Windows DLL can be built on
Linux with clang and lld-link, without an MSVC installation:

    python tools/build_kernel.py            # both, as available
    python tools/build_kernel.py --zig /path/to/zig   # cross-build both platforms

Outputs go to pcbir/native/. The router falls back to pure Python when no
kernel loads (for example on macOS until a dylib is added here).
"""
from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NATIVE = ROOT / "pcbir" / "native"
SOURCE = NATIVE / "astar.c"


def build_linux(out: Path) -> bool:
    compiler = shutil.which("gcc") or shutil.which("clang")
    if not compiler:
        return False
    subprocess.run([compiler, "-O3", "-fPIC", "-shared", "-fvisibility=hidden", "-nostdlib",
                    "-ffreestanding", "-fno-builtin",
                    "-o", str(out), str(SOURCE)], check=True)
    return True


def build_windows(out: Path) -> bool:
    clang, linker = shutil.which("clang"), shutil.which("lld-link")
    if not clang or not linker:
        return False
    with tempfile.TemporaryDirectory() as tmp:
        obj = Path(tmp) / "astar.obj"
        subprocess.run([clang, "--target=x86_64-pc-windows-msvc", "-O3", "-fno-stack-protector",
                        "-ffreestanding", "-fno-builtin",
                        "-mno-stack-arg-probe", "-c", str(SOURCE), "-o", str(obj)], check=True)
        subprocess.run([linker, "/dll", "/noentry", "/nodefaultlib", "/machine:x64",
                        f"/out:{out}", str(obj)], check=True)
    for extra in (out.with_suffix(".lib"), out.with_suffix(".exp")):
        extra.unlink(missing_ok=True)
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--zig', help='Zig executable for a freestanding cross-build of both kernels')
    args = parser.parse_args()
    built = []
    if args.zig:
        subprocess.run([args.zig, 'cc', '-target', 'x86_64-linux-gnu', '-O3', '-fPIC',
                        '-shared', '-fvisibility=hidden', '-nostdlib', '-ffreestanding', '-fno-builtin',
                        '-o', str(NATIVE / 'astar_linux_x86_64.so'), str(SOURCE)], check=True)
        built.append("astar_linux_x86_64.so")
        subprocess.run([args.zig, 'build-lib', '-target', 'x86_64-windows-msvc', '-dynamic',
                        '-O', 'ReleaseFast', '-fstrip', '-fno-stack-check', '-fno-stack-protector', '-fno-compiler-rt',
                        '--name', 'astar_windows_x86_64',
                        '-femit-bin=' + str(NATIVE / 'astar_windows_x86_64.dll'),
                        '-cflags', '-ffreestanding', '-fno-builtin', '-mno-stack-arg-probe', '--',
                        str(SOURCE)], check=True)
        built.append("astar_windows_x86_64.dll")
    else:
        if build_linux(NATIVE / "astar_linux_x86_64.so"):
            built.append("astar_linux_x86_64.so")
        if build_windows(NATIVE / "astar_windows_x86_64.dll"):
            built.append("astar_windows_x86_64.dll")
    for suffix in ('.lib', '.exp', '.pdb'):
        (NATIVE / ('astar_windows_x86_64' + suffix)).unlink(missing_ok=True)
    digest = hashlib.sha256(SOURCE.read_bytes()).hexdigest()
    (NATIVE / "astar.source.sha256").write_text(digest + "\n", encoding="utf-8")
    print("built:", ", ".join(built) or "nothing (no compiler found)")
    return 0 if built else 1


if __name__ == "__main__":
    sys.exit(main())
