from dataclasses import replace
import pcbir.decoupling_paths as paths
from pcbir.physical import Point
from test_decoupling_crosslayer_paths import fixture

def test_context_cache_reuses_only_exact_immutable_board_identity(monkeypatch):
    paths._cached_context.cache_clear(); paths._cached_board_digest.cache_clear()
    board,_=fixture(); original=paths.physical_board_digest; calls=[]
    def observe(value):
        calls.append(value)
        return original(value)
    monkeypatch.setattr(paths,"physical_board_digest",observe)
    first=paths._context(board); digest=paths._board_digest(board)
    assert paths._context(board)==first and paths._board_digest(board)==digest
    assert len(calls)==2
    clone=replace(board)
    assert paths._context(clone)==first and paths._board_digest(clone)==digest
    assert len(calls)==4
    changed=replace(board,placements=(replace(board.placements[0],position=Point.mm(7,8)),*board.placements[1:]))
    assert paths._context(changed)!=first and paths._board_digest(changed)!=digest
    assert len(calls)==6
    for _ in range(20):
        paths._context(replace(board)); paths._board_digest(replace(board))
    assert paths._cached_context.cache_info().currsize==8
    assert paths._cached_board_digest.cache_info().currsize==8
