"""Copy a recorded Tracker showcase, checking its asset bindings before writing."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path

ASSETS = (
    "routing-top.svg", "routing-inner.svg", "routing-bottom.svg", "placement.svg",
    "electrical-ir.excerpt.json", "physical-ir.excerpt.json",
)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tracker-root", required=True, type=Path)
    args = parser.parse_args()
    source = args.tracker_root.resolve() / "docs/showcase"
    snapshot = json.loads((source / "snapshot.json").read_text(encoding="utf-8"))
    # Check all inputs before replacing any website asset.
    for name in ASSETS:
        digest = hashlib.sha256((source / name).read_bytes()).hexdigest()
        if digest != snapshot["rendered_assets"][name]:
            raise SystemExit(f"{name}: hash mismatch; regenerate the Tracker showcase first")
    destination = Path(__file__).resolve().parents[1] / "site/assets"
    destination.mkdir(parents=True, exist_ok=True)
    for name in (*ASSETS, "snapshot.json"):
        shutil.copyfile(source / name, destination / name)
    print("Copied recorded showcase. Review website captions, date and metrics before publishing.")


if __name__ == "__main__":
    main()
