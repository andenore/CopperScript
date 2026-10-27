//! Python's `csv` module with the default `excel` dialect.
//!
//! The writer quotes minimally and ends every row with `"\r\n"` on every
//! platform (files are opened with `newline=""`). The reader follows the
//! `_csv` state machine, non-strict, as `csv.DictReader` uses it.

use indexmap::IndexMap;

/// One field as the writer formats it: `None` writes an empty field.
pub enum Field<'a> {
    Text(&'a str),
    None,
    Bool(bool),
    Int(usize),
}

impl<'a> From<&'a str> for Field<'a> {
    fn from(text: &'a str) -> Self {
        Field::Text(text)
    }
}

impl<'a> From<Option<&'a str>> for Field<'a> {
    fn from(text: Option<&'a str>) -> Self {
        text.map_or(Field::None, Field::Text)
    }
}

/// `csv.writer(stream).writerow(fields)`.
pub fn write_row(out: &mut String, fields: &[Field]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let text = match field {
            Field::Text(text) => (*text).to_owned(),
            Field::None => String::new(),
            Field::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
            Field::Int(value) => value.to_string(),
        };
        // A row of one empty field is quoted so that it is not a blank line.
        let lone_empty = fields.len() == 1 && text.is_empty() && !matches!(field, Field::None);
        if lone_empty || text.contains([',', '"', '\r', '\n']) {
            out.push('"');
            out.push_str(&text.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(&text);
        }
    }
    out.push_str("\r\n");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

/// `list(csv.reader(stream))` for a file opened with `newline=""`: records
/// end at `\r\n`, `\r` or `\n` outside quotes, and a blank line is an
/// empty record.
pub fn read_records(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut state = State::StartRecord;
    for line in lines(text) {
        for character in line.chars() {
            state = step(state, character, &mut field, &mut fields);
        }
        state = end_line(state, &mut field, &mut fields);
        if state == State::StartRecord {
            records.push(std::mem::take(&mut fields));
        }
    }
    // At the end of input only an open quoted field still holds a record.
    if state == State::InQuotedField {
        fields.push(field);
        records.push(fields);
    }
    records
}

/// The stream's lines with their endings, split like universal newlines.
fn lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        result.push(&text[start..end]);
        start = end;
        index = end;
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}

fn step(state: State, character: char, field: &mut String, fields: &mut Vec<String>) -> State {
    let mut save = |field: &mut String| fields.push(std::mem::take(field));
    match state {
        State::StartRecord | State::StartField => {
            if state == State::StartRecord && matches!(character, '\n' | '\r') {
                return State::EatCrnl;
            }
            match character {
                '\n' | '\r' => {
                    save(field);
                    State::EatCrnl
                }
                '"' => State::InQuotedField,
                ',' => {
                    save(field);
                    State::StartField
                }
                _ => {
                    field.push(character);
                    State::InField
                }
            }
        }
        State::InField => match character {
            '\n' | '\r' => {
                save(field);
                State::EatCrnl
            }
            ',' => {
                save(field);
                State::StartField
            }
            _ => {
                field.push(character);
                State::InField
            }
        },
        State::InQuotedField => {
            if character == '"' {
                State::QuoteInQuotedField
            } else {
                field.push(character);
                State::InQuotedField
            }
        }
        State::QuoteInQuotedField => match character {
            '"' => {
                field.push('"');
                State::InQuotedField
            }
            ',' => {
                save(field);
                State::StartField
            }
            '\n' | '\r' => {
                save(field);
                State::EatCrnl
            }
            _ => {
                field.push(character);
                State::InField
            }
        },
        // Lines split at every newline, so only newline characters reach here.
        State::EatCrnl => State::EatCrnl,
    }
}

/// `parse_process_char(EOL)`.
fn end_line(state: State, field: &mut String, fields: &mut Vec<String>) -> State {
    match state {
        State::StartRecord | State::EatCrnl => State::StartRecord,
        State::StartField | State::InField | State::QuoteInQuotedField => {
            fields.push(std::mem::take(field));
            State::StartRecord
        }
        State::InQuotedField => State::InQuotedField,
    }
}

/// `csv.DictReader` over a file opened with `encoding="utf-8-sig"`.
pub struct DictReader {
    pub fieldnames: Option<Vec<String>>,
    pub rows: Vec<IndexMap<String, Option<String>>>,
}

/// Read `text` (already decoded, without its byte-order mark) like
/// `csv.DictReader`: the first record names the fields, later blank records
/// are skipped, missing fields are `None` and surplus fields are dropped
/// (Python keeps them under the key `None`).
pub fn dict_reader(text: &str) -> DictReader {
    let mut records = read_records(text).into_iter();
    let Some(fieldnames) = records.next() else {
        return DictReader { fieldnames: None, rows: Vec::new() };
    };
    let rows = records
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut row = IndexMap::new();
            for (index, name) in fieldnames.iter().enumerate() {
                row.insert(name.clone(), record.get(index).cloned());
            }
            row
        })
        .collect();
    DictReader { fieldnames: Some(fieldnames), rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_quotes_minimally() {
        let mut out = String::new();
        write_row(&mut out, &[Field::Text("a,b"), Field::None, Field::Text("say \"hi\""), Field::Bool(true)]);
        write_row(&mut out, &[Field::Text("")]);
        assert_eq!(out, "\"a,b\",,\"say \"\"hi\"\"\",True\r\n\"\"\r\n");
    }

    #[test]
    fn reader_handles_quotes_and_newlines() {
        let records = read_records("a,\"b\r\nc\",\"d\"\"e\"\r\n\r\nf\rg\n\"h");
        assert_eq!(
            records,
            vec![
                vec!["a".to_owned(), "b\r\nc".to_owned(), "d\"e".to_owned()],
                vec![],
                vec!["f".to_owned()],
                vec!["g".to_owned()],
                vec!["h".to_owned()],
            ]
        );
    }
}
