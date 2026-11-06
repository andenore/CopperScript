//! Write zone fills into a `.kicad_pcb` text: each top-level zone's
//! `filled_polygon`s are replaced, and everything else in the file is kept
//! byte for byte.

use crate::board::Board;
use crate::fill::{ZoneFill, fractured};
use crate::geometry::{Contour, Edge};

/// Spans `(start, end)` of the direct child lists of the list at `[from, to)`,
/// with their tags. `to` is the index of the list's closing parenthesis.
fn children(text: &str, from: usize, to: usize) -> Vec<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let (mut depth, mut index, mut start) = (0usize, from + 1, 0usize);
    let mut in_string = false;
    while index < to {
        let byte = bytes[index];
        if in_string {
            if byte == b'\\' {
                index += 1;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'(' {
            if depth == 0 {
                start = index;
            }
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                let tag: String =
                    text[start + 1..].chars().take_while(|c| !c.is_whitespace() && *c != '(' && *c != ')').collect();
                result.push((start, index + 1, tag));
            }
        }
        index += 1;
    }
    result
}

/// The index of the closing parenthesis of the list opening at `open`.
fn closing(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut in_string, mut index) = (0usize, false, open);
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if byte == b'\\' {
                index += 1;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// Millimetres with trailing zeros trimmed, as KiCad writes coordinates.
fn mm(nm: i64) -> String {
    let text = crate::gerber::mm(nm);
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" { "0".to_owned() } else { trimmed.to_owned() }
}

fn line_indent(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let rest = &text[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

fn polygon_text(layer: &str, contour: &Contour, indent: &str, unit: &str) -> String {
    let mut points = vec![contour.start];
    points.extend(contour.edges.iter().take(contour.edges.len().saturating_sub(1)).map(Edge::end));
    let mut lines = vec![
        format!("{indent}(filled_polygon"),
        format!("{indent}{unit}(layer \"{layer}\")"),
        format!("{indent}{unit}(pts"),
    ];
    for chunk in points.chunks(4) {
        let row: Vec<String> = chunk.iter().map(|p| format!("(xy {} {})", mm(p.x), mm(p.y))).collect();
        lines.push(format!("{indent}{unit}{unit}{}", row.join(" ")));
    }
    lines.push(format!("{indent}{unit})"));
    lines.push(format!("{indent})"));
    lines.join("\n")
}

/// `text` with every filled zone's fills replaced by `fills`.
pub fn write_fills(text: &str, board: &Board, fills: &[ZoneFill]) -> Result<String, String> {
    let root_open = text.find('(').ok_or("not an s-expression")?;
    let root_close = closing(text, root_open).ok_or("unbalanced board file")?;
    let zones: Vec<(usize, usize)> = children(text, root_open, root_close)
        .into_iter()
        .filter(|(_, _, tag)| tag == "zone")
        .map(|(s, e, _)| (s, e))
        .collect();
    let contours = fractured(fills);
    let filled: Vec<usize> = fills.iter().map(|fill| fill.zone).collect();
    let mut output = String::with_capacity(text.len() * 2);
    let mut cursor = 0;
    for (zone_index, zone) in board.zones.iter().enumerate() {
        if zone.in_footprint || !filled.contains(&zone_index) {
            continue;
        }
        let (start, end) =
            *zones.get(zone.index).ok_or_else(|| format!("zone {:?} not found in the board text", zone.name))?;
        let close = end - 1;
        output.push_str(&text[cursor..start]);
        let kids = children(text, start, close);
        let child_indent = kids.first().map_or("\t\t".to_owned(), |(s, _, _)| line_indent(text, *s).to_owned());
        let unit = if child_indent.contains('\t') { "\t" } else { "  " };
        // Copy the zone without its old fills, marking it filled.
        let mut kept = String::new();
        let mut position = start;
        // Without this flag, KiCad reads fills of files older than format
        // 20250210 as outlines stroked by the minimum width.
        let exact = kids.iter().any(|(s, e, tag)| tag == "filled_areas_thickness" && text[*s..*e].contains(" no"));
        for (s, e, tag) in &kids {
            if tag == "fill" && !exact {
                kept.push_str(&text[position..*s]);
                kept.push_str(
                    "(filled_areas_thickness no)
",
                );
                kept.push_str(&child_indent);
                position = *s;
            }
            if tag == "filled_polygon" || tag == "fill_segments" {
                let line_start = text[..*s].rfind('\n').unwrap_or(*s);
                kept.push_str(&text[position..line_start.max(position)]);
                position = *e;
            } else if tag == "fill" && !text[*s..*e].starts_with("(fill yes") {
                kept.push_str(&text[position..*s]);
                kept.push_str("(fill yes");
                kept.push_str(&text[*s + "(fill".len()..*e]);
                position = *e;
            }
        }
        kept.push_str(&text[position..close]);
        // New fills go on their own lines before the closing parenthesis.
        let trailing = kept.len() - kept.trim_end().len();
        let (head, tail) = kept.split_at(kept.len() - trailing);
        output.push_str(head);
        for (_, layer, contour) in contours.iter().filter(|(z, _, _)| *z == zone_index) {
            output.push('\n');
            output.push_str(&polygon_text(&board.copper_layers[*layer], contour, &child_indent, unit));
        }
        output.push_str(tail);
        output.push(')');
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::parse_board;
    use crate::poly;
    use i_overlay::i_float::int::point::IntPoint;

    const BOARD: &str = "(kicad_pcb (version 20260206)\n  (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal))\n  (net 0 \"\") (net 1 \"GND\")\n  (zone\n    (net 1)\n    (net_name \"GND\")\n    (layer \"F.Cu\")\n    (uuid \"u1\")\n    (name \"z\")\n    (connect_pads yes (clearance 0.2))\n    (min_thickness 0.25)\n    (fill (thermal_gap 0.3))\n    (polygon (pts (xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10)))\n    (filled_polygon (layer \"F.Cu\") (pts (xy 1 1) (xy 2 1) (xy 2 2)))\n  )\n  (gr_text \"keep (me)\" (at 1 1) (layer \"F.SilkS\") (effects (font (size 1 1))))\n)\n";

    #[test]
    fn fills_replace_old_ones_and_keep_the_rest() {
        let board = parse_board(BOARD, "t").unwrap();
        let square: poly::Contour = vec![
            IntPoint::new(1_000_000, 1_000_000),
            IntPoint::new(9_500_000, 1_000_000),
            IntPoint::new(9_500_000, 9_000_000),
            IntPoint::new(1_000_000, 9_000_000),
        ];
        let fills =
            vec![ZoneFill { zone: 0, layer: 0, shapes: vec![vec![poly::positive(square)]], islands_removed: 0 }];
        let written = write_fills(BOARD, &board, &fills).unwrap();
        assert!(!written.contains("(xy 2 2)"), "{written}");
        assert!(written.contains("    (filled_polygon\n      (layer \"F.Cu\")\n      (pts\n        (xy 1 1) (xy 9.5 1) (xy 9.5 9) (xy 1 9)\n      )\n    )\n  )"), "{written}");
        assert!(
            written.contains(
                "(filled_areas_thickness no)
    (fill yes (thermal_gap 0.3))"
            ),
            "{written}"
        );
        assert!(written.contains("(gr_text \"keep (me)\""));
        let reread = parse_board(&written, "t").unwrap();
        assert_eq!(reread.fills.len(), 1);
    }
}
