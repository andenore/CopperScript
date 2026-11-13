//! The board `build` block: typed build settings and the command line
//! options they stand for.
//!
//! A setting is a default for one existing option of the physical commands
//! (`route-board`, `plan-layout`, ...). The commands read the block from the
//! board source and put its options before their own arguments, so an option
//! given on the command line overrides the source.
//!
//! The parser checks group and key names against [`KEYS`]; [`lower_build`]
//! checks the values. The `router` group is open: its keys are fields of the
//! unified router, which this crate does not know, so the command line tools
//! check them (`copper check`, `copper build`, `copper route-board`).

use copper_json::{Object, Value, python_str_repr as repr};
use copper_units::{Decimal, Dimension, Quantity};

use crate::error::CopperScriptError;
use crate::suggest::close_match;
use crate::syntax::{BuildDecl, BuildSettingDecl, Declaration, Document, Location, Scalar};

type Result<T> = std::result::Result<T, CopperScriptError>;

/// The sub-blocks of a `build` block.
pub const GROUPS: [&str; 4] = ["placement", "routing", "planes", "router"];

/// The open group whose keys are unified router fields.
pub const ROUTER_GROUP: &str = "router";

/// The fabrication presets the physical commands accept.
pub const FAB_PROFILES: [&str; 3] = ["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"];

/// The values of `routing.effort`.
pub const EFFORTS: [&str; 3] = ["quick", "balanced", "thorough"];

/// The type of a setting's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An integer of at least `minimum`.
    Integer {
        minimum: i64,
    },
    /// `true` or `false`. `negative` is the option `false` stands for; without
    /// one, `false` adds no option.
    Boolean {
        negative: Option<&'static str>,
    },
    /// A length with a unit; zero is allowed unless `positive`.
    Length {
        positive: bool,
    },
    /// A positive duration in whole seconds.
    Duration,
    Text,
    Choice(&'static [&'static str]),
}

/// One typed setting: where it is written and the option it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    /// The sub-block, empty at the top level of the `build` block.
    pub group: &'static str,
    pub key: &'static str,
    pub kind: Kind,
    /// The command line option (for a boolean, the option `true` stands for).
    pub option: &'static str,
}

const fn key(group: &'static str, key: &'static str, kind: Kind, option: &'static str) -> KeySpec {
    KeySpec { group, key, kind, option }
}

const COUNT: Kind = Kind::Integer { minimum: 0 };
const AT_LEAST_ONE: Kind = Kind::Integer { minimum: 1 };
const POSITIVE_LENGTH: Kind = Kind::Length { positive: true };
const LENGTH: Kind = Kind::Length { positive: false };

const fn switch(negative: &'static str) -> Kind {
    Kind::Boolean { negative: Some(negative) }
}

/// Every typed setting, in the order the documentation lists them. The open
/// `router` group is not listed: each of its settings is `--unified-set KEY=VALUE`.
pub const KEYS: [KeySpec; 28] = [
    key("", "fab_profile", Kind::Choice(&FAB_PROFILES), "--fab-profile"),
    key("placement", "candidates", AT_LEAST_ONE, "--candidates"),
    key("placement", "candidate", Kind::Text, "--placement-candidate"),
    key("placement", "grid", POSITIVE_LENGTH, "--placement-grid-mm"),
    key("placement", "decoupling_trials", COUNT, "--decoupling-placement-trials"),
    key("placement", "refinement_passes", COUNT, "--placement-refinement-passes"),
    key("placement", "escape_spacing_passes", COUNT, "--escape-spacing-passes"),
    key("placement", "feedback_trials", COUNT, "--placement-feedback-trials"),
    key("routing", "effort", Kind::Choice(&EFFORTS), "--routing-profile"),
    key("routing", "feedback_iterations", AT_LEAST_ONE, "--feedback-iterations"),
    key("routing", "router_iterations", AT_LEAST_ONE, "--router-iterations"),
    key("routing", "fanout", switch("--no-fanout"), "--fanout"),
    key("routing", "fanout_maze", Kind::Boolean { negative: None }, "--fanout-maze"),
    key("routing", "array_flow", switch("--no-array-flow"), "--array-flow"),
    key("routing", "via_lattice", switch("--no-via-lattice"), "--via-lattice"),
    key("routing", "congestion_feedback_trials", COUNT, "--congestion-feedback-trials"),
    key("routing", "filled_copper_trials", COUNT, "--filled-copper-trials"),
    key("routing", "pair_states", COUNT, "--package-initial-pair-states"),
    key("routing", "pitch", LENGTH, "--unified-pitch-mm"),
    key("routing", "time_budget", Kind::Duration, "--unified-time-budget-s"),
    key("planes", "early_stitch", switch("--no-early-plane-stitch"), "--early-plane-stitch"),
    key("planes", "surface_zones", switch("--no-stitch-surface-zones"), "--stitch-surface-zones"),
    key("planes", "stitch_step", POSITIVE_LENGTH, "--plane-stitch-step-mm"),
    key("planes", "stitch_radius", POSITIVE_LENGTH, "--plane-stitch-radius-mm"),
    key("planes", "stitch_detour", LENGTH, "--plane-stitch-detour-mm"),
    key("planes", "contact_radius", LENGTH, "--plane-contact-radius-mm"),
    key("planes", "maze_step", POSITIVE_LENGTH, "--plane-maze-step-mm"),
    key("planes", "maze_budget", AT_LEAST_ONE, "--plane-maze-budget"),
];

/// The option every `router` setting stands for.
pub const ROUTER_OPTION: &str = "--unified-set";

/// The typed keys of `group` (empty: the top level). The open `router` group has none.
pub fn keys(group: &str) -> Vec<&'static str> {
    KEYS.iter().filter(|spec| spec.group == group).map(|spec| spec.key).collect()
}

/// `group.key`, or `key` at the top level: how messages name a setting.
pub fn dotted(group: &str, key: &str) -> String {
    if group.is_empty() { key.to_owned() } else { format!("{group}.{key}") }
}

/// A checked value.
#[derive(Clone, Debug, PartialEq)]
pub enum BuildValue {
    Integer(i64),
    Boolean(bool),
    /// A length in nanometres.
    LengthNm(i64),
    /// A duration in seconds.
    Seconds(i64),
    Text(String),
    /// A `router` value with a decimal point.
    Number(f64),
}

/// One checked setting of a `build` block.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildSetting {
    /// The sub-block, empty at the top level.
    pub group: String,
    pub key: String,
    pub value: BuildValue,
    /// The key token.
    pub location: Location,
}

impl BuildSetting {
    /// `group.key`, or `key` at the top level.
    pub fn name(&self) -> String {
        dotted(&self.group, &self.key)
    }
}

/// The checked `build` block of a board, in source order.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildSettings {
    pub location: Location,
    pub settings: Vec<BuildSetting>,
}

/// The command line form of one setting: the option name and its tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceOption {
    /// The option the tokens start with (`--fanout`, `--no-fanout`, `--candidates`).
    pub option: String,
    /// The option and its value, as separate arguments.
    pub tokens: Vec<String>,
}

/// A length in nanometres as millimetre decimal text (`250000` is `0.25`).
pub fn mm_text(nm: i64) -> String {
    let scale = Decimal::parse("0.000001").expect("a valid decimal");
    Decimal::from_i64(nm).mul(&scale).map_or_else(|_| nm.to_string(), |mm| mm.decimal_text())
}

fn value_text(value: &BuildValue) -> String {
    match value {
        BuildValue::Integer(number) | BuildValue::Seconds(number) => number.to_string(),
        BuildValue::Boolean(flag) => flag.to_string(),
        BuildValue::LengthNm(nm) => mm_text(*nm),
        BuildValue::Text(text) => text.clone(),
        BuildValue::Number(number) => number.to_string(),
    }
}

impl BuildSettings {
    /// The settings of the open `router` group.
    pub fn router(&self) -> impl Iterator<Item = &BuildSetting> {
        self.settings.iter().filter(|setting| setting.group == ROUTER_GROUP)
    }

    /// The options the settings stand for, in source order. A boolean without
    /// a negative option adds nothing when `false`.
    pub fn options(&self) -> Vec<SourceOption> {
        let mut options = Vec::new();
        for setting in &self.settings {
            if setting.group == ROUTER_GROUP {
                options.push(SourceOption {
                    option: ROUTER_OPTION.to_owned(),
                    tokens: vec![ROUTER_OPTION.to_owned(), format!("{}={}", setting.key, value_text(&setting.value))],
                });
                continue;
            }
            let Some(spec) = KEYS.iter().find(|spec| spec.group == setting.group && spec.key == setting.key) else {
                continue;
            };
            match (&setting.value, spec.kind) {
                (BuildValue::Boolean(true), _) => {
                    options.push(SourceOption { option: spec.option.to_owned(), tokens: vec![spec.option.to_owned()] });
                }
                (BuildValue::Boolean(false), Kind::Boolean { negative: Some(negative) }) => {
                    options.push(SourceOption { option: negative.to_owned(), tokens: vec![negative.to_owned()] });
                }
                (BuildValue::Boolean(false), _) => {}
                (value, _) => options.push(SourceOption {
                    option: spec.option.to_owned(),
                    tokens: vec![spec.option.to_owned(), value_text(value)],
                }),
            }
        }
        options
    }

    /// The `build` member of the compiled design: top-level settings and one
    /// object per group. Lengths are integer nanometres under `KEY_nm`,
    /// durations integer seconds under `KEY_s`.
    pub fn document(&self) -> Value {
        let mut top = Object::new();
        let mut groups: Vec<(&str, Object)> = Vec::new();
        for setting in &self.settings {
            let (name, value) = match &setting.value {
                BuildValue::Integer(number) => (setting.key.clone(), Value::from(*number)),
                BuildValue::Boolean(flag) => (setting.key.clone(), Value::from(*flag)),
                BuildValue::LengthNm(nm) => (format!("{}_nm", setting.key), Value::from(*nm)),
                BuildValue::Seconds(seconds) => (format!("{}_s", setting.key), Value::from(*seconds)),
                BuildValue::Text(text) => (setting.key.clone(), Value::from(text.as_str())),
                BuildValue::Number(number) => (setting.key.clone(), Value::from(*number)),
            };
            if setting.group.is_empty() {
                top.insert(name, value);
            } else if let Some((_, members)) = groups.iter_mut().find(|(group, _)| *group == setting.group) {
                members.insert(name, value);
            } else {
                let mut members = Object::new();
                members.insert(name, value);
                groups.push((&setting.group, members));
            }
        }
        for (group, members) in groups {
            top.insert(group.to_owned(), Value::Object(members));
        }
        Value::Object(top)
    }
}

fn wrong_type(setting: &BuildSettingDecl, expected: &str) -> CopperScriptError {
    CopperScriptError::new(
        "BLD001",
        format!("build setting {} must be {expected}", repr(&dotted(&setting.group, &setting.key))),
        setting.location.clone(),
    )
}

fn out_of_range(setting: &BuildSettingDecl, expected: &str) -> CopperScriptError {
    CopperScriptError::new(
        "BLD002",
        format!("build setting {} must {expected}", repr(&dotted(&setting.group, &setting.key))),
        setting.location.clone(),
    )
}

fn integer(setting: &BuildSettingDecl, minimum: i64) -> Result<BuildValue> {
    let Scalar::Int(number) = &setting.value else { return Err(wrong_type(setting, "an integer")) };
    match i64::try_from(number) {
        Ok(number) if number >= minimum => Ok(BuildValue::Integer(number)),
        Ok(_) if minimum == 0 => Err(out_of_range(setting, "not be negative")),
        Ok(_) => Err(out_of_range(setting, &format!("be at least {minimum}"))),
        Err(_) if number.sign() == num_bigint::Sign::Minus => Err(out_of_range(setting, "not be negative")),
        Err(_) => Err(out_of_range(setting, "fit in 64 bits")),
    }
}

/// The base value (metres or seconds) of a quantity of `dimension`.
fn quantity(setting: &BuildSettingDecl, dimension: Dimension, expected: &str) -> Result<Decimal> {
    let Scalar::Quantity { value, unit } = &setting.value else { return Err(wrong_type(setting, expected)) };
    let number = Decimal::parse(value).map_err(|_| wrong_type(setting, expected))?;
    let quantity = Quantity::of(dimension, &number, unit)
        .map_err(|failure| wrong_type(setting, &format!("{expected} ({failure})")))?;
    Ok(quantity.base_value().clone())
}

fn length(setting: &BuildSettingDecl, positive: bool) -> Result<BuildValue> {
    let metres = quantity(setting, Dimension::Length, "a length with a unit, such as 0.5mm")?;
    let nm = metres
        .mul(&Decimal::from_i64(1_000_000_000))
        .map_err(|_| out_of_range(setting, "be an exact integer number of nanometres"))?;
    if !nm.is_integral() {
        return Err(out_of_range(setting, "be an exact integer number of nanometres"));
    }
    if nm < Decimal::zero() {
        return Err(out_of_range(setting, if positive { "be positive" } else { "not be negative" }));
    }
    if positive && nm.is_zero() {
        return Err(out_of_range(setting, "be positive"));
    }
    match nm.to_i64().filter(|nm| *nm <= copper_physical::MAX_COORDINATE) {
        Some(nm) => Ok(BuildValue::LengthNm(nm)),
        None => Err(out_of_range(setting, "be a board-sized length")),
    }
}

fn duration(setting: &BuildSettingDecl) -> Result<BuildValue> {
    let seconds = quantity(setting, Dimension::Time, "a duration with a unit, such as 600s")?;
    if !seconds.is_integral() {
        return Err(out_of_range(setting, "be a whole number of seconds"));
    }
    match seconds.to_i64() {
        Some(seconds) if seconds > 0 => Ok(BuildValue::Seconds(seconds)),
        _ => Err(out_of_range(setting, "be positive")),
    }
}

/// A `router` value: a plain number, a boolean or a name. The key and the
/// value's meaning belong to the unified router.
fn router_value(setting: &BuildSettingDecl) -> Result<BuildValue> {
    match &setting.value {
        Scalar::Str(text) => Ok(BuildValue::Text(text.clone())),
        Scalar::Bool(flag) => Ok(BuildValue::Boolean(*flag)),
        Scalar::Float(number) => Ok(BuildValue::Number(*number)),
        Scalar::Int(number) => {
            i64::try_from(number).map(BuildValue::Integer).map_err(|_| out_of_range(setting, "fit in 64 bits"))
        }
        Scalar::Quantity { .. } => Err(wrong_type(setting, "a number without a unit, true, false, a name or a string")),
    }
}

fn checked(setting: &BuildSettingDecl) -> Result<BuildValue> {
    if setting.group == ROUTER_GROUP {
        return router_value(setting);
    }
    let name = dotted(&setting.group, &setting.key);
    let Some(spec) = KEYS.iter().find(|spec| spec.group == setting.group && spec.key == setting.key) else {
        // The parser rejects these; a hand-built tree gets the same code.
        return Err(CopperScriptError::new(
            "PAR021",
            format!("unknown build setting {}", repr(&name)),
            setting.location.clone(),
        ));
    };
    match spec.kind {
        Kind::Integer { minimum } => integer(setting, minimum),
        Kind::Boolean { .. } => match &setting.value {
            Scalar::Bool(flag) => Ok(BuildValue::Boolean(*flag)),
            _ => Err(wrong_type(setting, "true or false")),
        },
        Kind::Length { positive } => length(setting, positive),
        Kind::Duration => duration(setting),
        Kind::Text => match &setting.value {
            Scalar::Str(text) if !text.is_empty() => Ok(BuildValue::Text(text.clone())),
            _ => Err(wrong_type(setting, "a non-empty string")),
        },
        Kind::Choice(allowed) => {
            let expected = format!("one of {}", allowed.join(", "));
            let Scalar::Str(text) = &setting.value else { return Err(wrong_type(setting, &expected)) };
            if allowed.contains(&text.as_str()) {
                return Ok(BuildValue::Text(text.clone()));
            }
            let mut error = wrong_type(setting, &expected);
            error.hint = close_match(text, allowed).map(|candidate| format!("did you mean {}?", repr(&candidate)));
            Err(error)
        }
    }
}

/// Check one `build` block.
pub fn lower_build_block(declaration: &BuildDecl) -> Result<BuildSettings> {
    let mut settings = Vec::new();
    for setting in &declaration.settings {
        settings.push(BuildSetting {
            group: setting.group.clone(),
            key: setting.key.clone(),
            value: checked(setting)?,
            location: setting.location.clone(),
        });
    }
    Ok(BuildSettings { location: declaration.location.clone(), settings })
}

/// The checked `build` block of `document`, `None` when it declares none.
pub fn lower_build(document: &Document) -> Result<Option<BuildSettings>> {
    document
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Build(block) => Some(lower_build_block(block)),
            _ => None,
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn lowered(body: &str) -> Result<Option<BuildSettings>> {
        lower_build(&parse(&format!("board B {{\n{body}\n}}"), "b.copper")?)
    }

    fn failure(body: &str) -> String {
        lowered(body).unwrap_err().to_string()
    }

    #[test]
    fn a_board_without_a_block_has_no_settings() {
        assert_eq!(lowered("").unwrap(), None);
    }

    #[test]
    fn settings_become_options_in_source_order() {
        let settings = lowered(
            "build {\n fab_profile = \"jlcpcb-six-layer\";\n placement { candidates = 2; candidate = \"candidate-01\"; \
             grid = 0.25mm; }\n routing { effort = balanced; fanout_maze = true; fanout = false; via_lattice = true; \
             time_budget = 600s; pitch = 100um; }\n planes { contact_radius = 3mm; stitch_step = 0.1mm; \
             early_stitch = false; }\n router { fine_regions = dense; maximum_expansions = 2000000; \
             present_factor = 0.5; buses_first = true; }\n}",
        )
        .unwrap()
        .unwrap();
        let tokens: Vec<String> = settings.options().into_iter().flat_map(|option| option.tokens).collect();
        assert_eq!(
            tokens.join(" "),
            "--fab-profile jlcpcb-six-layer --candidates 2 --placement-candidate candidate-01 \
             --placement-grid-mm 0.25 --routing-profile balanced --fanout-maze --no-fanout --via-lattice \
             --unified-time-budget-s 600 --unified-pitch-mm 0.1 --plane-contact-radius-mm 3 \
             --plane-stitch-step-mm 0.1 --no-early-plane-stitch --unified-set fine_regions=dense \
             --unified-set maximum_expansions=2000000 --unified-set present_factor=0.5 \
             --unified-set buses_first=true"
        );
        assert_eq!(settings.router().count(), 4);
        assert_eq!(settings.settings[1].name(), "placement.candidates");
        assert_eq!(settings.settings[1].location.line, 4);
    }

    #[test]
    fn a_false_switch_without_a_negative_option_adds_nothing() {
        let settings = lowered("build { routing { fanout_maze = false; } }").unwrap().unwrap();
        assert!(settings.options().is_empty());
    }

    #[test]
    fn the_compiled_form_names_units() {
        let settings = lowered(
            "build { fab_profile = generic; placement { grid = 0.25mm; candidates = 3; } \
             routing { time_budget = 2s; fanout = true; } router { direct = true; } }",
        )
        .unwrap()
        .unwrap();
        let text = copper_json::Dumps::new().sort_keys(true).dumps(&settings.document());
        assert_eq!(
            text,
            r#"{"fab_profile": "generic", "placement": {"candidates": 3, "grid_nm": 250000}, "router": {"direct": true}, "routing": {"fanout": true, "time_budget_s": 2}}"#
        );
    }

    #[test]
    fn wrong_values_are_located_at_their_key() {
        let cases = [
            ("placement { candidates = two; }", "BLD001: build setting 'placement.candidates' must be an integer"),
            ("placement { candidates = 0; }", "BLD002: build setting 'placement.candidates' must be at least 1"),
            (
                "placement { feedback_trials = -1; }",
                "BLD002: build setting 'placement.feedback_trials' must not be negative",
            ),
            ("placement { candidates = 1.5; }", "BLD001: build setting 'placement.candidates' must be an integer"),
            ("routing { fanout = 1; }", "BLD001: build setting 'routing.fanout' must be true or false"),
            (
                "placement { grid = 0.25; }",
                "BLD001: build setting 'placement.grid' must be a length with a unit, such as 0.5mm",
            ),
            (
                "placement { grid = 2s; }",
                "BLD001: build setting 'placement.grid' must be a length with a unit, such as 0.5mm \
                 (unsupported Length unit 's'; use m, mm, um)",
            ),
            ("placement { grid = 0mm; }", "BLD002: build setting 'placement.grid' must be positive"),
            ("planes { contact_radius = -1mm; }", "BLD002: build setting 'planes.contact_radius' must not be negative"),
            (
                "planes { stitch_step = 0.0000001mm; }",
                "BLD002: build setting 'planes.stitch_step' must be an exact integer number of nanometres",
            ),
            (
                "routing { time_budget = 600; }",
                "BLD001: build setting 'routing.time_budget' must be a duration with a unit, such as 600s",
            ),
            (
                "routing { time_budget = 1500ms; }",
                "BLD002: build setting 'routing.time_budget' must be a whole number of seconds",
            ),
            ("routing { time_budget = 0s; }", "BLD002: build setting 'routing.time_budget' must be positive"),
            (
                "routing { effort = fast; }",
                "BLD001: build setting 'routing.effort' must be one of quick, balanced, thorough",
            ),
            (
                "routing { effort = balancd; }",
                "BLD001: build setting 'routing.effort' must be one of quick, balanced, thorough \
                 (did you mean 'balanced'?)",
            ),
            (
                "fab_profile = \"jlcpcb\";",
                "BLD001: build setting 'fab_profile' must be one of generic, jlcpcb-four-layer, jlcpcb-six-layer",
            ),
            ("placement { candidate = 3; }", "BLD001: build setting 'placement.candidate' must be a non-empty string"),
            (
                "router { pitch_nm = 0.1mm; }",
                "BLD001: build setting 'router.pitch_nm' must be a number without a unit, true, false, a name or a \
                 string",
            ),
        ];
        for (body, expected) in cases {
            // The setting is on line 3: `board`, `build {`, then the case.
            let message = failure(&format!("build {{\n{body}\n}}"));
            let column = if body.contains('{') { body.find('{').unwrap() + 3 } else { 1 };
            assert_eq!(message, format!("b.copper:3:{column}: {expected}"), "{body}");
        }
    }

    #[test]
    fn lengths_print_as_millimetres() {
        assert_eq!(mm_text(250_000), "0.25");
        assert_eq!(mm_text(3_000_000), "3");
        assert_eq!(mm_text(0), "0");
        assert_eq!(mm_text(1), "0.000001");
    }
}
