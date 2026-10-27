//! `copper agent-guide [TOPIC] [--json]`: a port of `pcbir.agent_guide`.
//!
//! The topic text is embedded from `agent_guide_topics.json`, copied from the
//! Python oracle by `scripts/generate_rust_fixtures.py` until cutover.

use std::io::Write;

use copper_json::{Dumps, Object, Value, encode_str, text};
use sha2::{Digest, Sha256};

use crate::args;

const TOPICS_JSON: &str = include_str!("agent_guide_topics.json");

struct Topics {
    schema: String,
    guide_version: Value,
    topics: Vec<(String, Object)>,
}

fn topics() -> Topics {
    let document = copper_json::parse(TOPICS_JSON).expect("embedded guide topics are JSON");
    let field = |key: &str| document.get(key).cloned().unwrap_or_else(|| panic!("guide topics need {key}"));
    let topics = field("topics")
        .as_array()
        .expect("topics is a list")
        .iter()
        .map(|topic| {
            let name = topic.get("name").and_then(Value::as_str).expect("topic name").to_owned();
            let content = topic.get("content").and_then(Value::as_object).expect("topic content").clone();
            (name, content)
        })
        .collect();
    Topics {
        schema: field("schema").as_str().expect("schema").to_owned(),
        guide_version: field("guide_version"),
        topics,
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `pcbir.agent_guide.get_guide(topic)`.
fn guide(topics: &Topics, topic: &str) -> Option<Object> {
    let (_, content) = topics.topics.iter().find(|(name, _)| name == topic)?;
    let digest = sha256_hex(&Dumps::compact().sort_keys(true).dumps(&Value::Object(content.clone())));
    let mut guide = Object::new();
    guide.insert("schema".into(), Value::from(topics.schema.as_str()));
    guide.insert("guide_version".into(), topics.guide_version.clone());
    guide.insert("package_version".into(), Value::from(env!("CARGO_PKG_VERSION")));
    guide.insert("topic".into(), Value::from(topic));
    guide.insert("content_digest".into(), Value::from(format!("sha256:{digest}")));
    guide.insert(
        "topics".into(),
        Value::Array(topics.topics.iter().map(|(name, _)| Value::from(name.as_str())).collect()),
    );
    for (key, value) in content {
        guide.insert(key.clone(), value.clone());
    }
    Some(guide)
}

fn strings(guide: &Object, key: &str) -> Vec<String> {
    guide
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|item| match item {
            Value::Str(text) => text.clone(),
            other => Dumps::new().dumps(other),
        })
        .collect()
}

/// Python's `str.isspace()`: Unicode White_Space plus the separators U+001C..U+001F.
fn python_isspace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// `pcbir.agent_guide.render_guide`.
fn render(guide: &Object, as_json: bool) -> String {
    if as_json {
        return Dumps::compact().sort_keys(true).dumps(&Value::Object(guide.clone()));
    }
    let scalar = |key: &str| match guide.get(key) {
        Some(Value::Str(text)) => text.clone(),
        Some(other) => Dumps::new().dumps(other),
        None => String::new(),
    };
    let mut lines = vec![
        format!(
            "CopperScript agent guide v{} / {} (package {})",
            scalar("guide_version"),
            scalar("topic"),
            scalar("package_version")
        ),
        String::new(),
        scalar("title"),
        String::new(),
    ];
    lines.extend(strings(guide, "guidance").iter().enumerate().map(|(index, item)| format!("{}. {item}", index + 1)));
    lines.push(String::new());
    lines.push("Command templates (replace QUERY, PACKAGE::SYMBOL and paths):".to_owned());
    for command in guide.get("commands").and_then(Value::as_array).unwrap_or(&[]) {
        let words: Vec<String> = command
            .as_array()
            .unwrap_or(&[])
            .iter()
            .map(|word| {
                let word = word.as_str().unwrap_or_default();
                if word.chars().any(python_isspace) { encode_str(word, true) } else { word.to_owned() }
            })
            .collect();
        lines.push(words.join(" "));
    }
    for example in strings(guide, "examples") {
        lines.push(String::new());
        lines.push(example);
    }
    lines.push(String::new());
    lines.push(format!("More: copper agent-guide TOPIC; topics: {}", strings(guide, "topics").join(", ")));
    lines.push("JSON: copper agent-guide TOPIC --json".to_owned());
    lines.join("\n")
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let topics = topics();
    let names: Vec<&str> = topics.topics.iter().map(|(name, _)| name.as_str()).collect();
    let usage = format!("usage: copper agent-guide [-h] [--json] [{{{}}}]", names.join(","));
    let parsed = match args::parse(arguments, &["--help", "--json"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{usage}\ncopper agent-guide: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{usage}\n\nread compact, versioned agent guidance; start here");
        return 0;
    }
    let topic = match parsed.positionals.as_slice() {
        [] => "start",
        [topic] => topic.as_str(),
        [_, extra @ ..] => {
            let _ = writeln!(err, "{usage}\ncopper agent-guide: error: unrecognized arguments: {}", extra.join(" "));
            return 2;
        }
    };
    let Some(guide) = guide(&topics, topic) else {
        let _ = writeln!(
            err,
            "{usage}\ncopper agent-guide: error: argument topic: invalid choice: '{topic}' (choose from {})",
            names.join(", ")
        );
        return 2;
    };
    match text::print(out, &render(&guide, parsed.flags.contains(&"--json"))) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_renders_both_forms() {
        let topics = topics();
        assert_eq!(topics.topics.first().map(|(name, _)| name.as_str()), Some("start"));
        for (name, _) in &topics.topics {
            let guide = guide(&topics, name).unwrap();
            assert!(render(&guide, false).starts_with("CopperScript agent guide v1 / "));
            let json = copper_json::parse(&render(&guide, true)).unwrap();
            assert_eq!(json.get("topic").and_then(Value::as_str), Some(name.as_str()));
        }
    }
}
