# Shared Copper logo

The Copper projects use one **Cu** mark, based on CopperReview's original
Georgia bold lettering in a rounded badge. Keep the uppercase C and lowercase u, with copper's atomic number 29 set small at the upper left.
Do not add atomic weight, circuit traces or a different lettermark.
Product names remain separate text beside the badge.

The canonical files are `crates/copper-editor/assets/copper-logo.svg` and
`crates/copper-editor/assets/copper-logo.png` in CopperScript. The SVG contains outlined
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

The `copper` binary embeds the canonical files for the browser editor. The VS
Code build (`integrations/vscode/build.cjs`) copies the PNG into its VSIX as the
extension icon. CopperReview vendors a byte-identical SVG and retains its
standalone, dependency-free runtime.

After editing the canonical artwork and updating its PNG export, copy the files
to `site/assets/` (`copper-logo.svg`, `favicon.svg` from the SVG, and
`copper-logo.png`) and to CopperReview by hand, then run:

```console
node scripts/check_site.cjs
```

The site check verifies the site's logo copies against the canonical files. The
former `sync_branding` and social-image rendering scripts were removed with the
Python implementation; `site/assets/social-preview.png` is committed and must be
updated by hand if the badge changes. Rebuild the binary and VSIX when
distributing updated tools. CopperReview's vendored asset ships with its web
package.
