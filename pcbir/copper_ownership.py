"""Snapshot-bound occurrence ownership for disposable contact preparations.

This records producer evidence, not electrical qualification. Unattributed or
ambiguous copper stays held. An accepted/frozen ledger cannot release copper.
"""
from dataclasses import dataclass

from .physical import PadReference, TrackSegment, Via


@dataclass(frozen=True, slots=True)
class CopperUse:
    id: str
    kind: str
    terminals: tuple[PadReference, ...]
    tracks: tuple[TrackSegment, ...] = ()
    vias: tuple[Via, ...] = ()
    dependencies: tuple[str, ...] = ()

    def __post_init__(self):
        if not self.id or not self.kind or not self.terminals:
            raise ValueError('copper use requires an identity, kind and terminals')
        for name in ('terminals', 'tracks', 'vias', 'dependencies'):
            object.__setattr__(self, name, tuple(getattr(self, name)))


