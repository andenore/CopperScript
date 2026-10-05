# CopperScript editor bug and feature reports

This document records the reported editor behaviours, the verification method,
and the disposition of each item.  Verification was done against the current
`pcbir/editor` implementation, the editor unit tests, and the disposable
Playwright smoke tests where browser interaction was relevant.

## E-001 — keyboard shortcuts are incomplete

**Status: confirmed and fixed.**

The editor already handled arrows, `R`, `+`, `-`, `F`, and `Escape`, but had no
keyboard action for temporary lock/unlock, undo/redo, reverse rotation, or side
flip.  The toolbar remained the only way to perform those actions.

The editor now supports:

| Shortcut | Action |
| --- | --- |
| `L` | Lock the selected temporary pose |
| `Shift+L` or `U` | Unlock the selected temporary pose |
| `R` / `Shift+R` | Rotate to the next / previous allowed orientation |
| `S` | Toggle front/back when the side is not constrained |
| `Ctrl/Cmd+Z` | Undo |
| `Ctrl/Cmd+Shift+Z` or `Ctrl/Cmd+Y` | Redo |
| `Home` or `F` | Fit the board |

Existing arrow movement, zoom, pan, `Enter`, and `Escape` behaviours are
unchanged.

## E-002 — pad names are not visible while inspecting a package

**Status: confirmed and fixed.**

Before this change, pads had a browser `<title>` containing the reference,
number, and net, but no visible label.  The physicalizer now carries optional
part pin names as board metadata.  The scene exposes that name per pad and the
editor renders the name (or physical pad number when no name is available) once
the viewport is zoomed to 35 mm or less.  The complete reference/net tooltip
remains available at every zoom level.

## E-003 — connector body boxes can be offset from the actual footprint

**Status: confirmed and fixed.**

The editor previously drew every body as a `body_size` rectangle centered at the
placement origin.  That is incorrect for footprints whose origin is deliberately
on a contact or mounting datum.  In the resolved full-vertical footprint,
`J_CAN` has a local courtyard from approximately `x=-3..13 mm`, while the old
body box was centered at `x=-8..8 mm`.

The scene now derives an origin-preserving visual body envelope from the
footprint's non-courtyard graphics and pad envelopes, with the old centered box
retained as a fallback for minimal proxy footprints.  Courtyard and placement
legality geometry remain unchanged and continue to use the authoritative
footprint courtyard.

## E-004 — moving a component can snap back when a relative distance rule is
violated

**Status: confirmed and fixed for interactive editing.**

`EditorSession.operation("move")` previously called the full placement legality
predicate.  That predicate includes relative distance/alignment rules, so a
temporary drag was rejected even when board bounds, keepouts, package clearance,
source locks, and rigid macros were all legal.

The editor now uses a hard-geometry legality predicate for temporary move and
apply operations.  Relative rules remain strict for automatic placement,
source validation, routing, and manufacturing signoff.  A temporary pose that
breaks one is accepted as an inspection preview and reports:

* the violated rule, actual measurement, requested limit/tolerance, and excess;
* all participating component references;
* an orange warning outline/marker on each participating component; and
* a warning panel in the editor.

This keeps the source-authoritative workflow intact: saving a source edit still
requires the source placement to satisfy all represented constraints.

## E-005 — wheel zoom and right-click pan

**Status: verified; not a defect.**

The editor already binds the wheel to cursor-anchored zoom and a right-button
drag to pan.  The browser smoke test verifies both interactions, including that
right-dragging a footprint does not move it.  No code change was necessary.

## E-006 — selecting and moving a component group

**Status: requested and implemented.**

The editor now supports Shift-click selection.  A normal click-drag on any
highlighted component moves the complete selection by one snapped translation;
arrow movement uses the same group operation.  Rigid hard-macro members are
included automatically.  The server validates the group as one transaction,
rejecting unknown, source-position-locked, temporarily locked, or physically
illegal members without partially changing the session.  The existing preview /
apply behavior is preserved for preview drags.
