"""Repository entry point for the CopperScript device generator."""

from pathlib import Path
import sys


sys.path.insert(0, str(Path(__file__).parents[1]))

from pcbir.devicegen import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
