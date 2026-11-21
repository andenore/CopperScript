//! The simulation grammar: `param`, part `sim` blocks, bench documents and
//! bench expressions (`docs/language-reference.md`, "Simulation").

use super::{Parser, Result, repr};
use crate::lexer::TokenKind;
use crate::suggest::close_match;
use crate::syntax::*;

const BENCH_KEYWORDS: [&str; 19] = [
    "component",
    "net",
    "param",
    "supply",
    "model",
    "replace",
    "ignore",
    "analysis",
    "step",
    "reference",
    "let",
    "measure",
    "plot",
    "use",
    "import",
    "montecarlo",
    "worstcase",
    "tolerance",
    "ic",
];

impl Parser {
    /// `for TARGET` after a bench's or fixture's name: a board or a module
    /// name; a bench or a fixture may add `using FIXTURE, FIXTURE`.
    pub(super) fn bench_target(&mut self, _kind: DocumentKind) -> Result<Declaration> {
        self.expect_word("for")?;
        let location = self.current().location.clone();
        let board = self.name("board or module name")?;
        let mut fixtures = Vec::new();
        let mut fixture_locations = Vec::new();
        if self.current().kind == TokenKind::Identifier && self.current().text == "using" {
            self.advance();
            loop {
                fixture_locations.push(self.current().location.clone());
                fixtures.push(self.name("fixture name")?);
                if !self.accept_symbol(",") {
                    break;
                }
            }
        }
        Ok(Declaration::Bench(BenchStatement::Target(BenchTargetDecl { location, board, fixtures, fixture_locations })))
    }

    /// `param NAME = scalar;` or `param NAME = expression;` (the keyword is read).
    /// A number, a quantity or a negated one is a literal; a string is read as
    /// before; anything else is the expression of a derived param.
    pub(super) fn param(&mut self, location: Location) -> Result<ParamDecl> {
        let name = self.name("param name")?;
        self.expect_symbol("=")?;
        if self.current().kind == TokenKind::String {
            let value = self.scalar()?;
            self.expect_symbol(";")?;
            return Ok(ParamDecl { location, name, value, expression: None });
        }
        let expression = self.expression()?;
        self.expect_symbol(";")?;
        let literal = |negative: bool, kind: &ExprKind| -> Option<Scalar> {
            let sign = if negative { "-" } else { "" };
            match kind {
                ExprKind::Quantity { value, unit } => {
                    Some(Scalar::Quantity { value: format!("{sign}{value}"), unit: unit.clone() })
                }
                ExprKind::Number(text) if !text.contains(['.', 'e', 'E']) => {
                    format!("{sign}{text}").parse().ok().map(Scalar::Int)
                }
                ExprKind::Number(text) => format!("{sign}{text}").parse().ok().map(Scalar::Float),
                _ => None,
            }
        };
        let value = match &expression.kind {
            ExprKind::Negate(inner) => literal(true, &inner.kind),
            kind => literal(false, kind),
        };
        Ok(match value {
            Some(value) => ParamDecl { location, name, value, expression: None },
            None => ParamDecl { location, name, value: Scalar::Str(String::new()), expression: Some(expression) },
        })
    }

    /// `sim KIND [UNIT] { entries }` (the keyword is read). A part block may
    /// hold parameter sets per supply: `supply 3.3V { slew_rate = ...; }`.
    pub(super) fn sim_block(&mut self, location: Location) -> Result<SimDecl> {
        let kind = self.name("sim model kind")?;
        let unit = if self.at_symbol("{") { None } else { Some(self.name("sim unit name")?) };
        let entries = self.sim_entries_with(true)?;
        Ok(SimDecl { location, kind, unit, entries })
    }

    /// `{ key = value; ... }`, a value being a scalar, a comma list, or `KIND { ... }`.
    fn sim_entries(&mut self) -> Result<Vec<SimEntry>> {
        self.sim_entries_with(false)
    }

    /// [`Self::sim_entries`]; with `sets`, also `supply QUANTITY { key = value; ... }`
    /// entries, kept as key `supply` with a block whose kind is the quantity
    /// as `VALUE UNIT` (see [`crate::simulation`]).
    fn sim_entries_with(&mut self, sets: bool) -> Result<Vec<SimEntry>> {
        self.expect_symbol("{")?;
        let mut entries: Vec<SimEntry> = Vec::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let key = self.name("property name")?;
            if sets && key == "supply" && !self.at_symbol("=") {
                let span = match self.scalar()? {
                    Scalar::Quantity { value, unit } => format!("{value} {unit}"),
                    _ => {
                        return Err(self.error_at(
                            "SIM096",
                            "a parameter set per supply names the supply span as a voltage: supply 3.3V { ... }",
                            &token,
                        ));
                    }
                };
                let value = SimValue::Block { kind: span, entries: self.sim_entries()? };
                self.accept_symbol(";");
                entries.push(SimEntry { location: token.location, key, value });
                continue;
            }
            if entries.iter().any(|entry| entry.key == key) {
                return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &token));
            }
            self.expect_symbol("=")?;
            let block = self.current().kind == TokenKind::Identifier
                && self
                    .tokens
                    .get(self.index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Symbol && next.text == "{");
            let value = if block {
                let kind = self.advance().text;
                SimValue::Block { kind, entries: self.sim_entries()? }
            } else {
                let first = self.scalar()?;
                if self.at_symbol(",") {
                    let mut values = vec![first];
                    while self.accept_symbol(",") {
                        values.push(self.scalar()?);
                    }
                    SimValue::List(values)
                } else {
                    SimValue::Scalar(first)
                }
            };
            if !block {
                self.expect_symbol(";")?;
            } else {
                self.accept_symbol(";");
            }
            entries.push(SimEntry { location: token.location, key, value });
        }
        self.expect_symbol("}")?;
        Ok(entries)
    }

    /// One declaration of a bench body (the keyword is read).
    pub(super) fn bench_declaration(&mut self, keyword: &str, location: Location) -> Result<Declaration> {
        Ok(match keyword {
            "component" => Declaration::Component(self.component()?),
            "net" => Declaration::Net(self.bench_net()?),
            "param" => Declaration::Param(self.param(location)?),
            "supply" => Declaration::Supply(self.named_block("supply name")?),
            "model" => {
                let name = self.name("model name")?;
                self.expect_symbol(":")?;
                let kind = self.name("sim model kind")?;
                let entries = self.sim_entries()?;
                Declaration::Bench(BenchStatement::Model(ModelDecl { location, name, kind, entries }))
            }
            "replace" => {
                let target = self.reference()?;
                self.expect_word("with")?;
                let kind = self.name("sim model kind")?;
                let entries = self.sim_entries()?;
                Declaration::Bench(BenchStatement::Replace(ReplaceDecl { location, target, kind, entries }))
            }
            "ignore" => {
                let mut targets = Vec::new();
                let mut target_locations = Vec::new();
                loop {
                    target_locations.push(self.current().location.clone());
                    targets.push(self.reference()?);
                    if !self.accept_symbol(",") {
                        break;
                    }
                }
                self.expect_symbol(";")?;
                Declaration::Bench(BenchStatement::Ignore(IgnoreDecl { location, targets, target_locations }))
            }
            "analysis" => {
                let first = self.name("analysis kind")?;
                // `analysis NAME: KIND { ... }` names one analysis of several.
                let (name, kind) =
                    if self.accept_symbol(":") { (Some(first), self.name("analysis kind")?) } else { (None, first) };
                let entries = if self.accept_symbol(";") {
                    Vec::new()
                } else if kind == "noise" || kind == "pz" {
                    self.noise_entries()?
                } else {
                    self.sim_entries()?
                };
                Declaration::Bench(BenchStatement::Analysis(AnalysisDecl { location, name, kind, entries }))
            }
            "step" => {
                let param = self.name("param name")?;
                let mut others = Vec::new();
                while self.accept_symbol(",") {
                    others.push(self.name("param name")?);
                }
                let word = |parser: &Self, text: &str| {
                    parser.current().kind == TokenKind::Identifier && parser.current().text == text
                };
                if others.is_empty() && word(self, "from") {
                    // A range: `step P from a to b by s;` or `step P from a to b points n;`.
                    self.advance();
                    let from = self.scalar()?;
                    self.expect_word("to")?;
                    let to = self.scalar()?;
                    let (by, points) = if word(self, "by") {
                        self.advance();
                        (Some(self.scalar()?), None)
                    } else if word(self, "points") {
                        self.advance();
                        (None, Some(self.scalar()?))
                    } else {
                        return Err(self.error(
                            "SIM115",
                            "a stepped range needs its spacing or its number of values: step P from a to b by s; or step P from a to b points n;",
                        ));
                    };
                    self.expect_symbol(";")?;
                    return Ok(Declaration::Bench(BenchStatement::Step(StepDecl {
                        location,
                        param,
                        values: Vec::new(),
                        zip: Vec::new(),
                        range: Some(StepRange { from, to, by, points }),
                    })));
                }
                self.expect_word("in")?;
                if others.is_empty() {
                    let mut values = vec![self.scalar()?];
                    while self.accept_symbol(",") {
                        values.push(self.scalar()?);
                    }
                    self.expect_symbol(";")?;
                    return Ok(Declaration::Bench(BenchStatement::Step(StepDecl {
                        location,
                        param,
                        values,
                        zip: Vec::new(),
                        range: None,
                    })));
                }
                // Zipped: one parenthesized tuple of values per step.
                let mut columns: Vec<Vec<Scalar>> = vec![Vec::new(); others.len() + 1];
                loop {
                    let open = self.current().clone();
                    self.expect_symbol("(")?;
                    let mut row = vec![self.scalar()?];
                    while self.accept_symbol(",") {
                        row.push(self.scalar()?);
                    }
                    self.expect_symbol(")")?;
                    if row.len() != columns.len() {
                        return Err(self.error_at(
                            "SIM041",
                            format!(
                                "a zipped step of {} params needs {} values per tuple, found {}",
                                columns.len(),
                                columns.len(),
                                row.len()
                            ),
                            &open,
                        ));
                    }
                    for (column, value) in columns.iter_mut().zip(row) {
                        column.push(value);
                    }
                    if !self.accept_symbol(",") {
                        break;
                    }
                }
                self.expect_symbol(";")?;
                let mut columns = columns.into_iter();
                let values = columns.next().expect("one column per param");
                let zip = others.into_iter().zip(columns).collect();
                Declaration::Bench(BenchStatement::Step(StepDecl { location, param, values, zip, range: None }))
            }
            "reference" => {
                let name = self.name("reference name")?;
                let entries = self.sim_entries()?;
                Declaration::Bench(BenchStatement::Reference(ReferenceDecl { location, name, entries }))
            }
            "let" => {
                let name = self.name("let name")?;
                self.expect_symbol("=")?;
                let expression = self.expression()?;
                self.expect_symbol(";")?;
                Declaration::Bench(BenchStatement::Let(LetDecl { location, name, expression }))
            }
            "measure" => {
                let name = self.name("measure name")?;
                self.expect_symbol("=")?;
                let expression = self.expression()?;
                let (limits, conditions) =
                    if self.accept_symbol(";") { (Vec::new(), Vec::new()) } else { self.measure_limits()? };
                Declaration::Bench(BenchStatement::Measure(MeasureDecl {
                    location,
                    name,
                    expression,
                    limits,
                    conditions,
                }))
            }
            "ic" => {
                let target_location = self.current().location.clone();
                let probe = self.current().kind == TokenKind::Identifier
                    && self.current().text == "V"
                    && self
                        .tokens
                        .get(self.index + 1)
                        .is_some_and(|next| next.kind == TokenKind::Symbol && next.text == "(");
                let target = if probe {
                    self.advance();
                    self.expect_symbol("(")?;
                    let node = self.qualified_name("node name")?;
                    if self.at_symbol(",") {
                        return Err(self.error(
                            "SIM075",
                            "ic V(NODE) sets one node against ground; set a capacitor's voltage with ic C1 = value;",
                        ));
                    }
                    self.expect_symbol(")")?;
                    InitialConditionTarget::Node(node)
                } else {
                    InitialConditionTarget::Component(self.name("capacitor, inductor or V(NODE)")?)
                };
                self.expect_symbol("=")?;
                let value = self.scalar()?;
                self.expect_symbol(";")?;
                Declaration::Bench(BenchStatement::InitialCondition(InitialConditionDecl {
                    location,
                    target,
                    target_location,
                    value,
                }))
            }
            "plot" => {
                let mut expressions = vec![self.expression()?];
                while self.accept_symbol(",") {
                    expressions.push(self.expression()?);
                }
                self.expect_symbol(";")?;
                Declaration::Bench(BenchStatement::Plot(PlotDecl { location, expressions }))
            }
            "montecarlo" | "worstcase" => {
                let entries = if self.accept_symbol(";") { Vec::new() } else { self.sim_entries()? };
                Declaration::Bench(BenchStatement::Variation(VariationDecl {
                    location,
                    kind: keyword.to_owned(),
                    target: None,
                    others: Vec::new(),
                    match_value: None,
                    value: None,
                    entries,
                }))
            }
            "tolerance" => {
                // `tolerance T = t;`, or a matched group: `tolerance A, B = t match m;`.
                let target = self.qualified_name("param or COMPONENT.PARAMETER")?;
                let mut others = Vec::new();
                while self.accept_symbol(",") {
                    others.push(self.qualified_name("component or COMPONENT.PARAMETER")?);
                }
                self.expect_symbol("=")?;
                let value = self.scalar()?;
                let match_value = if self.current().kind == TokenKind::Identifier && self.current().text == "match" {
                    self.advance();
                    Some(self.scalar()?)
                } else {
                    None
                };
                self.expect_symbol(";")?;
                Declaration::Bench(BenchStatement::Variation(VariationDecl {
                    location,
                    kind: keyword.to_owned(),
                    target: Some(target),
                    others,
                    match_value,
                    value: Some(value),
                    entries: Vec::new(),
                }))
            }
            _ => {
                let suggestion = close_match(keyword, &BENCH_KEYWORDS);
                let mut error =
                    self.error_at("PAR002", format!("unknown declaration {}", repr(keyword)), self.previous(1));
                error.hint = suggestion.as_ref().map(|word| format!("did you mean {}?", repr(word)));
                error.replacement = suggestion;
                return Err(error);
            }
        })
    }

    /// A measure's limits block: `min = q;`, `max = q;` and at most one
    /// `where CONDITION and CONDITION;` (comparisons of params).
    fn measure_limits(&mut self) -> Result<(Vec<SimEntry>, Vec<Expr>)> {
        self.expect_symbol("{")?;
        let mut entries: Vec<SimEntry> = Vec::new();
        let mut conditions: Vec<Expr> = Vec::new();
        let mut scoped = false;
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let condition = token.kind == TokenKind::Identifier
                && token.text == "where"
                && !self
                    .tokens
                    .get(self.index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Symbol && next.text == "=");
            if condition {
                if scoped {
                    return Err(self.error_at(
                        "SIM072",
                        "a measure has one where; join its conditions with and",
                        &token,
                    ));
                }
                scoped = true;
                self.advance();
                conditions.push(self.comparison()?);
                while self.current().kind == TokenKind::Identifier && self.current().text == "and" {
                    self.advance();
                    conditions.push(self.comparison()?);
                }
                self.expect_symbol(";")?;
                continue;
            }
            let key = self.name("property name")?;
            if entries.iter().any(|entry| entry.key == key) {
                return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &token));
            }
            self.expect_symbol("=")?;
            let value = SimValue::Scalar(self.scalar()?);
            self.expect_symbol(";")?;
            entries.push(SimEntry { location: token.location, key, value });
        }
        self.expect_symbol("}")?;
        Ok((entries, conditions))
    }

    /// `comparison = expression [("<" | "<=" | ">" | ">=" | "==" | "!=") expression]`.
    pub(super) fn comparison(&mut self) -> Result<Expr> {
        let left = self.expression()?;
        let token = self.current().clone();
        if token.kind != TokenKind::Symbol || !["<", ">", "=", "!"].contains(&token.text.as_str()) {
            return Ok(left);
        }
        let joined = self.tokens.get(self.index + 1).is_some_and(|next| {
            next.kind == TokenKind::Symbol && next.text == "=" && next.location.offset == token.end_offset
        });
        let operator = match (token.text.as_str(), joined) {
            ("<", false) => CompareOperator::Less,
            ("<", true) => CompareOperator::LessEqual,
            (">", false) => CompareOperator::Greater,
            (">", true) => CompareOperator::GreaterEqual,
            ("=", true) => CompareOperator::Equal,
            ("!", true) => CompareOperator::NotEqual,
            _ => return Ok(left),
        };
        self.advance();
        if joined {
            self.advance();
        }
        let right = self.expression()?;
        Ok(Expr {
            location: token.location,
            kind: ExprKind::Compare { operator, left: Box::new(left), right: Box::new(right) },
        })
    }

    /// The properties of `analysis noise { ... }`: like [`Self::sim_entries`],
    /// except that `output` takes a probe, `output = V(OUT);` or
    /// `output = V(P, N);`, kept as a list of its node names; several probes
    /// (`output = V(A), V(B, C);`) are kept as a block of kind `outputs`
    /// with one `V` entry per probe.
    fn noise_entries(&mut self) -> Result<Vec<SimEntry>> {
        self.expect_symbol("{")?;
        let mut entries: Vec<SimEntry> = Vec::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let key = self.name("property name")?;
            if entries.iter().any(|entry| entry.key == key) {
                return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &token));
            }
            self.expect_symbol("=")?;
            let probe = self.current().kind == TokenKind::Identifier
                && self.current().text == "V"
                && self
                    .tokens
                    .get(self.index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Symbol && next.text == "(");
            let value = if probe {
                let mut probes: Vec<SimEntry> = Vec::new();
                loop {
                    let at = self.current().location.clone();
                    self.expect_word("V")?;
                    self.expect_symbol("(")?;
                    let mut nodes = vec![Scalar::Str(self.qualified_name("node name")?)];
                    while self.accept_symbol(",") {
                        nodes.push(Scalar::Str(self.qualified_name("node name")?));
                    }
                    self.expect_symbol(")")?;
                    probes.push(SimEntry { location: at, key: "V".to_owned(), value: SimValue::List(nodes) });
                    if !self.accept_symbol(",") {
                        break;
                    }
                }
                if probes.len() == 1 {
                    probes.remove(0).value
                } else {
                    SimValue::Block { kind: "outputs".to_owned(), entries: probes }
                }
            } else {
                SimValue::Scalar(self.scalar()?)
            };
            self.expect_symbol(";")?;
            entries.push(SimEntry { location: token.location, key, value });
        }
        self.expect_symbol("}")?;
        Ok(entries)
    }

    /// A bench net: its name may be a board pin (`net E_HC.1 { ... }`).
    fn bench_net(&mut self) -> Result<NetDecl> {
        let location = self.current().location.clone();
        let mut name = self.name("net name")?;
        if self.accept_symbol(".") {
            name = format!("{name}.{}", self.qualified_name("pin name")?);
        }
        self.expect_symbol("{")?;
        let mut endpoints = Vec::new();
        let mut endpoint_locations = Vec::new();
        while !self.at_symbol("}") {
            endpoint_locations.push(self.current().location.clone());
            endpoints.push(self.net_endpoint()?);
            self.expect_symbol(";")?;
        }
        self.expect_symbol("}")?;
        Ok(NetDecl { location, name, endpoints, endpoint_locations })
    }

    /// `expression = term (("+" | "-") term)*`.
    pub(super) fn expression(&mut self) -> Result<Expr> {
        let mut left = self.term()?;
        while self.at_symbol("+") || self.at_symbol("-") {
            let token = self.advance();
            let operator = if token.text == "+" { BinaryOperator::Add } else { BinaryOperator::Subtract };
            let right = self.term()?;
            left = Expr {
                location: token.location,
                kind: ExprKind::Binary { operator, left: Box::new(left), right: Box::new(right) },
            };
        }
        Ok(left)
    }

    /// `term = factor (("*" | "/") factor)*`.
    fn term(&mut self) -> Result<Expr> {
        let mut left = self.factor()?;
        while self.at_symbol("*") || self.at_symbol("/") {
            let token = self.advance();
            let operator = if token.text == "*" { BinaryOperator::Multiply } else { BinaryOperator::Divide };
            let right = self.factor()?;
            left = Expr {
                location: token.location,
                kind: ExprKind::Binary { operator, left: Box::new(left), right: Box::new(right) },
            };
        }
        Ok(left)
    }

    /// `factor = ("-" | "+") factor | primary`.
    fn factor(&mut self) -> Result<Expr> {
        if self.at_symbol("-") || self.at_symbol("+") {
            let token = self.advance();
            let operand = self.factor()?;
            if token.text == "+" {
                return Ok(operand);
            }
            return Ok(Expr { location: token.location, kind: ExprKind::Negate(Box::new(operand)) });
        }
        self.primary()
    }

    /// `primary = number [unit] | "(" expression ")" | name "(" arguments ")" | name`.
    fn primary(&mut self) -> Result<Expr> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Number => {
                self.advance();
                let kind = if self.at_unit() {
                    ExprKind::Quantity { value: token.text.clone(), unit: self.quantity_unit() }
                } else {
                    ExprKind::Number(token.text.clone())
                };
                Ok(Expr { location: token.location, kind })
            }
            TokenKind::Symbol if token.text == "(" => {
                self.advance();
                let inner = self.expression()?;
                self.expect_symbol(")")?;
                Ok(inner)
            }
            TokenKind::Identifier => {
                self.advance();
                if !self.accept_symbol("(") {
                    return Ok(Expr { location: token.location, kind: ExprKind::Name(token.text) });
                }
                if token.text == "V" || token.text == "I" {
                    return self.probe(token);
                }
                // Arguments may be comparisons (`upto(mag(h) > 0.707)`); the checker says where.
                let mut arguments = Vec::new();
                if !self.at_symbol(")") {
                    arguments.push(self.comparison()?);
                    while self.accept_symbol(",") {
                        arguments.push(self.comparison()?);
                    }
                }
                self.expect_symbol(")")?;
                Ok(Expr { location: token.location, kind: ExprKind::Call { function: token.text, arguments } })
            }
            _ => Err(self.error("SIM050", format!("expected an expression, found {}", Self::describe(self.current())))),
        }
    }

    /// The targets of `V(` or `I(` (read): node or component names, `X.PIN` allowed.
    fn probe(&mut self, function: crate::lexer::Token) -> Result<Expr> {
        let mut targets = Vec::new();
        let mut target_locations = Vec::new();
        loop {
            target_locations.push(self.current().location.clone());
            targets.push(self.qualified_name("probe target")?);
            if !self.accept_symbol(",") {
                break;
            }
        }
        self.expect_symbol(")")?;
        let limit = if function.text == "V" { 2 } else { 1 };
        if targets.len() > limit {
            return Err(self.error_at(
                "SIM051",
                format!(
                    "{}(...) takes {}",
                    function.text,
                    if limit == 2 { "a node, or two nodes" } else { "one component or COMPONENT.PIN" }
                ),
                &function,
            ));
        }
        Ok(Expr {
            location: function.location,
            kind: ExprKind::Probe { function: function.text, targets, target_locations },
        })
    }
}
