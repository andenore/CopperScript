# Stroke font data

`newstroke-latin.txt` holds the first 224 glyphs (U+0020 to U+00FF, one per
line) of the newstroke font, extracted unchanged from KiCad's
`common/newstroke_font.cpp` (KiCad 10.0 branch, file SHA-256
`dd686d654166869bad4866b107c81e538736c057721ddbf70036a50d8e844de9`). Matching
KiCad's own font keeps native silkscreen text identical to KiCad's plots.

Each line is a Hershey-style glyph: two characters for the left and right
bounds, then coordinate pairs offset by `'R'`, with `" R"` lifting the pen.

**Licence status: to be confirmed before release.** Newstroke is by Vladimir
Uryvaev. In 2010 he allowed KiCad to relicense it as CC0 1.0, as reported on
the kicad-developers mailing list (exact message not yet checked); KiCanvas credits it as "Originally licensed under Creative Commons CC0 1.0,
amended with an MIT-like license", plus SIL OFL 1.1 for some glyphs. Those
are the CJK glyphs from Source Han Sans, which are not included here. The
KiCad source file itself carries a GPL-2.0-or-later header. Only the Latin
glyph data is vendored here, no KiCad code.
