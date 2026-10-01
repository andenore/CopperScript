# Routing review after the first implementation pass

Reviewed 2026-10-01. Implementation commit: `8b8c896` (R1–R4).
Work list: [routing-review-todo.md](routing-review-todo.md).

## Outcome

The destructive fanout-cleanup regression is addressed: independent filled-zone
KiCad 10.0.6 DRC finds all 57 ordinary nets connected, no dangling tracks, and
three remaining GND unconnected items. The damaged baseline had 74 unconnected
items and 68 dangling tracks. Eight footprint-library findings remain unchanged;
neither baseline nor rerun reports copper shorts or track-clearance violations.
All 277 tests pass, including eight new routing-review regressions.

This is not a finished board. The earlier substantially connected board had
only one GND unconnected item. No critical USB/RF profiles are enabled in this
run, and no manufacturing or return-path signoff is implied.

## Implemented

- R1: Subset fanout cleanup is scoped to attempted nets. Unaffected copper,
  failed-subset input escapes, and input vias without explicit ownership survive.
- R2: Local repair checks unaffected copper identity and fresh native opens;
  committed metrics are rebuilt instead of inheriting stale connected flags.
  Placement scoring also counts actual native ordinary-net opens.
- R3: Physical 45-degree successors survive inserted nonuniform coordinates.
  Exact clearance remains mandatory. Follow-up R3b tracks overly conservative
  index-space obstacle sampling; near-obstacle invariance is not yet complete.
- R4: Global and detailed distance/layer/heading costs use a common physical
  length reference. Via and bend events remain separately weighted.

These are algorithm changes; no routes were manually drawn or moved.

## Matched-run provenance

The board is the same 100 x 80 mm full-vertical example, candidate-01,
six-layer JLCPCB profile, 1 mm detailed pitch, two passes, 20,000-state base
search budget, 10x bounded failed-net repair, fanout, soft rip-up,
constrained-pins-first, progressive guides, and qualified GND via-in-pad.
Four bounded full placement/zone trials ran, with six local transactions allowed.
No source-board constraints or critical profiles were changed.

The original artifact did not save every CLI option; documented settings were
reconstructed, not claimed to be a bit-for-bit replay. Final placement and
rotations match the damaged baseline exactly. Relative to the earlier connected
board, only U_CC differs: its x position is 53.0 rather than 52.5 mm.
All four new full placement trials were rejected by native signal-open checks;
their copper was not committed. The complete CLI run took about 45 minutes and
returned a failing/draft result, correctly preventing production-ready status.

The long process started before the final report-only metric adjustment in the
commit (exclude plane copper from detailed totals and use hypotenuse lengths).
It used the committed search, cleanup and acceptance changes. Measurements here
are independent post-export geometry and KiCad DRC, not inferred from those totals.

Artifacts are retained outside the repository at:
`C:/Users/anden/Documents/Codex/2026-09-17/referenced-chatgpt-conversation-this-is-an/outputs/routing-review-pass1-2026-10-01/`.
They include `provenance.md`, the routed draft, a separately refilled
`latest.kicad_pcb`/project, raw `latest-drc.json`, extracted geometry,
`layer-analysis.json`, six layer images, a mosaic, central zooms and a U_CC zoom.
Original board artifacts and user-owned repository drafts were left untouched.

## Repeated six-layer review

Lengths exclude GND. Red boxes mark the busiest 5 mm tiles by clipped signal
centreline length: this is a density proxy, not verified capacity overflow.
Pads in the diagnostic renderer are rectangular approximations.

| Layer | Earlier connected board | Damaged baseline | Rerun | Observations and next algorithm action |
| --- | ---: | ---: | ---: | --- |
| F.Cu | 695.97 mm | 522.04 mm | 645.57 mm | Escapes are restored. Densest tiles are x35–40/y30–35 (22.83 mm, six nets) around modem translation, and x45–50/y40–45 (20.23 mm, nine nets) around the MCU. Preserve joint package exits; improve these channels, not arbitrary board-wide spreading. |
| In1.Cu | 0 | 0 | 0 | Dedicated GND plane remains free of ordinary tracks. One refilled polygon does not prove all pads contact it; U_CC grounds remain open. |
| In2.Cu | 425.47 mm | 604.66 mm | 551.32 mm | Still the dominant signal layer. Horizontal length is 262.14 mm; 45-degree length is 153.09 mm. Busiest tile x50–55/y15–20 contains NRF_RF_ANT/V3V8/V5. Reference-aware eligibility and clearance-expanded demand feedback are still needed (R6). |
| In3.Cu | 274.63 mm | 71.90 mm | 210.13 mm | More routing than on the damaged baseline, but still sparse. The x45–65/y30–35 channel is mostly MCU_NRF_RX/MODEM_EN with USB_C_CURRENT_1. Horizontal length (92.63 mm) exceeds vertical (66.59 mm); enabled heading preferences do not guarantee which fallback policy produced the final route. |
| In4.Cu | 128.63 mm | 198.99 mm | 143.74 mm | Sparse, predominantly horizontal, with CAN_RX/GNSS_RF sharing the x50–65/y45–50 corridor. Alternate-guide/layer improvement should consider eligible ordinary nets without moving RF blindly. |
| B.Cu | 160.05 mm | 108.56 mm | 96.84 mm | Sparse; biggest local channel x40–45/y40–45 has ACCEL_INT/MODEM_PWRKEY_CTL/USER_BUTTON. Some useful diagonals, but remaining right-angle chains need branch-safe cleanup. |

Signal copper totals about 1,647.6 mm versus 1,684.75 mm on the earlier connected
board. All-net vias are 161 versus 180; degree-two 90-degree turns are 99 versus
126, and 45-degree turns are 135 versus 119. Branches and interlayer turns are
excluded. This is not an overall board-quality win yet: final ground closure
is worse than the earlier best, and its placement differs slightly. Decreased
length or via count on the damaged baseline is never credited as optimization.

Reported escaped pads without an F.Cu pad-centre track endpoint fall from 71
to zero. That proxy alone is not connectivity evidence; KiCad independently
confirms ordinary-net closure. Front GND copper is 191.24 mm and includes
same-number shield bridges as well as short via escapes, not just GND maze routes.

## Remaining findings and priority

### R10: native connectivity is endpoint-based, not exact copper contact

The native report flags V3V3, V3V8, MODEM_EN and PWR/MODEM_FB as open, while
the independently refilled KiCad board has no ordinary-net opens. Reviewing
`pcbir/drc.py::_check_connectivity` explains a demonstrated defect: tracks union
their endpoints but are not unioned at interior T/cross contacts; pads are tested
at centres rather than their full copper shape. Via/track overlap is similarly
not a general shape-contact graph. The message claiming "exact copper" is too
strong for this implementation.

`reproduce_native_connectivity.py` in the artifact directory proves a connected
three-pad interior T junction is reported open. These conservative false opens
can obstruct feedback acceptance. The rejected full-board trials were not
independently exported/checked, so this does not prove that they were valid.
Next implement a layer-aware copper-contact graph using the existing exact
shape predicates/spatial indexing and explicit physical via spans. Add T/cross,
pad-edge, via-contact and real-gap tests; keep open checks enabled. Zone contacts
still require independent fill evidence. Do not waive or globally loosen DRC.

### R11: the actual remaining ground bottleneck moved to U_CC

KiCad's three GND unconnected items involve U_CC pads 3, 10 and 11 at
(52.175, 28.4), (53.825, 29.2), and (53.825, 28.8) mm. J_SIM.SH is connected
on this rerun. U_CC's three logical ground-connected pins should be planned as
a joint package-access group before ordinary routes occupy their exits, with
bounded changes to the component/decoupler cluster if needed. Via-in-pad
permission does not remove drill, foreign-pad clearance or fabrication limits.
Fix R10 before treating native trial rejection as definitive routing evidence.

### R5–R8: route quality and critical integration remain unfinished

MCU_MODEM_TX is 29.51 mm against 8.08 mm pad separation (3.65x). MODEM_STATUS
is 67.38 mm against an 18.50 mm Euclidean pad-MST baseline (3.64x), and
USB_C_VBUS_DET is 12.40 mm against 3.41 mm (3.64x). These identify improvement
candidates, not proof that an unobstructed shorter legal path exists.
Progressive search still returns the first feasible guide. High-fanout V3V3 and
fallback GNSS_RF/USER_LED_2 use orthogonal mode; branch-safe 45-degree cleanup
is still missing. Global overflow remains zero despite local escape problems;
resource counts do not yet represent clearance-expanded detailed demand.

The critical route list is empty. USB pairs and RF are still ordinary independent
nets, so visual route closure is not USB/RF qualification. Explicit paired/critical
profiles and repair protection must precede aggressive shape optimization.

### R3b and R9: reachability and runtime follow-ups

An off-ray blocked node on a nonuniform grid can conservatively suppress a
legal diagonal through index-space supercover sampling. Replace it with physical
outline/keepout sampling while retaining exact track-clearance tests.
A read-only 15-second profile collected 734 samples during zone feedback:
96.6% include search, 27.4% via legality, 13.1% track legality, 12.8% neighbor
generation. Categories overlap and are not whole-run timings. Cache equivalent
physical-span via queries within an immutable search; add phase timings and
checkpoints. Localize/report neutral fallback rather than repeatedly rebuilding
the whole board. No unchecked performance shortcut was introduced in this pass.

## Next implementation order

1. R10 exact contact connectivity plus differential KiCad regression checks.
2. R3b physical obstacle sampling and R11 joint U_CC ground access.
3. R8 explicit USB/RF integration, then R6 demand feedback/local fallback and
   R5 bounded alternate-guide improvement.
4. R7 branch-safe tree cleanup, with R9 runtime instrumentation/cache work.
5. Repeat independent filled-zone DRC, layer measurements and manufacturing
   qualification. Retain failing/draft status until every required gate passes.
