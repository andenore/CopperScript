//! An Excellon reader for comparison: decimal coordinates in millimetres or
//! inches, tool diameters from the header, drill hits and `G85` slots.
//! Plating comes from the `TF.FileFunction` comment or the file name.

use indexmap::IndexMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub plated: bool,
    /// Diameter and coordinates in nanometres, Y as in the file (up).
    pub diameter: f64,
    pub start: (f64, f64),
    pub end: Option<(f64, f64)>,
}

fn number(text: &str, unit: f64) -> Result<f64, String> {
    if !text.contains('.') {
        return Err(format!(
            "Excellon coordinate {text:?} has no decimal point; only decimal coordinates are compared"
        ));
    }
    // Millimetres convert exactly; inches through the 25.4 mm factor.
    let nm = crate::board::parse_nm(text).ok_or_else(|| format!("bad Excellon number {text:?}"))?;
    Ok(nm as f64 * unit / 1e6)
}

fn coordinates(text: &str, unit: f64, at: (f64, f64)) -> Result<(f64, f64), String> {
    let (mut x, mut y) = at;
    let mut rest = text;
    while !rest.is_empty() {
        let axis = rest.as_bytes()[0];
        let end = rest[1..].find(['X', 'Y']).map_or(rest.len(), |index| index + 1);
        let value = number(&rest[1..end], unit)?;
        match axis {
            b'X' => x = value,
            b'Y' => y = value,
            _ => return Err(format!("bad Excellon coordinates {text:?}")),
        }
        rest = &rest[end..];
    }
    Ok((x, y))
}

/// The hits of one Excellon file. `name` decides plating when the file has no
/// `TF.FileFunction` comment.
pub fn read(text: &str, name: &str) -> Result<Vec<Hit>, String> {
    let mut plated = !name.to_ascii_uppercase().contains("NPTH");
    let mut unit = 1e6;
    let mut tools: IndexMap<u32, f64> = IndexMap::new();
    let mut tool = None;
    let mut position = (0.0, 0.0);
    let mut hits = Vec::new();
    let mut header = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(comment) = line.strip_prefix(';') {
            if let Some(function) = comment.trim().strip_prefix("#@! TF.FileFunction,") {
                plated = !function.starts_with("NonPlated");
            }
            continue;
        }
        match line {
            "" | "%" | "G90" | "G05" | "FMAT,2" | "M95" => {
                if line == "%" {
                    header = false;
                }
                continue;
            }
            "M48" => {
                header = true;
                continue;
            }
            "M30" | "M00" => break,
            "G91" => return Err("incremental Excellon coordinates are not compared".to_owned()),
            _ => {}
        }
        if line.starts_with("METRIC") {
            unit = 1e6;
            continue;
        }
        if line.starts_with("INCH") {
            unit = 25.4e6;
            continue;
        }
        if let Some(rest) = line.strip_prefix('T') {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let code: u32 = digits.parse().map_err(|_| format!("bad Excellon tool {line:?}"))?;
            let tail = &rest[digits.len()..];
            if let Some(diameter) = tail.strip_prefix('C') {
                let size = diameter.split(|c: char| c.is_ascii_alphabetic()).next().unwrap_or_default();
                tools.insert(code, number(size, unit)?);
            } else if !tail.is_empty() {
                return Err(format!("unsupported Excellon tool definition {line:?}"));
            }
            if !header {
                if code != 0 && !tools.contains_key(&code) {
                    return Err(format!("undefined Excellon tool T{code}"));
                }
                tool = Some(code);
            }
            continue;
        }
        if line.starts_with('X') || line.starts_with('Y') {
            let diameter = *tool.and_then(|code| tools.get(&code)).ok_or("Excellon hit without a tool")?;
            let (first, slot) = match line.split_once("G85") {
                Some((first, second)) => (first, Some(second)),
                None => (line, None),
            };
            let start = coordinates(first, unit, position)?;
            let end = slot.map(|second| coordinates(second, unit, start)).transpose()?;
            position = start;
            hits.push(Hit { plated, diameter, start, end });
            continue;
        }
        if header && (line.starts_with("ICI") || line.starts_with("VER") || line.starts_with("FMAT")) {
            continue;
        }
        return Err(format!("unsupported Excellon line {line:?}"));
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicad_drill_file_with_slot() {
        let text = "M48\n; #@! TF.FileFunction,Plated,1,4,PTH\nFMAT,2\nMETRIC\nT1C0.200\nT2C0.500\n%\nG90\nG05\nT1\nX13.775Y-20.0\nY-21.0\nT2\nX132.95Y-13.0G85X133.55Y-13.0\nG05\nM30\n";
        let hits = read(text, "board-PTH.drl").unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[1].start, (13_775_000.0, -21_000_000.0));
        assert_eq!(hits[2].end, Some((133_550_000.0, -13_000_000.0)));
        assert!(hits.iter().all(|hit| hit.plated));
        assert!(!read("M48\nMETRIC\n%\nM30\n", "board-NPTH.drl").unwrap().iter().any(|hit| hit.plated));
    }
}
