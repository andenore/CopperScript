# CopperScript public website

The promotion site lives with the compiler source and is published with GitHub
Pages at `https://andenore.github.io/CopperScript/`. This is the configured target;
adding these files locally does not confirm that the site has been published.

## Plan and implementation

The audience is PCB designers, embedded developers and people exploring hardware
design with LLM agents. The primary action is to try the compiler example; the
secondary action is to inspect a complete, documented board.

- Introduce CopperScript consistently across the three repository READMEs.
- Show actual source, inspectable IR and native KiCad illustrations rather than
  invented renders or unsupported performance claims.
- Explain language intent, the shared workflow, the tracker, quick start,
  practical questions and repository ownership in that order.
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

## Maintain the recorded showcase

The five tracker SVGs, two IR excerpts, `rendering.json` and `snapshot.json` are byte-for-byte copies from
CopperAssetTracker's `docs/showcase/`. The current illustrations represent the
`promo-reroute-20261013` fresh source-matched routing review, not a released
board. Its snapshot binds the exact exported PCB, source, native DRC, route
summary, in-language macro binding and rendered-asset hashes. Native DRC is clean, but five check types were
ignored, 13 electrical-requirement checks remain blocked, independent CAM was
not run for this review. No new manufacturing export is claimed, and
supplier/assembly review is incomplete. `qualified_release` stays
false. The site links to current board status separately.

Surface pad geometry is plotted separately by KiCad and overlaid in gold so
ground-connected lands remain visible against a fill. The placement image also
includes those native pads. The combined view shows all six copper layers,
tracks, vias and package outlines, hiding zones and solid copper fill graphics
only in a temporary plotting copy. Overlapping reference/value labels are hidden
in placement and combined views. `rendering.json` records the layers, hidden
geometry counts and hashes of the preserved native pads/tracks. The input PCB,
route and DRC evidence are unchanged. See the tracker's figure-refresh guide for
the exact retained-candidate workflow; historical source status is preserved.

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
