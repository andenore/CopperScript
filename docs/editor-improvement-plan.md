# Mechanical editor improvement plan

The editor should remain a thin inspection and floorplan tool over the
backend-neutral physical IR.  Improvements are therefore ordered from
interaction correctness to diagnostics, while preserving source authority and
keeping manufacturing legality outside the temporary preview path.

## Completed in this iteration

1. **Audit and regression fixtures** — verify each report against source,
   scene serialization, unit tests, and browser smoke coverage.
2. **Separate hard legality from relative intent** — expose
   `placement_solution_is_hard_legal` for editor previews while retaining
   `placement_solution_is_legal` for planners and signoff.
3. **Structured relative diagnostics** — expose deterministic per-rule
   measurements through `relative_placement_violations` and the scene's
   `placement_constraints` object.
4. **Non-blocking interactive movement** — permit a hard-legal temporary move or
   apply, preserve the preview/revision protocol, and keep source-save
   validation strict.
5. **Constraint visualization** — highlight every participating component and
   show a readable warning list and selected-component detail.
6. **Origin-correct footprint rendering** — derive the editor body envelope from
   local graphics/pads for offset-origin connectors; retain a safe proxy
   fallback.
7. **Pad labels** — expose optional electrical pin names, keep labels hidden
   until the corresponding pad is hovered, and size them in pad/user
   coordinates so they fit the land at every zoom level.
8. **Keyboard parity** — add lock/unlock, reverse rotation, side flip, undo,
   redo, and Home-to-fit shortcuts; update the accessibility help text.
9. **Verification** — add unit regressions for offset bodies and relative
   warnings, extend the keyboard browser smoke, run the editor/document suites,
   and validate JavaScript syntax.
10. **Group selection and movement** — Shift-click toggles a multi-selection;
    dragging or using arrow keys translates the selected components atomically.
    The backend validates the complete group, expands rigid macros, and keeps
    source-fixed or temporarily locked members protected.

## Follow-up work

* Add a first-class optional body envelope to the physical IR if backends need
  exact component-body geometry beyond the editor.  The current derived scene
  envelope intentionally avoids changing the physical schema.
* Add keyboard-focusable next/previous component selection and a constraint
  navigator that cycles through warning records.
* Add a visual measurement of the actual selected constraint target pads, not
  only component-level warning markers.
* Add an optional pad-label display preference if a future inspection workflow
  needs labels for an entire package at once; hover remains the uncluttered
  default.
