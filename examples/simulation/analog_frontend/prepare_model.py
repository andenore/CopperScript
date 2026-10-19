"""Prepare the byte-pinned off-the-shelf TI model; never rewrite vendor syntax."""
from __future__ import annotations

import argparse
import hashlib
import io
from pathlib import Path
from urllib.request import urlopen
import zipfile

ROOT = Path(__file__).resolve().parent
SOURCE = "https://www.ti.com/lit/zip/SBOMA34"
ARCHIVE_SHA256 = "9e55fcaa23d54cee025dda3fa9872b11c41c531f82e0802c2b93d51e943666e5"
MODEL_SHA256 = "fc5b020e63346e511bd808bf41c856b0150b000bcf8a41fe00eeececb1f422a5"


def prepare(archive: Path | None = None) -> Path:
    destination = ROOT / "models/vendor/OPAx197.LIB"
    if destination.exists():
        if hashlib.sha256(destination.read_bytes()).hexdigest() != MODEL_SHA256:
            raise ValueError("existing TI model changed; restore the pinned vendor bytes explicitly")
        return destination
    data = archive.read_bytes() if archive else urlopen(SOURCE, timeout=60).read()
    if hashlib.sha256(data).hexdigest() != ARCHIVE_SHA256:
        raise ValueError("TI archive differs from the reviewed bundle; no model was installed")
    with zipfile.ZipFile(io.BytesIO(data)) as bundle:
        model = bundle.read("OPAx197.LIB")
    if hashlib.sha256(model).hexdigest() != MODEL_SHA256:
        raise ValueError("TI model differs from the reviewed bytes")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as stream:
        stream.write(model)
    return destination


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, help="use a previously downloaded pinned SBOMA34 ZIP")
    args = parser.parse_args()
    print(prepare(args.archive))
