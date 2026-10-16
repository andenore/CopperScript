"""Passive DC feeds propagate source reachability without merging net identities."""
from dataclasses import replace
import pytest
from pcbir import compile_file, check, CopperScriptError
from tempfile import TemporaryDirectory
from pathlib import Path
from pcbir.elaborate import elaborate

SOURCE = """
part Feed {
    dc_power_path = "P1,P2";
    pin P1 { number = "1"; domains = "power"; directions = "passive"; }
    pin P2 { number = "2"; domains = "power"; directions = "passive"; }
}
part Load {
    pin VDD { number = "1"; domains = "power"; directions = "input"; }
}
board PassiveFeed {
    component F1: Feed;
    component F2: Feed;
    component U: Load;
    net SOURCE { F1.P1; }
    net MID { F1.P2; F2.P1; }
    net FILTERED { F2.P2; U.VDD; }
    supply SOURCE { voltage = 1.8 V; external = true; }
    supply FILTERED { voltage = 1.8 V; external = false; }
}
"""

def compile_source(source):
    with TemporaryDirectory() as temporary:
        root = Path(temporary)
        parts = root / 'parts'
        parts.mkdir()
        load_at = source.index('part Load')
        board_at = source.index('board PassiveFeed')
        (parts / 'feed.copper').write_text(source[:load_at])
        (parts / 'load.copper').write_text(source[load_at:board_at])
        board = source[board_at:].replace('board PassiveFeed {', 'board PassiveFeed { import p "./parts";')
        board = board.replace(': Feed;', ': p.Feed;').replace(': Load;', ': p.Load;')
        (root / 'board.copper').write_text(board)
        return compile_file(root / 'board.copper')

def codes(source):
    return {d.code for d in check(compile_source(source))}

def test_chained_explicit_dc_paths_keep_separate_net_and_land_identity():
    board = compile_source(SOURCE)
    assert check(board) == []
    flat = elaborate(board)
    assert {n.name for n in flat.nets} == {"SOURCE", "MID", "FILTERED"}
    assert all(not p.internal_pad_groups for p in flat.library.values())
    assert flat.library[flat.components[0].part].dc_power_path == ("P1", "P2")

def test_passive_source_reachability_is_bidirectional():
    assert codes(SOURCE.replace('net SOURCE { F1.P1; }', 'net SOURCE { F1.P2; }')
                .replace('net MID { F1.P2; F2.P1; }', 'net MID { F1.P1; F2.P1; }')) == set()

def test_unpowered_loop_creates_no_source():
    source = SOURCE.replace('supply SOURCE { voltage = 1.8 V; external = true; }', '')
    source = source.replace('net SOURCE { F1.P1; }', '').replace('net FILTERED { F2.P2; U.VDD; }',
        'net FILTERED { F2.P2; F1.P1; U.VDD; }')
    assert 'UNSOURCED_POWER_INPUT' in codes(source)

@pytest.mark.parametrize('change', ('omitted_contract', 'unassembled', 'disconnected', 'ambiguous'))
def test_no_implicit_or_unassembled_or_ambiguous_feed(change):
    source = SOURCE
    if change == 'omitted_contract': source = source.replace('dc_power_path = "P1,P2";', '')
    if change == 'unassembled': source = source.replace('part Feed {', 'part Feed { assembled = false;')
    if change == 'disconnected': source = source.replace('net SOURCE { F1.P1; }', 'net SOURCE { }')
    if change == 'ambiguous': source = source.replace('net MID {', 'net EXTRA { F1.P1; } net MID {')
    assert 'UNSOURCED_POWER_INPUT' in codes(source)

@pytest.mark.parametrize('path', ('P1', 'P1,P1', 'P1,P3', 'P1,P2,P1'))
def test_bad_path_contracts_are_rejected(path):
    with pytest.raises(CopperScriptError):
        compile_source(SOURCE.replace('"P1,P2"', '"'+path+'"'))

@pytest.mark.parametrize('replacement', ('directions = "output"', 'domains = "analog"'))
def test_dc_path_cannot_turn_an_active_pin_or_signal_path_into_a_supply(replacement):
    original = 'directions = "passive"' if replacement.startswith('directions') else 'domains = "power"'
    with pytest.raises(CopperScriptError): compile_source(SOURCE.replace(original, replacement, 1))
