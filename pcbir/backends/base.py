"""Common backend result types.

Backends consume semantic IR and return immutable artifacts. They never mutate
the authoritative design or add target-specific data to it.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from ..model import Board


@dataclass(frozen=True, slots=True)
class Artifact:
    name: str
    media_type: str
    content: str


@dataclass(frozen=True, slots=True)
class ArtifactManifest:
    backend: str
    target_version: str
    artifacts: tuple[Artifact, ...]
    warnings: tuple[str, ...] = ()


class Backend(Protocol):
    def generate(self, board: Board) -> ArtifactManifest:
        """Generate target artifacts without modifying ``board``."""

