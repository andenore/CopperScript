//! The panel's design rules from its boards' `.kicad_pro` and `.kicad_dru`.
//!
//! Project design settings and net classes must be identical across the
//! boards (a panel is fabricated with one rule set), and so must the custom
//! rules without a condition, in the same order: they are kept once, first,
//! and are the only rules between two boards' items, whatever order the
//! boards are declared in. Rules with a condition are board-specific
//! (CopperScript writes courtyard rules keyed on references). A board that
//! has them repeats its whole rule list in its own order (the last matching
//! rule wins), each rule renamed, its `Reference` and `NetName` names
//! prefixed for the panel (an empty name stays empty), and limited to pairs
//! where neither item belongs to another board and at least one belongs to
//! this board. Within the board, and against the frame, the board edge or
//! an unnetted hole, its rules therefore apply as on the board alone.

use copper_json::{Dumps, Value};

use crate::sexpr::{Node, parse, print, quote};

/// Functions whose quoted argument names footprint references, and nets.
const REFERENCE_FUNCTIONS: [&str; 4] =
    ["memberOfFootprint", "intersectsCourtyard", "intersectsFrontCourtyard", "intersectsBackCourtyard"];
const NET_FUNCTIONS: [&str; 1] = ["inDiffPair"];

/// Rewrite every name a condition gives to the panel's prefixed names:
/// `.Reference` and `.NetName` compared with `==` or `!=`, and the argument
/// of the functions that name footprints (`memberOfFootprint('J1')`, the
/// courtyard tests) or nets (`inDiffPair('USB')`), in single or double
/// quotes. An empty name, and one of only `*`, stays as written: it means
/// no name or any name, the edge and unnetted items included.
pub fn rewrite_condition(condition: &str, instance: &str) -> String {
    let rename = |name: &str, reference: bool| -> String {
        if name.is_empty() || name.chars().all(|c| c == '*') {
            name.to_owned()
        } else if reference {
            format!("{instance}-{name}")
        } else {
            format!("{instance}/{name}")
        }
    };
    let text = condition.as_bytes();
    let skip = |mut j: usize| {
        while j < text.len() && text[j].is_ascii_whitespace() {
            j += 1;
        }
        j
    };
    // The quoted value at `j`: (value start, value end, closing quote index).
    let quoted = |j: usize| -> Option<(usize, usize)> {
        let q = *text.get(j)?;
        if q != b'\'' && q != b'"' {
            return None;
        }
        let end = j + 1 + text[j + 1..].iter().position(|&c| c == q)?;
        Some((j + 1, end))
    };
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out: Vec<u8> = Vec::with_capacity(text.len() + 16);
    let mut k = 0;
    'scan: while k < text.len() {
        let c = text[k];
        if c == b'\'' || c == b'"' {
            // A string no rule below claimed: copied as it is.
            let end = quoted(k).map_or(text.len() - 1, |(_, end)| end);
            out.extend_from_slice(&text[k..=end]);
            k = end + 1;
            continue;
        }
        if c == b'.' {
            let rest = &condition[k + 1..];
            for (key, reference) in [("Reference", true), ("NetName", false)] {
                let after = k + 1 + key.len();
                if rest.starts_with(key) && text.get(after).is_none_or(|&c| !ident(c)) {
                    let op = skip(after);
                    if (text[op..].starts_with(b"==") || text[op..].starts_with(b"!="))
                        && let Some((start, end)) = quoted(skip(op + 2))
                    {
                        out.extend_from_slice(&text[k..start]);
                        out.extend_from_slice(rename(&condition[start..end], reference).as_bytes());
                        out.push(text[end]);
                        k = end + 1;
                        continue 'scan;
                    }
                }
            }
            let functions =
                REFERENCE_FUNCTIONS.iter().map(|f| (*f, true)).chain(NET_FUNCTIONS.iter().map(|f| (*f, false)));
            for (function, reference) in functions {
                let after = k + 1 + function.len();
                if rest.starts_with(function) && text.get(after).is_none_or(|&c| !ident(c)) {
                    let open = skip(after);
                    if text.get(open) == Some(&b'(')
                        && let Some((start, end)) = quoted(skip(open + 1))
                    {
                        out.extend_from_slice(&text[k..start]);
                        out.extend_from_slice(rename(&condition[start..end], reference).as_bytes());
                        out.push(text[end]);
                        k = end + 1;
                        continue 'scan;
                    }
                }
            }
        }
        out.push(c);
        k += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| condition.to_owned())
}

/// `item` (`A` or `B`) is one of board `instance`'s items.
fn owned_by(instance: &str, item: char) -> String {
    format!("({item}.NetName == '{instance}/*' || {item}.Reference == '{instance}-*')")
}

/// `item` belongs to none of the `others` boards.
fn not_others(others: &[&str], item: char) -> Option<String> {
    let terms: Vec<String> = others
        .iter()
        .map(|other| format!("{item}.NetName != '{other}/*' && {item}.Reference != '{other}-*'"))
        .collect();
    (!terms.is_empty()).then(|| format!("({})", terms.join(" && ")))
}

/// A board's rule condition on the panel (`None` for an unconditional
/// rule): its names prefixed, limited to pairs where neither item belongs to
/// one of the `others` boards and at least one belongs to `instance`.
pub fn scope_condition(condition: Option<&str>, instance: &str, others: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(not_others(others, 'A'));
    parts.extend(not_others(others, 'B'));
    parts.push(format!("({} || {})", owned_by(instance, 'A'), owned_by(instance, 'B')));
    if let Some(condition) = condition {
        parts.push(format!("({})", rewrite_condition(condition, instance)));
    }
    parts.join(" && ")
}

fn rules_of(text: &str, source: &str) -> Result<Vec<Node>, String> {
    let body: String = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n");
    let root = parse(&format!("(rules\n{body}\n)"), source)?;
    for item in root.items().iter().skip(1) {
        match item.tag() {
            Some("version" | "rule") => {}
            other => return Err(format!("{source}: unexpected {} in design rules", other.unwrap_or("item"))),
        }
    }
    Ok(root.children("rule").cloned().collect())
}

/// The largest copper-to-edge clearance the panel's rules can ask for: the
/// project minimum and every custom `edge_clearance` rule's minimum.
pub fn max_edge_clearance(pro: &str, dru: &str) -> Result<i64, String> {
    let mm_nm = |text: &str| -> Option<i64> {
        let number = text.trim().trim_end_matches("mm");
        number.parse::<f64>().ok().map(|v| (v * 1e6).round() as i64)
    };
    let project = copper_json::parse(pro).map_err(|e| format!("panel project: {e:?}"))?;
    let mut worst = project
        .get("board")
        .and_then(|b| b.get("design_settings"))
        .and_then(|d| d.get("rules"))
        .and_then(|r| r.get("min_copper_edge_clearance"))
        .and_then(|v| match v {
            Value::Float(f) => Some((f * 1e6).round() as i64),
            Value::Int(i) => i.to_string().parse::<i64>().ok().map(|v| v * 1_000_000),
            _ => None,
        })
        .unwrap_or(0);
    for rule in rules_of(dru, "panel design rules")? {
        for constraint in rule.children("constraint") {
            if constraint.value(1) == Some("edge_clearance")
                && let Some(min) = constraint.child("min").and_then(|m| m.value(1)).and_then(mm_nm)
            {
                worst = worst.max(min);
            }
        }
    }
    Ok(worst)
}

/// Whether a `.kicad_dru` text has a rule with a condition.
pub fn has_conditional(text: &str, source: &str) -> Result<bool, String> {
    Ok(rules_of(text, source)?.iter().any(|rule| rule.child("condition").is_some()))
}

/// Merge the boards' `.kicad_dru` texts, `(instance, text)` in board order
/// (the result does not depend on it).
pub fn merge_dru(boards: &[(String, String)]) -> Result<String, String> {
    let instances: Vec<&str> = boards.iter().map(|(instance, _)| instance.as_str()).collect();
    let mut shared: Option<(String, Vec<String>, Vec<Node>)> = None;
    let mut specific: Vec<Node> = Vec::new();
    for (instance, text) in boards {
        let source = format!("board {instance} design rules");
        let rules = rules_of(text, &source)?;
        let unconditional: Vec<Node> = rules.iter().filter(|r| r.child("condition").is_none()).cloned().collect();
        let printed: Vec<String> = unconditional.iter().map(print).collect();
        match &shared {
            None => shared = Some((instance.clone(), printed, unconditional)),
            Some((first, first_printed, _)) if *first_printed != printed => {
                return Err(format!(
                    "boards {first} and {instance} have different unconditional design rules (or the same in another \
                     order); a panel needs one unconditional rule set"
                ));
            }
            Some(_) => {}
        }
        if rules.iter().all(|r| r.child("condition").is_none()) {
            continue;
        }
        let others: Vec<&str> = instances.iter().copied().filter(|other| other != instance).collect();
        for rule in &rules {
            let name = rule.string_value(1).unwrap_or_default();
            let mut items = rule.items().to_vec();
            items[1] = Node::atom(quote(&format!("{instance}: {name}")));
            let condition = rule.child("condition").and_then(|c| c.string_value(1));
            let scoped = Node::list(vec![
                Node::atom("condition"),
                Node::atom(quote(&scope_condition(condition.as_deref(), instance, &others))),
            ]);
            match items.iter_mut().find(|item| item.tag() == Some("condition")) {
                Some(item) => *item = scoped,
                None => items.push(scoped),
            }
            specific.push(Node::List(items));
        }
    }
    let shared: Vec<Node> = shared.map(|(_, _, rules)| rules).unwrap_or_default();
    let mut out = String::from("(version 1)\n# Generated by copper panelize from the boards' rules; do not edit.\n");
    for rule in &shared {
        out.push_str(&print(rule));
    }
    for rule in &specific {
        out.push_str(&print(rule));
    }
    Ok(out)
}

/// Merge the boards' `.kicad_pro` texts: their design settings and net
/// settings must be identical; the panel keeps them under its own file name.
pub fn merge_pro(boards: &[(String, String)], file_name: &str) -> Result<String, String> {
    let mut first: Option<(String, Value)> = None;
    for (instance, text) in boards {
        let project = copper_json::parse(text).map_err(|e| format!("board {instance} project: {e:?}"))?;
        let Value::Object(object) = &project else { return Err(format!("board {instance} project is not an object")) };
        if let Some(Value::Object(net_settings)) = object.get("net_settings") {
            let patterns = net_settings.get("netclass_patterns");
            let assignments = net_settings.get("netclass_assignments");
            if !matches!(patterns, None | Some(Value::Null))
                && !matches!(patterns, Some(Value::Array(a)) if a.is_empty())
                || !matches!(assignments, None | Some(Value::Null))
            {
                return Err(format!(
                    "board {instance} assigns net classes; panels do not carry net-class assignments yet"
                ));
            }
        }
        match &first {
            None => first = Some((instance.clone(), project)),
            Some((first_instance, first_project)) => {
                for key in ["board", "net_settings"] {
                    let a = match first_project {
                        Value::Object(o) => o.get(key),
                        _ => None,
                    };
                    if a != object.get(key) {
                        return Err(format!(
                            "board {instance} has different project {key} from board {first_instance}; a panel needs one rule set"
                        ));
                    }
                }
            }
        }
    }
    let (_, mut project) = first.ok_or("a panel needs at least one board")?;
    if let Value::Object(object) = &mut project
        && let Some(Value::Object(meta)) = object.get_mut("meta")
    {
        meta.insert("filename".to_owned(), Value::from(file_name));
    }
    Ok(Dumps::new().indent(2).dumps(&project) + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_cam::rules::{Item, Rules};

    const PROJECT: &str = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.1, "min_copper_edge_clearance": 0.3, "min_hole_clearance": 0.25}}}, "net_settings": {"classes": []}}"#;

    fn item(net: &str) -> Item<'_> {
        Item { net: Some(net), ..Item::default() }
    }

    #[test]
    fn conditions_get_the_board_prefix_but_empty_names_stay_empty() {
        let condition = "(A.Reference == 'CA' || A.Reference=='RSER') && B.NetName != 'GND' && A.Value == 'x'";
        assert_eq!(
            rewrite_condition(condition, "A3"),
            "(A.Reference == 'A3-CA' || A.Reference=='A3-RSER') && B.NetName != 'A3/GND' && A.Value == 'x'"
        );
        assert_eq!(
            rewrite_condition("B.NetName == '' && B.Reference == ''", "A"),
            "B.NetName == '' && B.Reference == ''"
        );
        assert_eq!(
            rewrite_condition("B.NetName == '*' && B.Reference != '**'", "A"),
            "B.NetName == '*' && B.Reference != '**'"
        );
        assert_eq!(rewrite_condition("A.NetName == 'PWR*'", "A"), "A.NetName == 'A/PWR*'");
        // Double quotes, footprint functions and diff pairs are names too;
        // other strings are left alone.
        assert_eq!(rewrite_condition("A.NetName == \"GND\"", "A"), "A.NetName == \"A/GND\"");
        assert_eq!(
            rewrite_condition(
                "A.memberOfFootprint('J1') && B.memberOfFootprint( \"J1\" ) && (A.Pad_Type == 'NPTH, mechanical')",
                "X"
            ),
            "A.memberOfFootprint('X-J1') && B.memberOfFootprint( \"X-J1\" ) && (A.Pad_Type == 'NPTH, mechanical')"
        );
        assert_eq!(
            rewrite_condition("A.intersectsCourtyard('U1') || A.inDiffPair('USB_D')", "X"),
            "A.intersectsCourtyard('X-U1') || A.inDiffPair('X/USB_D')"
        );
        assert_eq!(rewrite_condition("A.Value == 'Reference'", "X"), "A.Value == 'Reference'");
    }

    #[test]
    fn shared_rules_merge_once_and_specific_ones_per_board() {
        let dru = "(version 1)\n# comment\n(rule \"Default gap\"\n  (constraint courtyard_clearance (min 0.5mm)))\n\
                   (rule \"Gap 1\"\n  (constraint courtyard_clearance (min 0mm))\n  (condition \"A.Reference == 'J1'\"))\n";
        let merged = merge_dru(&[("A".into(), dru.into()), ("B".into(), dru.into())]).expect("merged");
        assert_eq!(merged.matches("(rule \"Default gap\"").count(), 1, "{merged}");
        assert!(merged.contains("\"A: Default gap\"") && merged.contains("\"B: Default gap\""), "{merged}");
        assert!(merged.contains("\"A: Gap 1\"") && merged.contains("'B-J1'"), "{merged}");
        // B's rule: neither item is A's, at least one is B's, then its own condition.
        let scoped = "(A.NetName != 'A/*' && A.Reference != 'A-*') && (B.NetName != 'A/*' && B.Reference != 'A-*') && \
                      ((A.NetName == 'B/*' || A.Reference == 'B-*') || (B.NetName == 'B/*' || B.Reference == 'B-*')) && \
                      (A.Reference == 'B-J1')";
        assert!(merged.contains(scoped), "{merged}");
        let other = dru.replace("min 0.5mm", "min 0.4mm");
        let error = merge_dru(&[("A".into(), dru.into()), ("B".into(), other)]).expect_err("refused");
        assert!(error.contains("different unconditional design rules"), "{error}");
    }

    #[test]
    fn one_boards_rule_cannot_reach_another_boards_items() {
        // A raises SIG's clearance; B's rule, read on B's own names, lowers
        // everything but its GND. It must not match board A's nets.
        let a = "(version 1)\n(rule \"big\" (constraint clearance (min 1mm)) (condition \"A.NetName == 'SIG'\"))\n";
        let b = "(version 1)\n(rule \"small\" (constraint clearance (min 0.2mm)) (condition \"A.NetName != 'GND'\"))\n";
        let merged = merge_dru(&[("A".into(), a.into()), ("B".into(), b.into())]).expect("merged");
        let rules = Rules::load(PROJECT, Some(&merged)).expect("the panel rules");
        assert_eq!(rules.clearance(&item("A/GND"), &item("A/SIG")), 1_000_000, "{merged}");
        assert_eq!(rules.clearance(&item("B/GND"), &item("B/SIG")), 200_000, "{merged}");
    }

    #[test]
    fn a_board_keeps_its_rule_order_on_the_panel() {
        // On the board, the unconditional 1 mm rule follows (and overrides) the
        // conditional 0.2 mm one: SIG keeps 1 mm.
        let dru = "(version 1)\n(rule \"sig\" (constraint clearance (min 0.2mm)) (condition \"A.NetName == 'SIG'\"))\n\
                   (rule \"all\" (constraint clearance (min 1mm)))\n";
        let alone = Rules::load(PROJECT, Some(dru)).expect("the board's rules");
        assert_eq!(alone.clearance(&item("GND"), &item("SIG")), 1_000_000);
        let merged = merge_dru(&[("A".into(), dru.into())]).expect("merged");
        let panel = Rules::load(PROJECT, Some(&merged)).expect("the panel rules");
        assert_eq!(panel.clearance(&item("A/GND"), &item("A/SIG")), 1_000_000, "{merged}");
    }

    #[test]
    fn declaration_order_does_not_change_any_clearance() {
        // Both boards share a 3 mm baseline; A relaxes its SIG to 0.2 mm, B
        // has a rule of its own. Between the boards only the baseline applies.
        let a = "(version 1)\n(rule \"base\" (constraint clearance (min 3mm)))\n\
                 (rule \"fine\" (constraint clearance (min 0.2mm)) (condition \"A.NetName == 'SIG'\"))\n";
        let b = "(version 1)\n(rule \"base\" (constraint clearance (min 3mm)))\n\
                 (rule \"other\" (constraint clearance (min 0.5mm)) (condition \"A.NetName == 'X'\"))\n";
        let forward = merge_dru(&[("A".into(), a.into()), ("B".into(), b.into())]).expect("merged");
        let backward = merge_dru(&[("B".into(), b.into()), ("A".into(), a.into())]).expect("merged");
        for merged in [&forward, &backward] {
            let rules = Rules::load(PROJECT, Some(merged)).expect("the panel rules");
            assert_eq!(rules.clearance(&item("A/SIG"), &item("B/GND")), 3_000_000, "{merged}");
            assert_eq!(rules.clearance(&item("B/X"), &item("A/GND")), 3_000_000, "{merged}");
            assert_eq!(rules.clearance(&item("A/SIG"), &item("A/GND")), 200_000, "{merged}");
            assert_eq!(rules.clearance(&item("B/X"), &item("B/GND")), 500_000, "{merged}");
        }
        // The same unconditional rules in another order are refused.
        let swapped = "(version 1)\n(rule \"y\" (constraint clearance (min 0.2mm)))\n(rule \"x\" (constraint clearance (min 1mm)))\n";
        let straight = "(version 1)\n(rule \"x\" (constraint clearance (min 1mm)))\n(rule \"y\" (constraint clearance (min 0.2mm)))\n";
        let error = merge_dru(&[("A".into(), straight.into()), ("B".into(), swapped.into())]).expect_err("refused");
        assert!(error.contains("or the same in another order"), "{error}");
    }

    #[test]
    fn rules_against_the_edge_and_unnetted_holes_keep_working() {
        // A rule naming B still applies against the board edge (no net, no
        // reference) and an unnetted hole (empty net name stays empty).
        let dru = "(version 1)\n\
                   (rule \"edge\" (constraint edge_clearance (min 1mm)) (condition \"A.NetName == 'SIG' && B.NetName == ''\"))\n\
                   (rule \"hole\" (constraint hole_clearance (min 0.8mm)) (condition \"A.NetName == 'SIG' && B.NetName == ''\"))\n";
        let alone = Rules::load(PROJECT, Some(dru)).expect("the board's rules");
        let merged = merge_dru(&[("A".into(), dru.into()), ("B".into(), "(version 1)\n".into())]).expect("merged");
        let panel = Rules::load(PROJECT, Some(&merged)).expect("the panel rules");
        assert_eq!(alone.edge_clearance(&item("SIG")), 1_000_000);
        assert_eq!(panel.edge_clearance(&item("A/SIG")), 1_000_000, "{merged}");
        let hole = Item { reference: Some("A-MH1"), ..Item::default() };
        assert_eq!(alone.hole_clearance(&item("SIG"), &Item { reference: Some("MH1"), ..Item::default() }), 800_000);
        assert_eq!(panel.hole_clearance(&item("A/SIG"), &hole), 800_000, "{merged}");
        // Board B's items do not get A's rules.
        assert_eq!(panel.edge_clearance(&item("B/SIG")), 300_000, "{merged}");
        // A wildcard naming any net or reference still reaches the edge.
        for wild in ["B.NetName == '*'", "B.Reference == '*'"] {
            let dru = format!(
                "(version 1)\n(rule \"edge\" (constraint edge_clearance (min 1mm)) (condition \"A.NetName == 'SIG' && {wild}\"))\n"
            );
            let alone = Rules::load(PROJECT, Some(&dru)).expect("the board's rules");
            let merged = merge_dru(&[("A".into(), dru.clone()), ("B".into(), "(version 1)\n".into())]).expect("merged");
            let panel = Rules::load(PROJECT, Some(&merged)).expect("the panel rules");
            assert_eq!(alone.edge_clearance(&item("SIG")), 1_000_000, "{wild}");
            assert_eq!(panel.edge_clearance(&item("A/SIG")), 1_000_000, "{wild}: {merged}");
            assert_eq!(panel.edge_clearance(&item("B/SIG")), 300_000, "{wild}: {merged}");
        }
    }

    #[test]
    fn a_double_quoted_name_keeps_its_rule() {
        let dru =
            "(version 1)\n(rule \"g\" (constraint clearance (min 1mm)) (condition \"A.NetName == \\\"GND\\\"\"))\n";
        let alone = Rules::load(PROJECT, Some(dru)).expect("the board's rules");
        assert_eq!(alone.clearance(&item("GND"), &item("SIG")), 1_000_000);
        let merged = merge_dru(&[("A".into(), dru.into())]).expect("merged");
        let panel = Rules::load(PROJECT, Some(&merged)).expect("the panel rules");
        assert_eq!(panel.clearance(&item("A/GND"), &item("A/SIG")), 1_000_000, "{merged}");
    }

    #[test]
    fn projects_must_agree() {
        let pro = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.2}}}, "meta": {"filename": "a.kicad_pro", "version": 3}, "net_settings": {"classes": [], "netclass_patterns": []}}"#;
        let merged =
            merge_pro(&[("A".into(), pro.into()), ("B".into(), pro.into())], "panel.kicad_pro").expect("merged");
        assert!(merged.contains("panel.kicad_pro"), "{merged}");
        let other = pro.replace("0.2", "0.15");
        assert!(merge_pro(&[("A".into(), pro.into()), ("B".into(), other)], "p").is_err());
    }
}
