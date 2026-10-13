"""Migrated examples select unchanged owner copper from their source declarations."""
from hashlib import sha256
import json
from pathlib import Path

import pytest

from pcbir.compiler import compile_file
from pcbir.macro_bindings import prepare_macro_bindings
from pcbir.packages import resolve_module_asset
from examples.tps62130a_macro.trial import SOURCE, make_trial


@pytest.mark.parametrize('enabled', [False, True])
def test_tps_probe_source_variant_materializes_without_executing_library_code(tmp_path, enabled):
    board = make_trial(tmp_path, enabled=enabled, offline=True)
    assert board.materialized_macros == ('regulator',)
    assert board.tracks and board.vias and board.polygons
    assert (tmp_path / 'source.copper.txt').exists()
    assert json.loads(board.metadata['source_physical_macros'])[0]['resolution'] == 'local_input'


def test_tps_always_on_adaptation_preserves_every_source_copper_object():
    selection = prepare_macro_bindings(compile_file(SOURCE, locked=True, offline=True))[0]
    adapted = json.loads(selection.path.read_bytes())
    original_path = resolve_module_asset(SOURCE,
        'github.com/andenore/CopperLib/packages/circuits/ti/tps62130a-buck/assets/tps62130a-six-layer-always-on.json',
        locked=True, offline=True)
    original = json.loads(original_path.read_bytes())
    assert adapted['source']['base_asset_sha256'] == sha256(original_path.read_bytes()).hexdigest()
    for key in original.keys() - {'members', 'source'}:
        assert adapted[key] == original[key]
    for member, prior in zip(adapted['members'], original['members'], strict=True):
        assert {k: v for k, v in member.items() if k not in {'footprint', 'footprint_digest'}} == {
            k: v for k, v in prior.items() if k not in {'footprint', 'footprint_digest'}}
