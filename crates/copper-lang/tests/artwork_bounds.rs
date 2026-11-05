//! Copper text envelopes against KiCad's rendered stroke text.
//!
//! The ink extents below were measured from `kicad-cli pcb export svg` (KiCad
//! 10.0.6) for centred F.Cu text at 1 mm size and 0.15 mm stroke, relative to
//! the text position: stroke centrelines plus half the stroke width.

use copper_lang::silkscreen::{Artwork, ArtworkMaterial, artwork_bounds};
use copper_physical::{BoardSide, Point};
use copper_units::Decimal;

fn text(text: &str, rotation: &str) -> Artwork {
    Artwork::Text {
        id: "label".to_owned(),
        text: text.to_owned(),
        position: Point::new(10_000_000, 20_000_000),
        size_nm: 1_000_000,
        thickness_nm: 150_000,
        rotation_degrees: Decimal::parse(rotation).expect("literal"),
        side: BoardSide::Front,
        material: ArtworkMaterial::Copper,
        exposed: true,
    }
}

/// `(left, low, right, high)` relative to the text position.
fn relative(item: &Artwork) -> (i64, i64, i64, i64) {
    let (left, low, right, high) = artwork_bounds(item).expect("bounds");
    (left - 10_000_000, low - 20_000_000, right - 10_000_000, high - 20_000_000)
}

#[test]
fn copper_text_envelope_covers_the_rendered_ink_without_the_old_slack() {
    // Rendered: x -4.670 to 4.766, y -0.618 to 0.866 mm.
    let (left, low, right, high) = relative(&text("CopperScript", "0"));
    assert!(left <= -4_670_000 && right >= 4_766_000 && low <= -618_000 && high >= 866_000);
    // Twelve glyphs once reserved (12 + 2) sizes either side.
    assert!(right - left < 10_500_000 && high - low < 2_100_000);
}

#[test]
fn copper_text_envelope_rotates_with_the_text() {
    let (left, low, right, high) = relative(&text("v0.1.0", "90"));
    // Rendered unrotated: x -2.218 to 2.170, y -0.618 to 0.532 mm.
    assert!(low <= -2_218_000 && high >= 2_218_000 && left <= -618_000 && right >= 618_000);
    assert!(high - low < 5_000_000);
}

#[test]
fn copper_text_envelope_grows_with_each_line() {
    // Rendered three lines of "H": y -2.228 to 2.142 mm.
    let (_, low, _, high) = relative(&text("H\nH\nH", "0"));
    assert!(low <= -2_228_000 && high >= 2_228_000);
}
