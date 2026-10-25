"""Optional native kernels for the unified router (see astar.c).

``astar()`` returns a ctypes function, or None when no prebuilt kernel matches
this platform or ``COPPER_NO_NATIVE`` is set; callers then use pure Python.
"""
from __future__ import annotations

import ctypes
import os
import platform
from pathlib import Path

_LIBRARIES = {
    ("Linux", "x86_64"): "astar_linux_x86_64.so",
    ("Windows", "AMD64"): "astar_windows_x86_64.dll",
    ("Windows", "x86_64"): "astar_windows_x86_64.dll",
}
_loaded = None


def astar():
    global _loaded
    if _loaded is not None:
        return _loaded or None
    _loaded = False
    if os.environ.get("COPPER_NO_NATIVE"):
        return None
    name = _LIBRARIES.get((platform.system(), platform.machine()))
    if name is None:
        return None
    path = Path(__file__).with_name(name)
    try:
        library = ctypes.CDLL(str(path))
        function = library.pcbir_astar
    except (OSError, AttributeError):
        return None
    P = ctypes.c_void_p
    I = ctypes.c_int
    D = ctypes.c_double
    function.restype = I
    function.argtypes = [I, I, I, P, P, P, P, P, P, P, P, I, P, I, P, D, D,
                         I, I, I, I, I, I, I, I, P, P, I, P, P, P, P, I, P, P, P, I, P, I,
                         ctypes.POINTER(I), P, I,
                         # Nested lattice block tables (S3): n_blocks, blocks,
                         # block_of, ux, uy, twins, steps; block_of None = uniform.
                         I, P, P, P, P, P, P]
    _loaded = function
    return function
