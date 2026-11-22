//! Editor support inside benches and `sim` blocks: completion and hover of
//! bench statements, analysis kinds and their options, `sim` block kinds with
//! their roles and parameters, and the bench functions, all from
//! [`copper_lang::sim_reference`] (the table the checkers use); and the "run
//! this bench" code lens.

use std::path::{Path, PathBuf};

use copper_json::Value;
use copper_lang::expression::Function;
use copper_lang::sim_reference::{
    self, ANALYSES, AnalysisKind, BENCH_STATEMENTS, MONTECARLO_KEYS, PROBES, key_meaning, key_unit, kind_meaning,
};
use copper_lang::simulation::{BUILT_IN_KINDS, SimKind};

use crate::text::{Text, int, object};

/// The command of the "run this bench" code lens: arguments `[board file, bench name]`.
pub const RUN_BENCH: &str = "copper.sim.runBench";

/// Where in a bench or `sim` block a position is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Context {
    None,
    /// At the start of a statement of a bench or fixture body.
    Statement,
    /// After `analysis` or `analysis NAME:`.
    AnalysisKind,
    /// Inside `analysis KIND { ... }`.
    Analysis(&'static AnalysisKind),
    /// Inside a `sim KIND`, `replace X with KIND` or `model NAME: KIND` block.
    Block(SimKind),
    /// After `sim`, `replace X with` or `model NAME:`; `built_in` leaves out `spice`.
    KindName {
        built_in: bool,
    },
    /// In the expression of a `measure`, `let` or `plot`.
    Expression,
    /// Inside `montecarlo { ... }`.
    Montecarlo,
    /// Inside the limits block of a measure.
    Limits,
    /// After `datasheet` in a part's built-in `sim` block: the figure names.
    DatasheetFigure(SimKind),
    /// Inside `datasheet FIGURE { ... }` of a block of this kind.
    Datasheet(SimKind, &'static str),
}

fn after<'a>(words: &'a [String], word: &str) -> Option<&'a str> {
    words.iter().position(|w| w == word).and_then(|i| words.get(i + 1)).map(String::as_str)
}

/// The context of a position from the headers of the blocks around it
/// (`stack`, outermost first) and the words of the statement before it (`header`).
pub fn context(stack: &[Vec<String>], header: &[String]) -> Context {
    let Some(top) = stack.last() else { return Context::None };
    let first = top.first().map(String::as_str).unwrap_or("");
    // `datasheet FIGURE { ... }` inside a part's `sim KIND` block.
    if first == "datasheet"
        && let Some(parent) = stack.len().checked_sub(2).and_then(|k| stack.get(k))
        && parent.first().map(String::as_str) == Some("sim")
        && let Some(kind) = parent.get(1).and_then(|kind| SimKind::parse(kind))
    {
        let figure = top.get(1).map(String::as_str).unwrap_or("");
        return copper_lang::simulation::datasheet_figures(kind)
            .iter()
            .find(|(name, _, _)| *name == figure)
            .map_or(Context::None, |(name, _, _)| Context::Datasheet(kind, name));
    }
    if first == "sim"
        && header == ["datasheet"]
        && let Some(kind) = top.get(1).and_then(|kind| SimKind::parse(kind))
    {
        return Context::DatasheetFigure(kind);
    }
    match first {
        "analysis" => {
            let kind = after(top, ":").or_else(|| top.get(1).map(String::as_str)).unwrap_or("");
            return sim_reference::analysis(kind).map_or(Context::None, Context::Analysis);
        }
        "sim" => return top.get(1).and_then(|kind| SimKind::parse(kind)).map_or(Context::None, Context::Block),
        "replace" => return after(top, "with").and_then(SimKind::parse).map_or(Context::None, Context::Block),
        "model" => return after(top, ":").and_then(SimKind::parse).map_or(Context::None, Context::Block),
        "montecarlo" => return Context::Montecarlo,
        "measure" => return Context::Limits,
        _ => {}
    }
    let statement = header.first().map(String::as_str).unwrap_or("");
    if first == "part" {
        return if header == ["sim"] { Context::KindName { built_in: false } } else { Context::None };
    }
    if !matches!(first, "bench" | "fixture") || stack.len() != 1 {
        return Context::None;
    }
    match statement {
        "" => Context::Statement,
        "analysis" if header.len() == 1 || (header.len() == 3 && header[2] == ":") => Context::AnalysisKind,
        "replace" if header.last().is_some_and(|w| w == "with") => Context::KindName { built_in: true },
        "model" if header.last().is_some_and(|w| w == ":") => Context::KindName { built_in: true },
        "measure" | "let" if header.iter().any(|w| w == "=") => Context::Expression,
        "plot" => Context::Expression,
        _ => Context::None,
    }
}

fn detail(unit: &str, meaning: &str) -> String {
    if unit.is_empty() { meaning.to_owned() } else { format!("{unit}: {meaning}") }
}

/// LSP completion kinds.
const PROPERTY: u32 = 10;
const KEYWORD: u32 = 14;
const FUNCTION: u32 = 3;
const ENUM: u32 = 13;

/// The completion candidates of a context: label, LSP kind and detail.
pub fn completions(context: Context) -> Vec<(String, u32, String)> {
    let items = |list: &[sim_reference::Item], kind: u32| -> Vec<(String, u32, String)> {
        list.iter().map(|item| (item.name.to_owned(), kind, detail(item.unit, item.meaning))).collect()
    };
    match context {
        Context::None => Vec::new(),
        Context::Statement => items(&BENCH_STATEMENTS, KEYWORD),
        Context::AnalysisKind => ANALYSES
            .iter()
            .map(|kind| (kind.name.to_owned(), ENUM, format!("{}: {}", kind.meaning, kind.example)))
            .collect(),
        Context::Analysis(kind) => items(kind.options, PROPERTY),
        Context::Block(kind) => {
            let mut out: Vec<(String, u32, String)> = kind
                .keys()
                .into_iter()
                .map(|key| {
                    (key.to_owned(), PROPERTY, detail(key_unit(kind, key), key_meaning(kind, key).unwrap_or("")))
                })
                .collect();
            if !copper_lang::simulation::datasheet_figures(kind).is_empty() {
                let d = sim_reference::DATASHEET;
                out.push((d.name.to_owned(), KEYWORD, detail(d.unit, d.meaning)));
            }
            out
        }
        Context::DatasheetFigure(kind) => copper_lang::simulation::datasheet_figures(kind)
            .iter()
            .map(|(figure, dimension, _)| {
                let unit = sim_reference::dimension_unit(*dimension);
                let meaning = sim_reference::datasheet_meaning(kind, figure).unwrap_or("");
                ((*figure).to_owned(), PROPERTY, detail(unit, meaning))
            })
            .collect(),
        Context::Datasheet(..) => items(&sim_reference::DATASHEET_KEYS, PROPERTY),
        Context::KindName { built_in } => {
            let kinds: Vec<SimKind> = if built_in { BUILT_IN_KINDS.to_vec() } else { SimKind::ALL.to_vec() };
            kinds.into_iter().map(|kind| (kind.name().to_owned(), ENUM, kind_meaning(kind).to_owned())).collect()
        }
        Context::Expression => {
            let mut out: Vec<(String, u32, String)> = Function::ALL
                .iter()
                .map(|f| (f.name().to_owned(), FUNCTION, format!("{} - {}", f.signature(), f.meaning())))
                .collect();
            out.extend(PROBES.iter().map(|p| (p.name.to_owned(), FUNCTION, format!("{} - {}", p.unit, p.meaning))));
            out
        }
        Context::Montecarlo => items(&MONTECARLO_KEYS, PROPERTY),
        Context::Limits => vec![
            ("min".to_owned(), PROPERTY, "the lower limit, in the measure's dimension".to_owned()),
            ("max".to_owned(), PROPERTY, "the upper limit, in the measure's dimension".to_owned()),
            (
                "where".to_owned(),
                KEYWORD,
                "scope the limits to steps: where R_LOAD <= 1kohm and TEMP > 0degC".to_owned(),
            ),
        ],
    }
}

/// The hover text of `word` in `context` (`call`: the word is followed by `(`;
/// `header`: the statement's words before it).
pub fn hover(word: &str, call: bool, context: Context, header: &[String]) -> Option<String> {
    if call || context == Context::Expression {
        if let Some(function) = Function::parse(word) {
            return Some(format!("{}\n\n{}", function.signature(), function.meaning()));
        }
        if let Some(probe) = PROBES.iter().find(|p| p.name == word) {
            return Some(format!("{}\n\n{}", probe.unit, probe.meaning));
        }
    }
    match context {
        Context::Statement => BENCH_STATEMENTS
            .iter()
            .find(|s| s.name == word)
            .map(|s| format!("{} {}\n\n{}\n\n{}", s.name, s.unit, s.meaning, s.example)),
        Context::AnalysisKind => sim_reference::analysis(word)
            .map(|kind| format!("analysis {}\n\n{}\n\n{}", kind.name, kind.meaning, kind.example)),
        Context::Analysis(kind) => kind
            .options
            .iter()
            .find(|option| option.name == word && header.is_empty())
            .map(|option| format!("analysis {} {} ({})\n\n{}", kind.name, option.name, option.unit, option.meaning)),
        Context::Block(kind) if header.is_empty() && word != "datasheet" => key_meaning(kind, word).map(|meaning| {
            let unit = key_unit(kind, word);
            let unit = if unit.is_empty() { String::new() } else { format!(" ({unit})") };
            format!("sim {} {word}{unit}\n\n{meaning}", kind.name())
        }),
        Context::Block(_) if word == "datasheet" => {
            let d = sim_reference::DATASHEET;
            Some(format!("datasheet {}\n\n{}\n\n{}", d.unit, d.meaning, d.example))
        }
        Context::DatasheetFigure(kind) => sim_reference::datasheet_meaning(kind, word)
            .map(|meaning| format!("datasheet {word}\n\n--check-model judges it on {meaning}")),
        Context::Datasheet(kind, figure) => sim_reference::DATASHEET_KEYS
            .iter()
            .find(|k| k.name == word)
            .map(|k| format!("sim {} datasheet {figure} {}\n\n{}", kind.name(), k.name, k.meaning)),
        Context::KindName { .. } => {
            SimKind::parse(word).map(|kind| format!("sim {}\n\n{}", kind.name(), kind_meaning(kind)))
        }
        Context::Montecarlo => MONTECARLO_KEYS
            .iter()
            .find(|k| k.name == word)
            .map(|k| format!("montecarlo {} ({})\n\n{}", k.name, k.unit, k.meaning)),
        _ => None,
    }
}

/// The board file a bench in `bench_file` is for: a `.copper` file beside
/// the bench's `benches/` directory that declares `board TARGET`.
pub fn board_file(bench_file: &Path, target: &str) -> Option<PathBuf> {
    let directory = bench_file.parent()?.parent()?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "copper"))
        .collect();
    entries.sort();
    entries.into_iter().find(|path| {
        let Ok(text) = std::fs::read_to_string(path) else { return false };
        let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == '{').filter(|w| !w.is_empty()).collect();
        words.windows(2).any(|pair| pair[0] == "board" && pair[1] == target)
    })
}

/// The "run this bench" code lenses of a bench document: one per
/// `bench NAME for TARGET` header whose board file is found.
pub fn code_lenses(text: &Text, tokens: &[&copper_lang::Token], path: &Path) -> Value {
    let mut lenses = Vec::new();
    let words: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
    for (i, token) in tokens.iter().enumerate() {
        if token.text != "bench" || words.get(i + 2) != Some(&"for") || (i > 0 && words[i - 1] != "}") {
            continue;
        }
        let Some(name) = words.get(i + 1) else { continue };
        // `for Board`, or `for module M of Board`.
        let rest = &words[i + 3..];
        let end = rest.iter().position(|w| *w == "{" || *w == "using").unwrap_or(rest.len());
        let target = match rest[..end].iter().position(|w| *w == "of") {
            Some(of) => rest.get(of + 1).copied(),
            None => rest.first().copied(),
        };
        let Some(board) = target.and_then(|target| board_file(path, target)) else { continue };
        let board = copper_lang::paths::display(&board);
        lenses.push(object([
            ("range", text.range(token.location.offset, token.end_offset)),
            (
                "command",
                object([
                    ("title", Value::Str(format!("Run bench {name}"))),
                    ("command", Value::from(RUN_BENCH)),
                    ("arguments", Value::Array(vec![Value::Str(board), Value::Str((*name).to_owned())])),
                ]),
            ),
        ]));
    }
    let _ = int;
    Value::Array(lenses)
}

/// What a finished bench run reports to the editor: the verdict lines
/// (`PASS: ...`, `FAIL: ...`, `ERROR: ...`) and where the pages are.
pub fn summary(stdout: &str, stderr: &str, code: Option<i32>) -> String {
    let mut lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("PASS:")
                || line.starts_with("FAIL:")
                || line.starts_with("ERROR:")
                || line.starts_with("wrote ")
        })
        .collect();
    if lines.is_empty() {
        lines.extend(stderr.lines().map(str::trim).filter(|line| !line.is_empty()).take(3));
    }
    let verdict = match code {
        Some(0) => "copper sim: pass",
        Some(1) => "copper sim: fail",
        _ => "copper sim: error",
    };
    format!("{verdict}. {}", lines.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn contexts_of_bench_and_sim_positions() {
        let bench = vec![words("bench B for Board")];
        assert_eq!(context(&bench, &[]), Context::Statement);
        assert_eq!(context(&bench, &words("analysis")), Context::AnalysisKind);
        assert_eq!(context(&bench, &words("analysis main :")), Context::AnalysisKind);
        assert_eq!(context(&bench, &words("measure m =")), Context::Expression);
        assert_eq!(context(&bench, &words("replace J1 with")), Context::KindName { built_in: true });
        let mut tran = bench.clone();
        tran.push(words("analysis tran"));
        assert!(matches!(context(&tran, &[]), Context::Analysis(kind) if kind.name == "tran"));
        let mut named = bench.clone();
        named.push(words("analysis pump : loop"));
        assert!(matches!(context(&named, &[]), Context::Analysis(kind) if kind.name == "loop"));
        let mut replace = bench.clone();
        replace.push(words("replace J1 with source"));
        assert_eq!(context(&replace, &[]), Context::Block(SimKind::Source));
        let part = vec![words("part OPA"), words("sim opamp")];
        assert_eq!(context(&part, &[]), Context::Block(SimKind::Opamp));
        assert_eq!(context(&part[..1], &words("sim")), Context::KindName { built_in: false });
    }

    #[test]
    fn completion_and_hover_come_from_the_checkers_table() {
        let names: Vec<String> = completions(Context::Block(SimKind::Opamp)).into_iter().map(|c| c.0).collect();
        let mut keys = SimKind::Opamp.keys();
        keys.push("datasheet");
        assert_eq!(names, keys);
        // A source has no data-sheet figures, so no datasheet entry is offered.
        let names: Vec<String> = completions(Context::Block(SimKind::Source)).into_iter().map(|c| c.0).collect();
        assert_eq!(names, SimKind::Source.keys());
        // `datasheet` then a figure, and inside `datasheet FIGURE { ... }` min, typ, max.
        let part = vec![words("part OPA"), words("sim opamp")];
        assert_eq!(context(&part, &words("datasheet")), Context::DatasheetFigure(SimKind::Opamp));
        let figures: Vec<String> =
            completions(Context::DatasheetFigure(SimKind::Opamp)).into_iter().map(|c| c.0).collect();
        let expected: Vec<&str> =
            copper_lang::simulation::datasheet_figures(SimKind::Opamp).iter().map(|(name, _, _)| *name).collect();
        assert_eq!(figures, expected);
        let mut inside = part.clone();
        inside.push(words("datasheet slew_rate"));
        assert_eq!(context(&inside, &[]), Context::Datasheet(SimKind::Opamp, "slew_rate"));
        let keys: Vec<String> =
            completions(Context::Datasheet(SimKind::Opamp, "slew_rate")).into_iter().map(|c| c.0).collect();
        assert_eq!(keys, ["min", "typ", "max"]);
        let text = hover("min", false, Context::Datasheet(SimKind::Opamp, "slew_rate"), &[]).unwrap();
        assert!(text.starts_with("sim opamp datasheet slew_rate min"), "{text}");
        let text = hover("datasheet", false, Context::Block(SimKind::Opamp), &[]).unwrap();
        assert!(text.starts_with("datasheet FIGURE"), "{text}");
        let gbw = completions(Context::Block(SimKind::Opamp)).into_iter().find(|c| c.0 == "gbw").unwrap();
        assert_eq!(gbw.2, "Hz: gain-bandwidth product");
        let tran = sim_reference::analysis("tran").unwrap();
        let options: Vec<String> = completions(Context::Analysis(tran)).into_iter().map(|c| c.0).collect();
        assert_eq!(options, sim_reference::option_names("tran"));
        let functions = completions(Context::Expression);
        assert!(functions.iter().any(|c| c.0 == "thd" && c.2.starts_with("thd(x, f0)")));
        assert_eq!(
            hover("thd", true, Context::Expression, &[]).unwrap().lines().next(),
            Some("thd(x, f0) or thd(x, f0, n)")
        );
        assert!(
            hover("max_step", false, Context::Analysis(tran), &[]).unwrap().starts_with("analysis tran max_step (s)")
        );
        assert!(hover("slew_rate", false, Context::Block(SimKind::Opamp), &[]).unwrap().contains("(V/s)"));
        assert!(hover("tran", false, Context::AnalysisKind, &[]).unwrap().contains("analysis tran { stop = 1ms; }"));
    }

    #[test]
    fn a_run_summary_keeps_the_verdicts() {
        let out = "BENCH A (B): ac\n  m  1 V  none\n  PASS: 1 of 1 limited measures within limits\nwrote build/sim: sim-results.json\n";
        assert_eq!(
            summary(out, "", Some(0)),
            "copper sim: pass. PASS: 1 of 1 limited measures within limits wrote build/sim: sim-results.json"
        );
        assert_eq!(
            summary("", "SIM ERROR: board B has no bench 'X'", Some(2)),
            "copper sim: error. SIM ERROR: board B has no bench 'X'"
        );
    }
}
