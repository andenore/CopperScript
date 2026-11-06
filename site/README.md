# CopperScript public website

The promotion site lives with the compiler source and is published with GitHub
Pages at `https://andenore.github.io/CopperScript/`. This is the configured target;
adding these files locally does not confirm that the site has been published.

## Plan and implementation

The audience is PCB designers, embedded developers and people exploring hardware
design with LLM agents. The primary action is to try the compiler example; the
secondary action is to inspect a complete, documented board.

- Introduce CopperScript consistently across the three repository READMEs.
- Show actual source, inspectable IR and illustrations rendered from native plots rather than
  invented renders or unsupported performance claims.
- Explain language intent, hard macros, antenna synthesis, the shared
  workflow, the tracker, quick start, practical questions and repository
  ownership in that order.
- Use static HTML/CSS and small progressive enhancements. No framework,
  installation, external fonts, analytics or board build is needed to publish.
- Add descriptive page metadata, a canonical URL, PNG share image and sitemap.
- Validate local paths/anchors and recorded asset hashes in pull requests; deploy
  only the `site/` directory from `main` after the check passes.

## Preview and check

The live prototype at `demo/` runs the real Rust compiler in a browser worker.
Build its generated assets before previewing it; see [demo setup and scope](demo/README.md).
The landing page itself still needs only static files. The Pages workflow builds
and tests the pinned WASM module before uploading the site. Generated WASM and
bindings under `demo/pkg/` are ignored by Git.

From the CopperScript repository, using Node 22 or newer and any static file
server (for example Python's, if installed):

```console
node scripts/check_site.cjs
python3 -m http.server 8000 --bind 127.0.0.1 --directory site
```

Open `http://127.0.0.1:8000/`. Check both a wide and a narrow viewport and the
five board-view links, including the combined six-layer view with fills hidden.
The hero stacks the top, In2 and bottom showcase plots; hovering widens the
stack. In the antenna section, switch between the template and the synthesized
design and hover the return-loss chart.
Package pads are highlighted gold in the surface, placement and combined views.
The full-size SVG link follows the selected view. The site also works without JavaScript: the views open
their SVG files, the quick-start text remains selectable and FAQs use native
details elements. Stop the server with Ctrl+C.

## Publish on GitHub

1. Commit and push the reviewed website files, scripts, workflow and README
   updates to their respective repositories. Include the tracker documentation
   referenced by the website; do not include unrelated local changes.
2. In **CopperScript → Settings → Pages → Build and deployment**, select
   **GitHub Actions** as the source.
3. Run **Actions → Publish CopperScript website → Run workflow** on `main`
   if the initial push happened before Pages was enabled. Later website pushes
   to `main` deploy automatically. Pull requests only validate the site.
4. Confirm the deployment's reported URL and open it before announcing the site.
5. Add that URL to the **About → Website** field of all three repositories.

The workflow follows [GitHub's custom Pages workflow](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages).
Its deploy job has Pages/OIDC permissions and uses the `github-pages`
environment. Board compilation and dependency pins are separate and unchanged. Only public site files are uploaded.

## Repository descriptions and topics

Suggested About metadata, to apply in GitHub's repository settings:

| Repository | Description | Suggested topics |
| --- | --- | --- |
| CopperScript | Typed PCB design as code: connections, electrical intent, physical constraints and KiCad export. | `pcb-design`, `electronics`, `kicad`, `compiler`, `domain-specific-language`, `hardware-design` |
| CopperLib | Reusable CopperScript parts, footprints, circuits and engineering contracts with source provenance. | `copperscript`, `pcb-design`, `electronics`, `component-library`, `hardware-design` |
| CopperAssetTracker | Cellular, GNSS and Bluetooth asset tracker designed in CopperScript, with inspectable routing and qualification evidence. | `copperscript`, `pcb-design`, `asset-tracking`, `gnss`, `bluetooth`, `embedded` |

`assets/social-preview.png` can also be uploaded as CopperScript's repository
social preview. Metadata settings are account configuration, not Git-tracked
files, and are not applied by this workflow.

The share image is committed so deployment needs no image tools. Its renderer
script was removed with the Python implementation; update the PNG by hand if
the design changes.

## Discovery and launch

After deployment, verify the URL-prefix property in Google Search Console using
the supplied HTML verification file or meta tag. Add the site to Bing Webmaster
Tools and submit `https://andenore.github.io/CopperScript/sitemap.xml` to both.
These account verification steps require the owner's accounts.

GitHub serves the Pages project under `/CopperScript/`; a `robots.txt` there
cannot control crawlers for `andenore.github.io`. Robots rules are read from the
host root. If you later use an account-level Pages site or a custom domain, manage
the root rules there and check the actual crawler access. `llms.txt` is not
required for this site or a promised ranking mechanism.

Publish a short demonstration with a link to this site and ask readers to try
the small example. Suggested announcement:

> I’m building CopperScript, a typed language and compiler for PCB design as
> code. It describes connections, electrical intent and physical constraints,
> with inspectable outputs and KiCad export. The site shows a six-layer cellular
> asset-tracker review candidate built with the shared experimental routing
> workflow. Routing closure is documented; hardware qualification remains open.
> I’d welcome feedback from people trying the language and library workflow.

Link the verified deployed URL with the announcement. Share where project
posts are welcome. No posts, account settings or search submissions are sent
automatically. Track indexed pages, search queries, GitHub referrals and example
feedback; search-enabled LLM answers are a useful spot check, not a stable score.

## Maintain the antenna figure

`assets/antenna-synthesis.json` holds the antenna section's data: the
fine-mesh return-loss curves, matching networks and efficiencies of
`examples/antenna_synthesis` with and without its synthesized `geometry`,
and the copper of both boards after native routing and fill. Nothing in it is
drawn by hand or simulated by the site. `check_site.cjs` fails if the
synthesized geometry or `tuned_for` digest differs from the example's
declaration, so re-run synthesis and refresh the figure together.

From the repository root, with a release `copper` (each verify run takes a
few minutes on the fine mesh):

```console
copper=target/release/copper; out=build/promo
mkdir -p $out && rm -rf $out/template-src && cp -r examples/antenna_synthesis $out/template-src
sed -i '/geometry = "/d; /tuned_for = /d' $out/template-src/board.copper
$copper synthesize-antenna examples/antenna_synthesis/board.copper --verify --report $out/antenna-verify.json --locked --offline
$copper synthesize-antenna $out/template-src/board.copper --verify --report $out/antenna-template.json --locked --offline
$copper route-board examples/antenna_synthesis/board.copper -o $out/synth.kicad_pcb --report $out/synth-route.json --locked --offline
$copper route-board $out/template-src/board.copper -o $out/template.kicad_pcb --report $out/template-route.json --locked --offline
$copper fill $out/synth.kicad_pcb -o $out/synth.filled.kicad_pcb
$copper fill $out/template.kicad_pcb -o $out/template.filled.kicad_pcb
node scripts/update_site_antenna.cjs \
  --template-report $out/antenna-template.json --template-pcb $out/template.filled.kicad_pcb \
  --synthesized-report $out/antenna-verify.json --synthesized-pcb $out/synth.filled.kicad_pcb
node scripts/check_site.cjs
```

The JSON records the SHA-256 of each input. Review the resonance, band and
efficiency wording in `index.html` against the new numbers; the stat tiles are
filled from the JSON, the prose is not.

## Maintain the CopperReview screenshots

`assets/review/` holds four 1600 × 1000 JPEG screenshots (quality 82) of
CopperReview with the showcase board and its compiled IR: Layout, the
Electrical schematic with `MODEM_UART` selected, 3D (Realistic, then 3D) and
Assembly with the tracker's `assembly.lock`. They are captures of the real
viewer, not mock-ups. Retake them when the showcase or CopperReview's interface
changes: from a `make showcase` run, copy `filled.kicad_pcb` to
`CopperAssetTracker.kicad_pcb` beside it (the viewer titles the board by its
file name), serve it with `python -m copper_review BOARD --stage fully_routed`
from CopperReview, open the run's `board.json` and the tracker's
`assembly.lock` through **Open → File…**, and capture each mode at a 1600 × 1000
viewport.

## Maintain the recorded showcase

The five tracker SVGs, two IR excerpts, `rendering.json` and `snapshot.json` are byte-for-byte copies from
CopperAssetTracker's `docs/showcase/`, made by its `make showcase`
(`docs/showcase.md` there). The current illustrations are the
`native-showcase-20261106` routing review, built without KiCad: the current
source routed with native plane verification, then `copper fill`, `copper drc
--cam` (zero violations, zero unconnected, CAM read-back clean) and `copper
plot`. The snapshot binds the tracker commit and inputs, the compiler commit and
executable, the route, DRC and read-back results and every asset by SHA-256.
Native DRC lists the checks it does not make; 12 electrical-requirement checks
remain blocked; no independent CAM or manufacturing export was made, and
`qualified_release` is false. The site links to current board status separately.

Every view is drawn by `copper render` from the plotted Gerbers, viewed from
above without mirroring. Gold is copper inside the solder-mask openings
(exposed copper); the placement view shows the front silkscreen and the
openings themselves. The combined view draws all six copper layers from a plot
of the routed board before zone filling. `rendering.json` records each view's
plot and layer plates.

Refresh from a regenerated, source-matched tracker showcase:

```console
scripts/update_site_showcase.sh --tracker-root ../CopperAssetTracker
node scripts/check_site.cjs
```

Review date, dimensions, counts, candidate scope, image descriptions and
qualification wording in `index.html` and `assets/site.js` whenever the showcase
changes. The copying/checking scripts verify asset identity; they do not rerun
routing or establish hardware qualification. Source excerpts also need review
against the board. Keep reusable algorithms and definitions in their owners.

When moving to a custom domain, update canonical, Open Graph, structured-data and
share-image URLs in `index.html`, the sitemap URL and `BASE_URL` in the checker.
Update repository Website settings and search properties too.
