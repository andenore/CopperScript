"""Copy or check the shared Cu mark; no runtime cross-repository dependency."""
from __future__ import annotations

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "pcbir/editor/assets"


def targets(review_root: Path | None = None) -> dict[Path, Path]:
    result = {
        ROOT / "site/assets/copper-logo.svg": SOURCE / "copper-logo.svg",
        ROOT / "site/assets/favicon.svg": SOURCE / "copper-logo.svg",
        ROOT / "site/assets/copper-logo.png": SOURCE / "copper-logo.png",
    }
    if review_root is not None:
        target = review_root / "copper_review/web"
        if not target.is_dir():
            raise ValueError(f"Not a CopperReview checkout: {review_root}")
        result[target / "copper-logo.svg"] = SOURCE / "copper-logo.svg"
    return result


def check_copies(review_root: Path | None = None) -> list[str]:
    errors = []
    for target, source in targets(review_root).items():
        if not target.is_file() or target.read_bytes() != source.read_bytes():
            errors.append(f"Logo copy differs: {target}")
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--review-root", type=Path, help="Optional standalone CopperReview checkout")
    parser.add_argument("--check", action="store_true", help="Verify bytes without writing")
    args = parser.parse_args()
    try:
        if args.check:
            errors = check_copies(args.review_root)
            if errors:
                raise ValueError("\n".join(errors))
            print("Shared Cu logo copies match.")
        else:
            for target, source in targets(args.review_root).items():
                target.write_bytes(source.read_bytes())
                print(f"Updated {target}")
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
