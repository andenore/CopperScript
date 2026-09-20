"""Command-line compiler and checker for CopperScript."""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Sequence

from .erc import check, has_errors
from .loader import BoardLoadError, load_board
from .power import analyze_power_states
from .serializer import board_to_json, write_json


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="copper", description="CopperScript v0.1 compiler")
    subparsers = parser.add_subparsers(dest="command", required=True)
    check_parser = subparsers.add_parser("check", help="run electrical-rules checks")
    check_parser.add_argument("board", type=Path, help="a .copper source file")

    power_parser = subparsers.add_parser(
        "power-check", help="analyze explicit steady-state power scenarios"
    )
    power_parser.add_argument("board", type=Path, help="a .copper source file")

    compile_parser = subparsers.add_parser("compile", help="compile source to JSON IR")
    compile_parser.add_argument("board", type=Path, help="a .copper source file")
    compile_parser.add_argument("-o", "--output", type=Path, help="write JSON IR to this file")
    compile_parser.add_argument(
        "--no-check", action="store_true", help="emit IR even when electrical checks fail"
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.command in {"check", "power-check", "compile"}:
        try:
            board = load_board(args.board)
        except BoardLoadError as exc:
            print(f"COMPILE ERROR: {exc}")
            return 2
        diagnostics = (
            analyze_power_states(board) if args.command == "power-check" else check(board)
        )
        if args.command == "check":
            if diagnostics:
                for diagnostic in diagnostics:
                    print(diagnostic)
            else:
                print(f"OK: {board.name} passed ERC")
            return 1 if has_errors(diagnostics) else 0
        if args.command == "power-check":
            if diagnostics:
                for diagnostic in diagnostics:
                    print(diagnostic)
            else:
                print(f"OK: {board.name} passed power-state analysis")
            return 1 if has_errors(diagnostics) else 0

        if has_errors(diagnostics) and not args.no_check:
            for diagnostic in diagnostics:
                print(diagnostic)
            print("Compilation stopped because ERC reported errors.")
            return 1
        if args.output:
            try:
                write_json(board, args.output)
            except OSError as exc:
                print(f"OUTPUT ERROR: {exc}")
                return 2
            print(f"Compiled {board.name} -> {args.output}")
        else:
            print(board_to_json(board), end="")
        return 0
    return 2
