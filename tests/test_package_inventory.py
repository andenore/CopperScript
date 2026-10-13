import hashlib
from pathlib import Path

import pytest

from pcbir import packages


def test_inventory_prunes_ignored_directories_and_retains_exact_sorted_hashes(tmp_path, monkeypatch):
    content = {"z/Part.copper": b"part Z {}\n", "a/A.KICAD_MOD": b"(footprint A)\n",
               "copper.mod": b"module test/inventory\n", "a/layout.json": b"{}\n"}
    for name, data in {**content, ".git/objects/ignored.copper": b"ignored",
                       "cache/deep/ignored.json": b"ignored", "a/__pycache__/ignored.json": b"ignored",
                       "notes.txt": b"not locked"}.items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    visited = []
    original = packages.os.scandir
    def scan(path):
        visited.append(Path(path).relative_to(tmp_path).as_posix())
        return original(path)
    monkeypatch.setattr(packages.os, "scandir", scan)
    expected = tuple(packages.LockedFile(name, len(data), hashlib.sha256(data).hexdigest())
                     for name, data in sorted(content.items()))
    assert packages._module_inventory(tmp_path) == expected
    assert set(visited) == {".", "a", "z"}
    # Every operation must read fresh bytes, including same-size content edits.
    (tmp_path / "a/layout.json").write_bytes(b"[]\n")
    assert packages._module_inventory(tmp_path) != expected


def test_inventory_still_rejects_symlinks_even_for_unlocked_file_types(tmp_path):
    target = tmp_path / "original.txt"
    target.write_text("outside the inventory")
    try:
        (tmp_path / "linked.txt").symlink_to(target)
    except OSError:
        pytest.skip("symlink creation is unavailable")
    with pytest.raises(ValueError, match="package assets may not be symlinks: linked.txt"):
        packages._module_inventory(tmp_path)
