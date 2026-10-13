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


def test_large_inventory_hashes_fresh_bytes_in_canonical_order_with_bounded_reads(tmp_path, monkeypatch):
    from threading import Lock
    from time import sleep
    for index in range(260):
        (tmp_path / f"{index:04}.kicad_mod").write_bytes(str(index).encode())
    expected = tuple(packages.LockedFile(f"{index:04}.kicad_mod", len(str(index)),
                       hashlib.sha256(str(index).encode()).hexdigest()) for index in range(260))
    original = packages._inventory_file
    lock = Lock()
    active = maximum = 0
    def read(candidate):
        nonlocal active, maximum
        with lock:
            active += 1
            maximum = max(maximum, active)
        try:
            sleep(0.001)
            return original(candidate)
        finally:
            with lock:
                active -= 1
    monkeypatch.setattr(packages, "_inventory_file", read)
    assert packages._module_inventory(tmp_path) == expected
    assert 1 < maximum <= 4
    (tmp_path / "0000.kicad_mod").write_bytes(b"X")
    assert packages._module_inventory(tmp_path)[0].sha256 == hashlib.sha256(b"X").hexdigest()
