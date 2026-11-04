//! Stroke text as KiCad plots it: the newstroke glyphs (U+0020 to U+00FF),
//! KiCad's line layout and alignment fudge factors, mirroring about the text
//! anchor and rotation about it. Strokes come out in board coordinates.

use std::sync::OnceLock;

use crate::board::{HAlign, Text, VAlign};
use crate::geometry::rotate_f64;

const GLYPHS: &str = include_str!("../fonts/newstroke-latin.txt");
const SCALE: f64 = 1.0 / 21.0;
/// Moves the glyph origin to the baseline, in font units.
const BASELINE_OFFSET: i32 = -8;
const ITALIC_TILT: f64 = 1.0 / 8.0;
const INTERLINE_PITCH: f64 = 1.68;
/// KiCad's adjustment of the stroke font's interline to its legacy spacing.
const LEGACY_INTERLINE: f64 = 0.9583;

/// A stroke: connected points in board nanometres.
pub type Polyline = Vec<(f64, f64)>;

struct Glyph {
    /// Advance (and bounding-box width) in em.
    width: f64,
    strokes: Vec<Vec<(f64, f64)>>,
}

fn glyphs() -> &'static [Glyph] {
    static FONT: OnceLock<Vec<Glyph>> = OnceLock::new();
    FONT.get_or_init(|| GLYPHS.lines().map(parse_glyph).collect())
}

fn parse_glyph(line: &str) -> Glyph {
    let bytes = line.as_bytes();
    let coordinate = |byte: u8| i32::from(byte) - i32::from(b'R');
    let (start, end) = (f64::from(coordinate(bytes[0])) * SCALE, f64::from(coordinate(bytes[1])) * SCALE);
    let mut strokes: Vec<Vec<(f64, f64)>> = vec![Vec::new()];
    for pair in bytes[2..].chunks_exact(2) {
        if pair == b" R" {
            strokes.push(Vec::new());
            continue;
        }
        let x = f64::from(coordinate(pair[0])) * SCALE - start;
        let y = f64::from(coordinate(pair[1]) + BASELINE_OFFSET) * SCALE;
        strokes.last_mut().expect("a stroke").push((x, y));
    }
    strokes.retain(|stroke| !stroke.is_empty());
    Glyph { width: end - start, strokes }
}

fn glyph(c: char) -> &'static Glyph {
    let font = glyphs();
    let index = (c as u32).checked_sub(32).map(|index| index as usize).filter(|&index| index < font.len());
    &font[index.unwrap_or('?' as usize - 32)]
}

/// KiCad's `KiROUND`: half away from zero.
fn kiround(value: f64) -> i64 {
    value.round() as i64
}

/// The pen width KiCad plots `text` with.
pub fn pen_width(text: &Text) -> i64 {
    let (width, height) = text.size;
    let mut pen = text.thickness;
    if pen <= 1 {
        pen = if text.bold { kiround(width as f64 / 5.0) } else { kiround(width as f64 / 8.0) };
    }
    pen.min(kiround(width.abs().min(height.abs()) as f64 * 0.25))
}

/// Horizontal advance of one line, as KiCad accumulates it.
fn advance(line: &str, width: i64) -> i64 {
    let space = glyphs()[0].width;
    line.chars()
        .map(|c| if c == ' ' { kiround(width as f64 * space) } else { kiround(glyph(c).width * width as f64) })
        .sum()
}

/// The strokes of `text`, each a polyline in board nanometres, and the pen width.
pub fn strokes(text: &Text) -> Result<(Vec<Polyline>, i64), String> {
    if text.text.contains('\t') {
        return Err(format!("text {:?}: tabs are not plotted natively", text.text));
    }
    let pen = pen_width(text);
    let (width, height) = text.size;
    let lines: Vec<&str> = text.text.split('\n').collect();
    let interline = (height as f64 * INTERLINE_PITCH * LEGACY_INTERLINE * text.line_spacing) as i64;
    let mut block_height: i64 = 0;
    for index in 0..lines.len() {
        block_height =
            if index == 0 { (block_height as f64 + height as f64 * 1.17) as i64 } else { block_height + interline };
    }
    // KiCad's offsets to match its 6.0 stroke-text positions.
    let offset_x: i64 = (pen as f64 / 1.52) as i64;
    let mut offset_y: i64 = (height as f64 - pen as f64 * 0.052) as i64;
    match text.v_align {
        VAlign::Top => {}
        VAlign::Center => offset_y -= block_height / 2,
        VAlign::Bottom => offset_y -= block_height,
    }
    let origin = (text.position.x as f64, text.position.y as f64);
    let tilt = if text.italic { ITALIC_TILT } else { 0.0 };
    let mut result = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_width = advance(line, width);
        let x = match text.h_align {
            HAlign::Left => offset_x,
            HAlign::Center => -line_width / 2,
            HAlign::Right => -(line_width + offset_x),
        };
        let mut cursor = (text.position.x + x, text.position.y + offset_y + index as i64 * interline);
        for c in line.chars() {
            if c == ' ' {
                cursor.0 += kiround(width as f64 * glyphs()[0].width);
                continue;
            }
            let glyph = glyph(c);
            for stroke in &glyph.strokes {
                result.push(
                    stroke
                        .iter()
                        .map(|&(gx, gy)| {
                            let (px, py) = (gx * width as f64, gy * height as f64);
                            let mut x = px - py * tilt + cursor.0 as f64;
                            let y = py + cursor.1 as f64;
                            if text.mirrored {
                                x = origin.0 - (x - origin.0);
                            }
                            let (rx, ry) = rotate_f64(x - origin.0, y - origin.1, text.angle);
                            (rx + origin.0, ry + origin.1)
                        })
                        .collect(),
                );
            }
            cursor.0 += kiround(glyph.width * width as f64);
        }
    }
    Ok((result, pen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Layer;
    use crate::geometry::Point;

    fn text(content: &str) -> Text {
        Text {
            layer: Layer::EdgeCuts,
            text: content.to_owned(),
            position: Point::new(0, 0),
            angle: 0.0,
            size: (1_000_000, 1_000_000),
            thickness: 150_000,
            bold: false,
            italic: false,
            mirrored: false,
            h_align: HAlign::Center,
            v_align: VAlign::Center,
            line_spacing: 1.0,
        }
    }

    #[test]
    fn font_covers_basic_latin_and_latin_1() {
        assert_eq!(glyphs().len(), 224);
        // 'H' is two verticals and a bar; its advance includes the spacing.
        assert_eq!(glyph('H').strokes.len(), 3);
        assert!((glyph('H').width - 22.0 / 21.0).abs() < 1e-12);
        assert!(std::ptr::eq(glyph('\u{2603}'), glyph('?')));
    }

    #[test]
    fn centred_text_straddles_its_anchor() {
        let (strokes, pen) = strokes(&text("HH")).unwrap();
        assert_eq!(pen, 150_000);
        let xs: Vec<f64> = strokes.iter().flatten().map(|point| point.0).collect();
        let (min, max) = xs.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(min < 0.0 && max > 0.0 && (min + max).abs() < 300_000.0, "{min} {max}");
        let ys: Vec<f64> = strokes.iter().flatten().map(|point| point.1).collect();
        assert!(ys.iter().all(|&y| y.abs() < 600_000.0));
    }

    #[test]
    fn mirrored_and_rotated_text_turns_about_the_anchor() {
        let plain = strokes(&text("L")).unwrap().0;
        let mut mirrored = text("L");
        mirrored.mirrored = true;
        let flipped = strokes(&mirrored).unwrap().0;
        for (a, b) in plain.iter().flatten().zip(flipped.iter().flatten()) {
            assert!((a.0 + b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
        }
        let mut turned = text("L");
        turned.angle = 90.0;
        let rotated = strokes(&turned).unwrap().0;
        for (a, b) in plain.iter().flatten().zip(rotated.iter().flatten()) {
            assert!((b.0 - a.1).abs() < 1e-6 && (b.1 + a.0).abs() < 1e-6);
        }
    }
}
