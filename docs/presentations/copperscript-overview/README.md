# CopperScript overview deck

A 21-slide introduction to CopperScript: the idea (engineering intent belongs in
source), how the language works, a complete small board and how to build and
route it, the live routing lab, how the design helps an AI agent make good
decisions, the features included today and what the results do not claim.
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
| 10, the coin-cell board | `make EXAMPLE=nrf52 route` (`copper build` of `examples/nrf52_coin_cell`): 19 of 19 nets, 213 tracks, 44 vias, native DRC 0 violations and 0 unconnected, 3.1 s. `assets/nrf52-coin-cell-routed.png` is `copper plot` then `copper render` of that run's board. |
| 11, live routing lab | `site/demo/routing/` built with `scripts/build_demo.cjs` and served locally: the default weave case, 8 of 8 signals, 12 vias, 0 internal DRC findings. |
| 15, `copper check --json` | `examples/invalid_board/board.copper`, two of its findings. |
| 18, analysis | The simulation and EMI figures of the project website (`site/assets/simulation-noise.json`, `site/assets/emi/`) and the antenna figure (`site/assets/antenna-synthesis.json`). |
| 19, asset tracker | The website's recorded showcase (`site/assets/snapshot.json`); `assets/asset-tracker-top.png` is `site/assets/routing-top.svg` rasterized. |

Code on the slides is abridged from the examples named in the speaker notes.
Review the numbers when those examples or the showcase change; the deck makes
no claim beyond them. Routing is experimental, and none of these results is a
hardware, EMC or production qualification.
