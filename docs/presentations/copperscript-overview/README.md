# CopperScript overview deck

A 31-slide introduction to CopperScript: the idea (engineering intent belongs in
source), how the language works (dependencies imported by URL, library context,
peripheral selection, modules, differential pairs and skew matching for a
four-lane CSI-2 link and serpentine length tuning, antenna synthesis, vendor
models, build settings and panels), how to build and route a board and find the
cause of a routing failure with copper issues and copper view, how CopperScript
works with KiCad, how the design helps an AI agent make good decisions (compared
with GUI EDA such as KiCad and Altium and with netlist-as-code tools, and mapped
to the mistakes agents typically make), the features included today and what the
results do not claim. Language features are shown with the schematic or layout
they produce. Every slide has speaker notes.

- [copperscript-overview.pdf](copperscript-overview.pdf): one slide per page,
  readable on GitHub.
- [index.html](index.html): open it in a browser. Arrow keys or a click move
  between slides, N shows the speaker notes, `#5` in the URL opens page 5, and
  printing gives one slide per page. Fonts load from Google Fonts; offline, the
  page falls back to Arial and Courier New.

## Sources

`deck.json` holds the title, slide order, outline sections and font links;
`deck.css` the shared styles; `slides/ID.html` one `<section>` per slide, its
speaker notes in the closing `<aside>`; `assets/` the pictures; `sources/` the
two small fixture boards the figures of pages 11 and 12 were made from (not
circuits). The slides use the 1920 × 1080 slide format of Claude's Slides
artifacts, where the deck was first written, so the same files can be published
there again.

`index.html` and the PDF are generated. After changing a source, rebuild both
from the repository root:

```console
node scripts/build_presentation.cjs docs/presentations/copperscript-overview --pdf
```

The PDF uses the pre-installed Playwright and Chromium; without `--pdf` only
`index.html` is written.

## Where the numbers come from

Every figure is from the repository or a run of the current compiler on
23 November 2026, not invented:

| Page | Source |
| --- | --- |
| 5, facts and relationships | `examples/simulation/preamp` (part abridged); `assets/schematic-decoupling.png` is CopperReview's schematic of its compiled IR (`copper compile`, then `python -m copper_review --electrical-ir`) with C_VP selected. |
| 6, physical intent | `examples/mechanical_curves`, routed with `route-board`, then `copper fill`, `copper plot` and `copper render`; the slot and hole in `assets/mechanics.png` are drawn at the coordinates of the plotted NPTH drill file, which `copper render` does not draw. |
| 7, dependencies | `examples/nrf52_coin_cell/board.copper` and its `copper.mod` (the import lines only). |
| 8, library context | `copper library search "coin cell holder" --project examples/nrf52_coin_cell`, trimmed. |
| 9, peripheral selection | `examples/full_vertical` (`configure U_MCU.I2C2`) and CopperLib's STM32G0C1 device (`mux PA6: I2C2.SDA`, AF8); `assets/buses-peripherals.png` is CopperReview's bus view of its IR with MOTION_I2C selected. |
| 10, modules and power states | `examples/simulation/impedance_analyzer`; `assets/power-states.png` is CopperReview's power view of its IR with SLEEP selected and VP picked. |
| 11, CSI-2 | The constraint syntax of the language reference's signal-integrity routing intent; `copper si-check` on `sources/csi2-four-lane/board.copper` (clock and four data pairs, a six-layer stack-up): every pair 99.7 ohm differential and 51.9 ohm single-ended. Screened, not routed. |
| 12, serpentine length matching | `route-board` of `sources/serpentine-lanes/board.copper` (two pairs, 28 and 24 mm), with and without its `length_match` line: group skew 4.000 mm to 0, a 4-leg serpentine with legs up to 0.914 mm, native DRC passed; both pictures are `copper render` of the native plots. |
| 13, antenna synthesis | `examples/antenna_synthesis` and the website's antenna figure (`site/assets/antenna-synthesis.json`); `assets/antenna-synthesis.png` is a screenshot of that figure. |
| 14, vendor models | `examples/simulation/vendor_models/parts/opa810_vendor.copper` and the 3D-model fields of `docs/3d-models.md`. |
| 15, panels | `examples/panel`, built with the commands of its README (two `route-board` runs, `panelize`, `fill`, `drc`: 0 violations), then `copper plot` and `copper render` (`assets/panel.png`). |
| 18, finding the cause | `copper issues` and `copper view --issue X1` on a failed `route-board` run of a four-lane CSI-2 test board (two CopperLib `csi2-dphy-x4-cabline-ca` ports), abridged; `assets/copper-view-escape.png` is that view rasterized. That board is not stored. |
| 25, `copper check --json` | `examples/invalid_board/board.copper`, two of its findings. |
| 28, analysis | The simulation and EMI figures of the project website (`site/assets/simulation-noise.json`, `site/assets/emi/`) and the antenna figure (`site/assets/antenna-synthesis.json`). |
| 29, asset tracker | The website's recorded showcase (`site/assets/snapshot.json`); `assets/asset-tracker-top.png` is `site/assets/routing-top.svg` rasterized. |

The comparison on page 21 is a simplified summary by approach, not a feature audit of
each tool; its speaker notes say so.

Code on the slides is abridged from the examples named in the speaker notes.
Review the numbers when those examples or the showcase change; the deck makes
no claim beyond them. Routing is experimental, and none of these results is a
hardware, EMC or production qualification.
