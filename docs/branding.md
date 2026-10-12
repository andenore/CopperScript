# Shared Copper logo

The Copper projects use one **Cu** mark, based on CopperReview's original
Georgia bold lettering in a rounded badge. Keep the uppercase C and lowercase u, with copper's atomic number 29 set small at the upper left.
Do not add atomic weight, circuit traces or a different lettermark.
Product names remain separate text beside the badge.

The canonical files are `pcbir/editor/assets/copper-logo.svg` and
`pcbir/editor/assets/copper-logo.png` in CopperScript. The SVG contains outlined
lettering, so no installed font or web-font request is needed. The transparent
256 × 256 PNG is the raster export used for VS Code's extension icon, mobile
bookmark icon and the social image. Preserve the geometry, proportions and
palette in both formats:

| Element | Color |
| --- | --- |
| Cu lettering | `#ecab78` |
| Badge background | `#312a23` |
| Badge border | `#6b4d34` |

Use the badge at 42 × 42 CSS pixels in tool and website headers. Scale the whole
asset uniformly; do not recreate the letters with CSS or change the colors for
individual tools. A nearby product name supplies context; images use `alt="Cu"`.

The Python wheel bundles the canonical files. VS Code stages these same files
into its VSIX and allows only local images in its webview policy. CopperReview
vendors a byte-identical SVG and retains its standalone, dependency-free runtime.

After editing the canonical artwork and updating its PNG export, run:

```console
python scripts/sync_branding.py --review-root ../CopperReview
python scripts/sync_branding.py --check --review-root ../CopperReview
python scripts/render_site_social.py
python scripts/check_site.py
```

Omit `--review-root` when working without a CopperReview checkout. The site check
also verifies its local logo copies. Social-image rendering uses Pillow and the
system fonts described in `scripts/render_site_social.py`; logo copying and all
runtime uses require no extra packages. Rebuild the wheel and VSIX when
distributing updated tools. CopperReview's vendored asset ships with its web
package.
