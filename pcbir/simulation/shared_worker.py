"""Run a native ngspice shared library in a disposable child process.

Used for Windows installations such as KiCad that distribute a DLL instead of
a console executable. No native state is retained in the CopperScript process.
"""
from __future__ import annotations

import argparse
import ctypes
import os
from pathlib import Path


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("library", type=Path)
    parser.add_argument("--version", action="store_true")
    parser.add_argument("--deck", default="deck.cir")
    parser.add_argument("--raw", default="run.raw")
    parser.add_argument("--log", default="run.log")
    args = parser.parse_args(argv)
    library = args.library.resolve()
    handle = os.add_dll_directory(str(library.parent)) if os.name == "nt" else None
    messages = []
    exited = []
    sendchar_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_void_p)
    exit_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.c_bool, ctypes.c_bool, ctypes.c_int, ctypes.c_void_p)
    @sendchar_type
    def sendchar(message, ident, user):
        line = message.decode("utf-8", "replace")
        messages.append(line)
        print(line, flush=True)
        return 0
    @exit_type
    def controlled_exit(status, immediate, quit_exit, ident, user):
        exited.append(status or 1)
        return 0
    try:
        ngspice = ctypes.CDLL(str(library))
        ngspice.ngSpice_Init.argtypes = [ctypes.c_void_p] * 7
        ngspice.ngSpice_Init.restype = ctypes.c_int
        ngspice.ngSpice_Command.argtypes = [ctypes.c_char_p]
        ngspice.ngSpice_Command.restype = ctypes.c_int
        if ngspice.ngSpice_Init(sendchar, None, controlled_exit, None, None, None, None):
            raise RuntimeError("ngSpice_Init failed")
        def command(value):
            if ngspice.ngSpice_Command(value.encode("utf-8")) or exited:
                raise RuntimeError(f"ngspice command failed: {value}")
        command("set nomoremode")
        if args.version:
            command("version -f")
            return 0
        # These arguments are fixed by the parent runner, never supplied as SPICE expressions.
        if (args.deck, args.raw, args.log) != ("deck.cir", "run.raw", "run.log"):
            raise RuntimeError("worker accepts only fixed artifact filenames")
        command("set filetype=ascii")
        command("source deck.cir")
        command("run")
        command("write run.raw all")
        Path(args.log).write_text("\n".join(messages) + "\n", encoding="utf-8")
        return 0 if Path(args.raw).is_file() else 1
    except (OSError, RuntimeError) as exc:
        print(str(exc), flush=True)
        if not args.version:
            Path(args.log).write_text("\n".join([*messages, str(exc)]) + "\n", encoding="utf-8")
        return 1
    finally:
        if handle is not None:
            handle.close()


if __name__ == "__main__":
    raise SystemExit(main())
