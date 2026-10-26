"""Exact temporary copper obstacles for append-only producer transactions."""
from collections import Counter
from dataclasses import dataclass, fields, replace


@dataclass(frozen=True)
class ReservedCopperScene:
    base: object
    board: object

    def extract(self, after):
        """Transfer only producer additions; retain the original source prefix."""
        if any(getattr(after, f.name) != getattr(self.board, f.name)
               for f in fields(self.board) if f.name not in {'tracks', 'vias', 'metadata'}):
            raise ValueError('temporary copper producer changed physical intent')
        if (after.tracks[:len(self.board.tracks)] != self.board.tracks
                or after.vias[:len(self.board.vias)] != self.board.vias):
            raise ValueError('temporary copper producer must retain its exact input prefix')
        return replace(self.base,
            tracks=(*self.base.tracks, *after.tracks[len(self.board.tracks):]),
            vias=(*self.base.vias, *after.vias[len(self.board.vias):]), metadata=after.metadata)


def reserve_copper_scene(board, tracks=(), vias=()):
    """Include missing actual occurrences, without changing source ownership.

    Maximal multiplicity preserves both the caller's reservations and the base
    prefix. The caller must keep producer nets disjoint from temporary nets;
    temporary connectivity is never transferred or used as native evidence.
    """
    values = []
    for current, reserved in ((board.tracks, tracks), (board.vias, vias)):
        remaining = Counter(reserved)-Counter(current)
        appended = []
        for value in reserved:
            if remaining[value]:
                appended.append(value)
                remaining[value] -= 1
        values.append((*current, *appended))
    return ReservedCopperScene(board, replace(board, tracks=values[0], vias=values[1]))
