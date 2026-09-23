"""Choose a physical via span shared by routing stages.

Without an explicit technology table, the only safe default is a through-via
spanning the complete stackup. A guide's layer transition is electrical intent,
not proof that blind or buried fabrication is available.
"""

from __future__ import annotations

from .physical import CopperLayer, PhysicalBoard, ViaKind, select_via_technology


def physical_via_span(
    board: PhysicalBoard, first: CopperLayer, second: CopperLayer
) -> tuple[CopperLayer, CopperLayer, str | None] | None:
    stackup = board.stackup
    outer_first, outer_last = stackup.copper_layers[0], stackup.copper_layers[-1]
    if not stackup.via_technologies:
        return outer_first, outer_last, None
    for start, end in ((first, second), (outer_first, outer_last)):
        try:
            technology_id = select_via_technology(
                stackup, start, end,
                board.rules.default_via_size_nm, board.rules.default_via_drill_nm,
            )
        except ValueError:
            continue
        technology = next(
            item for item in stackup.via_technologies if item.id == technology_id
        )
        if (start, end) != (first, second) and technology.kind is not ViaKind.THROUGH:
            continue
        ordered = sorted((start, end), key=stackup.copper_layers.index)
        return ordered[0], ordered[1], technology_id
    return None
