# CopperScript overview deck

A 26-slide introduction to CopperScript: the idea (engineering intent belongs in
source), how the language works (dependencies imported by URL, library context,
peripheral selection, modules, differential pairs and skew matching for a
four-lane CSI-2 link, antenna synthesis, vendor models, build settings and
panels), how to build and route a board, how the design helps an AI agent make
good decisions, the features included today and what the results do not claim.
Every slide has speaker notes.

- [copperscript-overview.pdf](copperscript-overview.pdf): one slide per page,
  readable on GitHub.
- [index.html](index.html): open it in a browser. Arrow keys or a click move
  between slides, N shows the speaker notes, `#5` in the URL opens page 5, and
  printing gives one slide per page. Fonts load from Google Fonts; offline, the
  page falls back to Arial and Courier New.

## Sources

`deck.json` holds the title, slide order, outline sections and font links;
`deck.css` the shared styles; `slides/ID.html` one `<section>` per slide, its
speaker notes in the closing `<aside>`; `assets/` the pictures. The slides use
the 1920 × 1080 slide format of Claude's Slides artifacts, where the deck was
first written, so the same files can be published there again.

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
| 7, dependencies | `examples/nrf52_coin_cell/board.copper` and its `copper.mod` (the import lines only). |
| 8, library context | `copper library search "coin cell holder" --project examples/nrf52_coin_cell`, trimmed. |
| 11, CSI-2 | The constraint syntax of the language reference's signal-integrity routing intent; `copper si-check` on a four-lane fixture (clock and four data pairs, the same rules, a six-layer stack-up): every pair 99.7 ohm differential and 51.9 ohm single-ended. The fixture was screened, not routed. |
| 12, antenna synthesis | `examples/antenna_synthesis` and the website's antenna figure (`site/assets/antenna-synthesis.json`); `assets/antenna-synthesis.png` is a screenshot of that figure. |
| 13, vendor models | `examples/simulation/vendor_models/parts/opa810_vendor.copper` and the 3D-model fields of `docs/3d-models.md`. |
| 20, `copper check --json` | `examples/invalid_board/board.copper`, two of its findings. |
| 23, analysis | The simulation and EMI figures of the project website (`site/assets/simulation-noise.json`, `site/assets/emi/`) and the antenna figure (`site/assets/antenna-synthesis.json`). |
| 24, asset tracker | The website's recorded showcase (`site/assets/snapshot.json`); `assets/asset-tracker-top.png` is `site/assets/routing-top.svg` rasterized. |

Code on the slides is abridged from the examples named in the speaker notes.
Review the numbers when those examples or the showcase change; the deck makes
no claim beyond them. Routing is experimental, and none of these results is a
hardware, EMC or production qualification.
