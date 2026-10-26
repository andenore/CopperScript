"""One append-only ordinary exit retry after disposable plane contacts leave."""
from dataclasses import dataclass
from .physical import PadReference


@dataclass(frozen=True, slots=True)
class ReleasedExitRetryReport:
    status: str = "not_needed"
    passes: int = 0
    maximum_passes: int = 1
    selected_pads: tuple[PadReference, ...] = ()
    completed_pads: tuple[PadReference, ...] = ()
    remaining_pads: tuple[PadReference, ...] = ()
    added_tracks: int = 0
    added_vias: int = 0
    reason: str = ""


