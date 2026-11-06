//! Readers for the IPC-D-356 netlist and the component position CSV, as
//! comparable records.

use indexmap::IndexMap;

/// One IPC-D-356 test record with net aliases resolved and names upper-cased.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Record {
    pub code: String,
    pub net: String,
    pub reference: String,
    pub pin: String,
    pub midpoint: bool,
    /// `D` field: drill in 0.0001 in and `P`/`U`.
    pub hole: String,
    /// Access, location, size, rotation and mask fields, as written.
    pub fields: Vec<String>,
}

fn columns(line: &str, from: usize, to: usize) -> String {
    line.chars().skip(from).take(to - from).collect::<String>().trim().to_owned()
}

pub fn read_ipc356(text: &str) -> Result<Vec<Record>, String> {
    let mut aliases: IndexMap<String, String> = IndexMap::new();
    let mut records = Vec::new();
    for line in text.lines() {
        // `P  NNAME1 FULL_NAME` declares the alias NNAME1; KiCad writes
        // `P  NNAMEM0000 FULL_NAME` and uses M0000 in its records.
        if let Some(rest) = line.strip_prefix("P  NNAME") {
            let (token, name) = rest.trim_start().split_once(char::is_whitespace).unwrap_or((rest, ""));
            let name = name.trim().to_ascii_uppercase();
            aliases.insert(format!("NNAME{token}"), name.clone());
            aliases.insert(token.to_owned(), name);
            continue;
        }
        let code = columns(line, 0, 3);
        if !matches!(code.as_str(), "317" | "327" | "367" | "307" | "027") {
            continue;
        }
        let net = columns(line, 3, 17);
        let tail: String = line.chars().skip(32).collect();
        let (hole, rest) = if tail.starts_with('D') && tail.len() >= 6 {
            (tail[..6].to_owned(), &tail[6..])
        } else {
            (String::new(), tail.trim_start())
        };
        // Split the remaining fields at each field letter.
        let mut fields = Vec::new();
        let mut current = String::new();
        for c in rest.chars().filter(|c| !c.is_whitespace()) {
            if c.is_ascii_alphabetic() && !current.is_empty() {
                fields.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
        if !current.is_empty() {
            fields.push(current);
        }
        records.push(Record {
            code,
            net: aliases.get(&net).cloned().unwrap_or_else(|| net.to_ascii_uppercase()),
            reference: columns(line, 20, 26),
            pin: columns(line, 27, 31),
            midpoint: line.chars().nth(31) == Some('M'),
            hole,
            fields,
        });
    }
    records.sort();
    Ok(records)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub value: String,
    pub package: String,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub side: String,
}

fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Positions by reference from a `Ref,Val,Package,PosX,PosY,Rot,Side` CSV.
pub fn read_positions(text: &str) -> Result<IndexMap<String, Position>, String> {
    let mut lines = text.lines();
    let header = csv_fields(lines.next().unwrap_or_default());
    if header != ["Ref", "Val", "Package", "PosX", "PosY", "Rot", "Side"] {
        return Err(format!("unexpected position columns {header:?}"));
    }
    let mut positions = IndexMap::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let fields = csv_fields(line);
        let number =
            |index: usize| fields[index].trim().parse::<f64>().map_err(|_| format!("bad position row {line:?}"));
        if fields.len() != 7 {
            return Err(format!("bad position row {line:?}"));
        }
        positions.insert(
            fields[0].clone(),
            Position {
                value: fields[1].clone(),
                package: fields[2].clone(),
                x: number(3)?,
                y: number(4)?,
                rotation: number(5)?,
                side: fields[6].trim().to_owned(),
            },
        );
    }
    Ok(positions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc356_records_resolve_aliases() {
        let smd = format!("327{:<14}   {:<6}-{:<4} {:6}A01X+037795Y-008642X0354Y0315R000 S2", "NNAME1", "Q04", "2", "");
        let via = format!("317{:<14}   {:<6} {:<4}MD0079PA00X+025886Y-010827X0197Y0000R000S3", "GND", "VIA", "");
        let text = format!("P  JOB   board\nP  NNAME1 A_VERY_LONG_NET_NAME_INDEED\n{smd}\n{via}\n999\n");
        let records = read_ipc356(&text).unwrap();
        assert_eq!(records.len(), 2);
        let smd = records.iter().find(|record| record.code == "327").unwrap();
        assert_eq!(smd.net, "A_VERY_LONG_NET_NAME_INDEED");
        assert_eq!((smd.reference.as_str(), smd.pin.as_str(), smd.midpoint), ("Q04", "2", false));
        assert_eq!(smd.fields, ["A01", "X+037795", "Y-008642", "X0354", "Y0315", "R000", "S2"]);
        let via = records.iter().find(|record| record.code == "317").unwrap();
        assert_eq!((via.hole.as_str(), via.midpoint), ("D0079P", true));
        assert_eq!(via.fields.last().map(String::as_str), Some("S3"));
    }

    #[test]
    fn positions_parse_kicad_quoting() {
        let text =
            "Ref,Val,Package,PosX,PosY,Rot,Side\n\"C1\",\"100 nF\",\"C_0603\",126.000000,-42.000000,90.000000,top\n";
        let positions = read_positions(text).unwrap();
        assert_eq!(positions["C1"].value, "100 nF");
        assert_eq!(positions["C1"].y, -42.0);
    }
}
