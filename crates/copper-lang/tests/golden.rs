//! The parser against outcomes recorded from Python's `pcbir.parser` by
//! `scripts/generate_rust_fixtures.py`: every in-repo source, seeded
//! truncations, deletions and insertions of each, and targeted snippets.
//!
//! A parse is compared through Python's state encoding of the tree (field and
//! class names included); errors by code, message, location, hint and
//! replacement. On a mismatch the Rust encoding is written to
//! `target/parse-oracle/` for a diff against
//! `python scripts/generate_rust_fixtures.py --dump-parse SOURCE`.

use std::path::{Path, PathBuf};

use copper_json::{Dumps, Value as Json};
use copper_lang::syntax::*;
use sha2::{Digest, Sha256};

/// Python's `state_codec.encode(document)`.
fn encode(document: &Document) -> Json {
    copper_lang::syntax_state::document(document).encode()
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn field<'a>(case: &'a Json, key: &str) -> &'a Json {
    case.get(key).unwrap_or_else(|| panic!("{key} in {case:?}"))
}

fn optional_text(value: &Json) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// Python's `str.__getitem__` slicing on code points.
fn mutate(text: &str, mutation: &Json) -> String {
    let Some(parts) = mutation.as_array() else { return text.to_owned() };
    let characters: Vec<char> = text.chars().collect();
    let index = |value: &Json| match value {
        Json::Int(number) => usize::try_from(number.clone()).unwrap(),
        other => panic!("index {other:?}"),
    };
    let at = index(&parts[1]).min(characters.len());
    let (head, tail) = characters.split_at(at);
    let head: String = head.iter().collect();
    match parts[0].as_str().unwrap() {
        "truncate" => head,
        "delete" => format!("{head}{}", tail.iter().skip(1).collect::<String>()),
        "insert" => format!("{head}{}{}", parts[2].as_str().unwrap(), tail.iter().collect::<String>()),
        other => panic!("mutation {other}"),
    }
}

#[test]
fn parser_matches_python_on_every_recorded_case() {
    check_fixture(include_str!("fixtures/parse.json"));
}

/// An extra fixture from `generate_rust_fixtures.py --parse-corpus DIR`, for
/// sources outside this repository (CopperLib, consumer boards).
#[test]
fn parser_matches_python_on_an_external_corpus() {
    if let Ok(path) = std::env::var("COPPER_PARSE_FIXTURE") {
        check_fixture(&std::fs::read_to_string(path).unwrap());
    }
}

fn check_fixture(fixture: &str) {
    let fixture = copper_json::parse(fixture).unwrap();
    let cases = field(&fixture, "cases").as_array().unwrap();
    let mismatch_directory = repository().join("target/parse-oracle");
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (source, filename) = match case.get("source").and_then(Json::as_str) {
            Some(path) => {
                let text = std::fs::read_to_string(repository().join(path)).unwrap().replace("\r\n", "\n");
                (mutate(&text, field(case, "mutation")), path.to_owned())
            }
            None => {
                (field(case, "text").as_str().unwrap().to_owned(), field(case, "filename").as_str().unwrap().to_owned())
            }
        };
        let label =
            format!("case {index} ({filename} {})", Dumps::new().dumps(case.get("mutation").unwrap_or(&Json::Null)));
        match (copper_lang::parse(&source, &filename), case.get("digest"), case.get("error")) {
            (Ok(document), Some(expected), _) => {
                let encoded = encode(&document);
                let body = Dumps::compact().sort_keys(true).dumps(&encoded);
                let digest: String = Sha256::digest(body.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
                if format!("sha256:{digest}") != expected.as_str().unwrap() {
                    std::fs::create_dir_all(&mismatch_directory).unwrap();
                    let path = mismatch_directory.join(format!("case-{index}.rust.json"));
                    std::fs::write(&path, Dumps::new().indent(1).sort_keys(true).dumps(&encoded)).unwrap();
                    failures.push(format!("{label}: tree differs; Rust encoding in {}", path.display()));
                }
            }
            (Err(error), _, Some(expected)) => {
                let got = (
                    error.code.clone(),
                    error.message.clone(),
                    error.location.filename.to_string(),
                    error.location.offset,
                    error.location.line,
                    error.location.column,
                    error.hint.clone(),
                    error.replacement.clone(),
                    error.to_string(),
                );
                let number = |key: &str| match field(expected, key) {
                    Json::Int(number) => usize::try_from(number.clone()).unwrap(),
                    other => panic!("{key} {other:?}"),
                };
                let want = (
                    field(expected, "code").as_str().unwrap().to_owned(),
                    field(expected, "message").as_str().unwrap().to_owned(),
                    field(expected, "filename").as_str().unwrap().to_owned(),
                    number("offset"),
                    number("line"),
                    number("column"),
                    optional_text(field(expected, "hint")),
                    optional_text(field(expected, "replacement")),
                    field(expected, "text").as_str().unwrap().to_owned(),
                );
                if got != want {
                    failures.push(format!("{label}:\n  Rust   {got:?}\n  Python {want:?}"));
                }
            }
            (Ok(_), None, Some(expected)) => failures.push(format!(
                "{label}: Rust parsed, Python failed with {}",
                field(expected, "text").as_str().unwrap()
            )),
            (Err(error), Some(_), _) => failures.push(format!("{label}: Python parsed, Rust failed with {error}")),
            _ => panic!("{label}: fixture case has no outcome"),
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
