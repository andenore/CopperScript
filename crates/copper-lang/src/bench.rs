//! Simulation benches (`docs/language-reference.md`, "Simulation").
//!
//! A bench is its own document, `bench NAME for TARGET { ... }`, in a file of
//! the board's `benches/` directory. Its target is the board, or a module
//! the board's design contains, simulated on its own: the module's components
//! and nets under their local names, its ports as the nets they are on, with
//! the bench supplying them (`supply`, `model`). It adds simulation-only
//! components, nets, supplies and model instances to the target, swaps or
//! removes the target's components, names one analysis, steps params, and
//! declares typed `let`, `measure` and `plot` expressions. [`compile_bench`] checks everything decidable without
//! simulating and returns a [`Bench`] of plain data for a simulator lowering
//! step (`copper-simulate`, behind `copper sim`). Nothing here simulates.

use std::path::{Path, PathBuf};

use copper_json::{Dumps, Value, python_str_repr as repr};
use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;

use crate::compiler::{Design, package_sources, read_source};
use crate::elaborate::elaborate;
use crate::error::CopperScriptError;
use crate::expression::{self, Expression, ExpressionKind, ExpressionType, Node, Probe, Scope, Units};
use crate::model::*;
use crate::parser::parse;
use crate::paths;
use crate::serializer::quantity_value;
use crate::simulation::{self, PartSimModel, SimKind, SimQuantity, SimSpec, ValueScope, Waveform};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

/// The `schema` of [`bench_value`].
pub const SCHEMA: &str = "copperscript-bench/v0.1";

/// The directory beside a board file that holds its benches.
pub const BENCH_DIRECTORY: &str = "benches";

/// The pin name of `I(SUPPLY)` in a probe of supply `NAME` (component `supply NAME`):
/// the current the supply delivers into its net.
pub const SUPPLY_NET: &str = "net";

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

fn suggesting(mut failure: CopperScriptError, word: &str, candidates: &[&str]) -> CopperScriptError {
    if let Some(suggestion) = close_match(word, candidates) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    failure
}

/// Where an effective param's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamOrigin {
    Board,
    /// A bench `param` that overrides the board's param of the same name.
    BenchOverride,
    Bench,
}

impl ParamOrigin {
    pub fn name(self) -> &'static str {
        match self {
            ParamOrigin::Board => "board",
            ParamOrigin::BenchOverride => "bench_override",
            ParamOrigin::Bench => "bench",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchParam {
    /// The nominal value; for a derived param, its expression at the other params' nominal values.
    pub value: Quantity,
    pub origin: ParamOrigin,
    pub location: Location,
    /// A derived param's expression of earlier params and literals, which a
    /// simulator re-evaluates at every step: `param R_C = R_T * 7;`.
    pub derived: Option<Expression>,
}

/// An ideal two-terminal element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveKind {
    Resistor,
    Capacitor,
    Inductor,
}

impl PassiveKind {
    pub fn name(self) -> &'static str {
        match self {
            PassiveKind::Resistor => "resistor",
            PassiveKind::Capacitor => "capacitor",
            PassiveKind::Inductor => "inductor",
        }
    }

    pub fn dimension(self) -> Dimension {
        match self {
            PassiveKind::Resistor => Dimension::Resistance,
            PassiveKind::Capacitor => Dimension::Capacitance,
            PassiveKind::Inductor => Dimension::Inductance,
        }
    }

    /// A part's passive kind, from its category (`passive.resistor...`).
    pub fn of(part: &PartDefinition) -> Option<PassiveKind> {
        let category = part.category.as_str();
        let is = |prefix: &str| category == prefix || category.starts_with(&format!("{prefix}."));
        if is("passive.resistor") {
            Some(PassiveKind::Resistor)
        } else if is("passive.capacitor") {
            Some(PassiveKind::Capacitor)
        } else if is("passive.inductor") {
            Some(PassiveKind::Inductor)
        } else {
            None
        }
    }
}

/// How the bench simulates one component.
#[derive(Clone, Debug, PartialEq)]
pub enum Treatment {
    /// An ideal resistor, capacitor or inductor between the part's two pins,
    /// with the component's temperature coefficients and the part's parasitics.
    Passive { kind: PassiveKind, value: SimQuantity, extras: Box<PassiveExtras> },
    /// The part's own `sim` blocks ([`PartDefinition::sim`]).
    PartModel,
    /// Swapped for a built-in model: the index into [`Bench::replacements`].
    Replaced(usize),
    /// Removed; its pins are open.
    Ignored,
}

impl Treatment {
    pub fn name(&self) -> &'static str {
        match self {
            Treatment::Passive { .. } => "passive",
            Treatment::PartModel => "part_model",
            Treatment::Replaced(_) => "replaced",
            Treatment::Ignored => "ignored",
        }
    }
}

/// What a passive carries beyond its value: the component's temperature
/// coefficients (`tempco`, `tempco2`) and, from its part's EMI facts, a
/// capacitor's `esr`, `esl` (or `self_resonance`) and an inductor's `esr`
/// (its DC resistance).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PassiveExtras {
    /// The linear temperature coefficient (`ppm/degC`), from 27 degC.
    pub tempco: Option<Quantity>,
    /// The quadratic temperature coefficient (`ppm/degC2`), from 27 degC.
    pub tempco2: Option<Quantity>,
    /// A capacitor's equivalent series resistance, an inductor's DC resistance.
    pub esr: Option<Quantity>,
    /// A capacitor's equivalent series inductance.
    pub esl: Option<Quantity>,
    /// A capacitor's self-resonance, which gives its ESL when `esl` is not declared.
    pub self_resonance: Option<Quantity>,
}

impl PassiveExtras {
    /// The extras of `component` (a `kind`) on `part`: the parasitics a kind does not have are left out.
    pub fn of(kind: PassiveKind, component: &ComponentInstance, part: &PartDefinition) -> PassiveExtras {
        let stored = |key: &str| {
            component.properties.get(key).and_then(|text| {
                let (number, unit) = text.rsplit_once(' ')?;
                expression::sim_quantity(number, unit)
            })
        };
        let emi = part.emi.as_ref();
        let (esr, esl, self_resonance) = match kind {
            PassiveKind::Capacitor => (
                emi.and_then(|e| e.esr.clone()),
                emi.and_then(|e| e.esl.clone()),
                emi.and_then(|e| e.self_resonance.clone()),
            ),
            // An inductor's `esr` is its DC resistance, as `copper emi-check` reads it.
            PassiveKind::Inductor => (emi.and_then(|e| e.esr.clone()), None, None),
            PassiveKind::Resistor => (None, None, None),
        };
        PassiveExtras {
            tempco: stored(crate::tolerance::TEMPCO),
            tempco2: stored(crate::tolerance::TEMPCO2),
            esr: esr.filter(|r| !r.base_value().is_zero()),
            esl: esl.filter(|l| !l.base_value().is_zero()),
            self_resonance,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == PassiveExtras::default()
    }
}

/// Model fidelity, as every simulation result must name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    /// An ideal element (passives).
    Ideal,
    /// A built-in parameter model (`sim` blocks and `replace`).
    Macro,
}

impl Fidelity {
    pub fn name(self) -> &'static str {
        match self {
            Fidelity::Ideal => "ideal",
            Fidelity::Macro => "macro",
        }
    }
}

/// One component of the simulated circuit: every flat board component, then
/// every bench component, in source order.
#[derive(Clone, Debug, PartialEq)]
pub struct SimComponent {
    pub reference: String,
    /// The qualified part name; its definition is in the board's or the bench's library.
    pub part: String,
    /// `true` for a bench `component`.
    pub bench: bool,
    pub treatment: Treatment,
}

impl SimComponent {
    pub fn fidelity(&self) -> Option<Fidelity> {
        match self.treatment {
            Treatment::Passive { .. } => Some(Fidelity::Ideal),
            Treatment::PartModel | Treatment::Replaced(_) => Some(Fidelity::Macro),
            Treatment::Ignored => None,
        }
    }
}

/// What a bench net's name joins.
#[derive(Clone, Debug, PartialEq)]
pub enum NetJoin {
    /// The board net of that name, or of the board pin the name gives.
    BoardNet(String),
    /// A board pin that no board net connects.
    BoardPin(Endpoint),
    /// A simulation-only net.
    New,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchNet {
    /// The name as written (`E_HC.1`, `GND`, `VSRC`).
    pub name: String,
    pub location: Location,
    pub join: NetJoin,
    /// The circuit node the net is.
    pub node: Node,
    /// Bench component pins on the net.
    pub endpoints: Vec<Endpoint>,
}

/// How the bench simulates a board `supply`.
#[derive(Clone, Debug, PartialEq)]
pub enum SupplyTreatment {
    /// The 0 V supply's net: the simulation ground.
    Ground,
    /// An ideal DC voltage source from the supply's net to ground (an AC short).
    DcSource,
    /// Driven by the simulated component the supply names as its `source`.
    Component(String),
    /// Swapped for a built-in model: the index into [`Bench::replacements`].
    Replaced(usize),
    /// Removed; the supply does not drive its net.
    Ignored,
}

impl SupplyTreatment {
    pub fn name(&self) -> &'static str {
        match self {
            SupplyTreatment::Ground => "ground",
            SupplyTreatment::DcSource => "dc_source",
            SupplyTreatment::Component(_) => "component",
            SupplyTreatment::Replaced(_) => "replaced",
            SupplyTreatment::Ignored => "ignored",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimSupply {
    pub name: String,
    pub net: String,
    pub voltage: Quantity,
    pub treatment: SupplyTreatment,
    /// `true` for a bench `supply` (an ideal DC source the bench adds, for
    /// example on a module port); `false` for a supply of the target.
    pub bench: bool,
}

/// `model NAME: KIND { ... }`: a built-in model instance the bench adds,
/// on target or bench nodes (a stimulus on a module port, a load).
#[derive(Clone, Debug, PartialEq)]
pub struct BenchModel {
    pub name: String,
    pub location: Location,
    /// Role -> node, in role order.
    pub nodes: IndexMap<String, Node>,
    pub spec: SimSpec,
}

/// `replace X with KIND { ... }`: X is a board component or a board supply.
#[derive(Clone, Debug, PartialEq)]
pub struct Replacement {
    /// The replaced component reference or supply name.
    pub target: String,
    /// `true` when `target` is a supply.
    pub supply: bool,
    pub location: Location,
    /// Role -> node, in role order.
    pub nodes: IndexMap<String, Node>,
    pub spec: SimSpec,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Analysis {
    /// `analysis op;`: the operating point.
    Op,
    /// `analysis ac { from; to; points_per_decade; }`: a logarithmic small-signal sweep.
    Ac { from: SimQuantity, to: SimQuantity, points_per_decade: u32 },
    /// `analysis noise { from; to; points_per_decade; output = V(...); input; temperature; }`.
    Noise(Box<crate::noise::NoiseAnalysis>),
    /// `analysis tran { stop; start; max_step; reltol; uic; }`: the response in time.
    Tran(Box<crate::transient::TranAnalysis>),
    /// `analysis dc { sweep; from; to; step | points; }`: operating points over a swept value.
    Dc(Box<DcAnalysis>),
    /// `analysis loop { from; to; points_per_decade; probe = COMPONENT.PIN; }`: the loop gain at a pin.
    Loop(Box<crate::stability::LoopAnalysis>),
    /// `analysis pz;` or `analysis pz { input; output = V(...); }`: poles and zeros.
    Pz(Box<crate::stability::PzAnalysis>),
}

impl Analysis {
    pub fn name(&self) -> &'static str {
        match self {
            Analysis::Op => "op",
            Analysis::Ac { .. } => "ac",
            Analysis::Noise(_) => "noise",
            Analysis::Tran(_) => "tran",
            Analysis::Dc(_) => "dc",
            Analysis::Loop(_) => "loop",
            Analysis::Pz(_) => "pz",
        }
    }
}

/// What `analysis dc` sweeps.
#[derive(Clone, Debug, PartialEq)]
pub enum DcSweep {
    /// A param (not a derived one, not a stepped one); derived params follow it.
    Param(String),
    /// The `dc` value of a source model: a bench `model` of kind source or a
    /// component replaced with one, by its simulator instance name.
    Source(String),
    /// The voltage of a supply simulated as its ideal DC source.
    Supply(String),
    /// The `dc` value of a supply replaced with a source model.
    ReplacedSupply(String),
}

/// `analysis dc { sweep = NAME; from = q; to = q; step = q; }` (or `points = N`).
#[derive(Clone, Debug, PartialEq)]
pub struct DcAnalysis {
    /// The swept name as written.
    pub name: String,
    pub sweep: DcSweep,
    /// The dimension of the swept value.
    pub dimension: Dimension,
    pub from: SimQuantity,
    pub to: SimQuantity,
    /// The spacing (positive; the sweep runs from `from` towards `to`), or `None` with `points`.
    pub step: Option<SimQuantity>,
    /// The number of evenly spaced points, ends included (2 to 100000), or `None` with `step`.
    pub points: Option<u32>,
}

/// What an initial condition of a transient run sets.
#[derive(Clone, Debug, PartialEq)]
pub enum InitialTarget {
    /// A node voltage against ground.
    Node(Node),
    /// A capacitor's voltage, its first pin against its second.
    Capacitor(String),
    /// An inductor's current, into its first pin.
    Inductor(String),
}

/// `ic V(NODE) = q;`, `ic C1 = q;` or `ic L1 = q;`.
#[derive(Clone, Debug, PartialEq)]
pub struct InitialCondition {
    pub target: InitialTarget,
    pub value: SimQuantity,
    pub location: Location,
}

/// `step PARAM in a, b, c;`. Several steps nest, the first outermost. A
/// zipped step (`step A, B in (a1, b1), (a2, b2);`) moves its params together.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub param: String,
    pub values: Vec<Quantity>,
    /// The other params of a zipped step, each with one value per entry of `values`.
    pub zip: Vec<(String, Vec<Quantity>)>,
    pub location: Location,
}

impl Step {
    /// The params this step sets, first one first.
    pub fn params(&self) -> Vec<&str> {
        std::iter::once(self.param.as_str()).chain(self.zip.iter().map(|(name, _)| name.as_str())).collect()
    }

    /// The (param, value) pairs of entry `index`.
    pub fn entry(&self, index: usize) -> Vec<(String, Quantity)> {
        std::iter::once((self.param.clone(), self.values[index].clone()))
            .chain(self.zip.iter().map(|(name, values)| (name.clone(), values[index].clone())))
            .collect()
    }
}

/// `reference NAME { PARAM = value; ... }`: the bench re-run with these
/// params (a calibration standard), read by `reference(x, NAME)`. Derived
/// params that are not overridden follow the overridden ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub name: String,
    pub overrides: Vec<(String, SimQuantity)>,
    pub location: Location,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Let {
    pub name: String,
    pub expression: Expression,
    pub location: Location,
    /// Whether the expression reads a reference run; such lets are not
    /// evaluated in reference runs themselves.
    pub uses_reference: bool,
}

/// `measure NAME = expr { min = ...; max = ...; }`: a real value per step,
/// failing the run when it leaves its limits.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub name: String,
    pub expression: Expression,
    pub min: Option<SimQuantity>,
    pub max: Option<SimQuantity>,
    /// `where` comparisons of params: the limits apply at the steps where
    /// all of them hold (empty: at every step).
    pub conditions: Vec<Expression>,
    pub location: Location,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plot {
    pub expressions: Vec<Expression>,
    pub location: Location,
}

/// A checked bench.
#[derive(Clone, Debug, PartialEq)]
pub struct Bench {
    pub name: String,
    /// The board whose design holds the target.
    pub board: String,
    /// For a bench of a module: the module definition's qualified name
    /// (`analog.AnalogFrontEnd`); `None` for a bench of the board.
    pub module: Option<String>,
    pub location: Location,
    /// The params the bench sees: the board's (with bench overrides), then the bench's own.
    pub params: IndexMap<String, BenchParam>,
    /// Bench components as compiled (their values with params substituted).
    pub bench_components: Vec<ComponentInstance>,
    /// Bench component -> attribute -> the param it names.
    pub bench_param_uses: IndexMap<String, IndexMap<String, String>>,
    /// Parts of bench components that the board's library does not have (from the bench's `use library`).
    pub library: IndexMap<String, PartDefinition>,
    /// Parts of the target's components that the board's own library does
    /// not have: the parts of module components.
    pub target_library: IndexMap<String, PartDefinition>,
    /// The whole simulated circuit's components and how each is simulated.
    pub components: Vec<SimComponent>,
    /// Every supply of the target, then every bench supply, and how each is simulated.
    pub supplies: Vec<SimSupply>,
    /// The ground net: the net of the board's 0 V supply.
    pub ground: String,
    pub nets: Vec<BenchNet>,
    pub replacements: Vec<Replacement>,
    /// Bench `model` instances, in source order.
    pub models: Vec<BenchModel>,
    pub analysis: Analysis,
    pub steps: Vec<Step>,
    pub references: Vec<Reference>,
    pub lets: Vec<Let>,
    pub measures: Vec<Measure>,
    pub plots: Vec<Plot>,
    /// `montecarlo` or `worstcase` with the tolerances it varies; `None` runs the nominal values only.
    pub variation: Option<crate::tolerance::Variation>,
    /// The initial conditions of `analysis tran`, in source order.
    pub initial_conditions: Vec<InitialCondition>,
    /// The fixtures the bench uses (`using A, B`, and the fixtures those use), in order.
    pub fixtures: Vec<String>,
    /// For one analysis of a bench with several: the analysis's name (the
    /// bench's `name` is then `BENCH.SECTION`).
    pub section: Option<String>,
    /// Which of a part's macro and vendor models run (`copper sim --models`);
    /// set by the runner, [`simulation::ModelChoice::Default`] when compiled.
    pub model_choice: simulation::ModelChoice,
}

impl Bench {
    /// The name of the bench as declared: `name` without its section.
    pub fn bench_name(&self) -> &str {
        match &self.section {
            Some(section) => self.name.strip_suffix(&format!(".{section}")).unwrap_or(&self.name),
            None => &self.name,
        }
    }
}

impl Bench {
    /// The definition of a simulated component's part.
    pub fn part<'a>(&'a self, design: &'a Design, component: &SimComponent) -> Option<&'a PartDefinition> {
        self.library
            .get(&component.part)
            .or_else(|| self.target_library.get(&component.part))
            .or_else(|| design.electrical.library.get(&component.part))
    }

    /// The flat view of the bench's target: the elaborated board, or the module on its own.
    pub fn view(&self, design: &Design) -> std::result::Result<FlatElectricalView, String> {
        target_view(design, self.module.as_deref())
    }

    /// The part sim models a component simulated with [`Treatment::PartModel`]
    /// runs with: per unit, the built-in or the `sim spice` block as
    /// [`Bench::model_choice`] selects when the unit has both
    /// ([`simulation::select_models`]).
    pub fn models<'a>(&'a self, design: &'a Design, component: &SimComponent) -> Vec<&'a PartSimModel> {
        match (&component.treatment, self.part(design, component)) {
            (Treatment::PartModel, Some(part)) => simulation::select_models(&part.sim, self.model_choice),
            _ => Vec::new(),
        }
    }
}

/// The flat view a bench simulates: the elaborated board (`module` `None`),
/// or one module definition of the board's design on its own
/// ([`crate::elaborate::elaborate_module`]).
pub fn target_view(design: &Design, module: Option<&str>) -> std::result::Result<FlatElectricalView, String> {
    match module {
        None => elaborate(&design.electrical),
        Some(module) => crate::elaborate::elaborate_module(&design.electrical, module),
    }
}

/// The module definitions of the design that a bench target `name` names:
/// by qualified name (`analog.AnalogFrontEnd`) or by the definition's own
/// name (`AnalogFrontEnd`).
fn modules_named<'a>(design: &'a Design, name: &str) -> Vec<&'a String> {
    design
        .electrical
        .module_definitions
        .keys()
        .filter(|key| key.as_str() == name || key.rsplit('.').next() == Some(name))
        .collect()
}

/// The module a bench is for, or `None` when it is for the board.
fn resolve_target(document: &Document, target: &BenchTargetDecl, design: &Design) -> Result<Option<String>> {
    let board = &design.electrical;
    if target.board == board.name {
        return Ok(None);
    }
    match modules_named(design, &target.board).as_slice() {
        [module] => Ok(Some((*module).clone())),
        [] if board.module_definitions.is_empty() => Err(error(
            "SIM030",
            format!(
                "bench {} is for board {}, but the board is {}",
                repr(&document.name),
                repr(&target.board),
                repr(&board.name)
            ),
            &target.location,
        )),
        [] => {
            let mut names: Vec<&str> = vec![board.name.as_str()];
            names.extend(board.module_definitions.keys().filter_map(|key| key.rsplit('.').next()));
            let modules: Vec<&str> = board.module_definitions.keys().map(String::as_str).collect();
            Err(suggesting(
                error(
                    "SIM030",
                    format!(
                        "bench {} is for {}, but the board is {} and has no module of that name (its modules: {})",
                        repr(&document.name),
                        repr(&target.board),
                        repr(&board.name),
                        modules.join(", ")
                    ),
                    &target.location,
                ),
                &target.board,
                &names,
            ))
        }
        several => {
            let names: Vec<&str> = several.iter().map(|name| name.as_str()).collect();
            Err(error(
                "SIM030",
                format!(
                    "bench {} is for {}, which names several modules ({}); name one by its qualified name",
                    repr(&document.name),
                    repr(&target.board),
                    names.join(", ")
                ),
                &target.location,
            ))
        }
    }
}

/// `<board directory>/benches/*.copper`, sorted: benches and fixtures.
pub fn bench_sources(board_file: &Path) -> Vec<PathBuf> {
    let directory = board_file.parent().unwrap_or(Path::new("."));
    package_sources(&directory.join(BENCH_DIRECTORY))
}

/// Compile every bench in the board's `benches/` directory that is for this
/// board or for one of the modules its design contains. A bench for another
/// board is skipped when a `.copper` file beside `benches/` declares that
/// board; a bench for anything else is an error.
pub fn load_benches(board_file: &Path, design: &Design) -> Result<Vec<Bench>> {
    let sources = bench_sources(board_file);
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let directory = board_file.parent().unwrap_or(Path::new("."));
    let mut boards: Option<Vec<String>> = None;
    let mut benches = Vec::new();
    let mut documents = Vec::new();
    for path in sources {
        documents.push(read_document(&path)?);
    }
    let (fixtures, documents): (Vec<Document>, Vec<Document>) =
        documents.into_iter().partition(|document| document.kind == DocumentKind::Fixture);
    for document in documents {
        let target = bench_target(&document)?;
        if target.board == design.electrical.name || !modules_named(design, &target.board).is_empty() {
            benches.extend(compile_bench_sections(&document, &fixtures, design)?);
            continue;
        }
        let known = boards.get_or_insert_with(|| {
            package_sources(directory)
                .iter()
                .filter_map(|path| read_source(path).ok().map(|text| (path, text)))
                .filter_map(|(path, text)| parse(&text, &paths::display(path)).ok())
                .filter(|document| document.kind == DocumentKind::Board)
                .map(|document| document.name)
                .collect()
        });
        if !known.contains(&target.board) {
            let names: Vec<&str> = known.iter().map(String::as_str).collect();
            return Err(suggesting(
                error(
                    "SIM030",
                    format!(
                        "bench {} is for board {}, which no source in {} declares and which is no module of board {}",
                        repr(&document.name),
                        repr(&target.board),
                        paths::display(directory),
                        repr(&design.electrical.name)
                    ),
                    &target.location,
                ),
                &target.board,
                &names,
            ));
        }
    }
    Ok(benches)
}

fn read_document(path: &Path) -> Result<Document> {
    let filename = paths::display(&paths::resolve(path));
    let source = read_source(path).map_err(|failure| {
        error(
            "CMP001",
            format!("cannot read source: {}", paths::python_os_error(&failure, Some(&filename))),
            &crate::packages::location(&filename, 1),
        )
    })?;
    parse(&source, &filename)
}

/// Compile the bench in `path` against `design` (with the fixtures of the
/// `.copper` files beside it).
pub fn compile_bench_file(path: &Path, design: &Design) -> Result<Bench> {
    let document = read_document(path)?;
    let mut fixtures = Vec::new();
    if bench_target(&document).is_ok_and(|target| !target.fixtures.is_empty()) {
        let directory = path.parent().unwrap_or(Path::new("."));
        for source in package_sources(directory) {
            if let Ok(other) = read_document(&source)
                && other.kind == DocumentKind::Fixture
            {
                fixtures.push(other);
            }
        }
    }
    compile_bench_with_fixtures(&document, &fixtures, design)
}

/// Compile the bench source `source` against `design`.
pub fn compile_bench_source(source: &str, filename: &str, design: &Design) -> Result<Bench> {
    compile_bench(&parse(source, filename)?, design)
}

fn bench_target(document: &Document) -> Result<&BenchTargetDecl> {
    match document.declarations.first() {
        Some(Declaration::Bench(BenchStatement::Target(target))) if document.kind == DocumentKind::Bench => Ok(target),
        _ => Err(error("SIM030", "expected a bench document: bench NAME for TARGET { ... }", &document.location)),
    }
}

/// The name a declaration of a fixture declares, for a bench to override:
/// a param, a supply, a bench model, a replacement or a bench component.
fn overridable(declaration: &Declaration) -> Option<(&'static str, &str)> {
    match declaration {
        Declaration::Param(item) => Some(("param", &item.name)),
        Declaration::Supply(item) => Some(("supply", &item.name)),
        Declaration::Component(item) => Some(("component", &item.reference)),
        Declaration::Bench(BenchStatement::Model(item)) => Some(("model", &item.name)),
        Declaration::Bench(BenchStatement::Replace(item)) => Some(("replace", &item.target)),
        _ => None,
    }
}

/// The bench `document` with the declarations of the fixtures it uses in
/// front of its own, in `using` order. A fixture may use other fixtures
/// (`fixture B for T using A`): they come first, each fixture once, and a
/// fixture declaration of the same param, supply, bench model, replacement
/// or bench component as one of a fixture it uses (directly or through
/// another) takes that declaration's place. A bench declaration of the same
/// key as any fixture's takes the fixture declaration's place (so later
/// fixture params derived from it follow it). Fixtures hold setup only (`SIM073`).
fn merge_fixtures(document: &Document, fixtures: &[Document], design: &Design) -> Result<Document> {
    let target = bench_target(document)?;
    if target.fixtures.is_empty() {
        return Ok(document.clone());
    }
    let own = resolve_target(document, target, design)?;
    let mut merged = Merged::default();
    let mut seen: Vec<String> = Vec::new();
    for (name, location) in target.fixtures.iter().zip(&target.fixture_locations) {
        if seen.contains(name) {
            return Err(error(
                "SIM073",
                format!("bench {} uses fixture {} twice", repr(&document.name), repr(name)),
                location,
            ));
        }
        seen.push(name.clone());
        let mut stack = vec![document.name.clone()];
        include_fixture(name, location, document, fixtures, design, &own, &mut merged, &mut stack)?;
    }
    let own_declarations = &document.declarations[1..];
    let mut taken = vec![false; own_declarations.len()];
    let mut declarations = vec![document.declarations[0].clone()];
    for (declaration, _) in &merged.declarations {
        let replacement = overridable(declaration)
            .and_then(|key| own_declarations.iter().position(|own| overridable(own) == Some(key)));
        match replacement {
            Some(index) if !taken[index] => {
                taken[index] = true;
                declarations.push(own_declarations[index].clone());
            }
            _ => declarations.push(declaration.clone()),
        }
    }
    declarations.extend(own_declarations.iter().zip(&taken).filter(|(_, taken)| !**taken).map(|(own, _)| own.clone()));
    let mut libraries = merged.libraries;
    for library in &document.libraries {
        if !libraries.contains(library) {
            libraries.push(library.clone());
        }
    }
    let mut imports = merged.imports;
    imports.extend(document.imports.iter().cloned());
    let mut header = document.declarations[0].clone();
    // The bench records every fixture it compiled with, in the order they were included.
    if let Declaration::Bench(BenchStatement::Target(target)) = &mut header {
        target.fixtures = merged.included.clone();
    }
    declarations[0] = header;
    Ok(Document {
        location: document.location.clone(),
        kind: document.kind,
        name: document.name.clone(),
        libraries,
        imports,
        declarations,
    })
}

/// The fixtures a bench includes, flattened.
#[derive(Default)]
struct Merged {
    /// Each declaration and the fixture it came from.
    declarations: Vec<(Declaration, String)>,
    /// Fixture name -> the fixtures it uses, directly or through others.
    uses: IndexMap<String, Vec<String>>,
    included: Vec<String>,
    libraries: Vec<String>,
    imports: Vec<ImportDecl>,
}

/// Add fixture `name` (after the fixtures it uses) to `merged`, once.
#[allow(clippy::too_many_arguments)]
fn include_fixture(
    name: &str,
    location: &Location,
    user: &Document,
    fixtures: &[Document],
    design: &Design,
    own: &Option<String>,
    merged: &mut Merged,
    stack: &mut Vec<String>,
) -> Result<()> {
    if merged.included.iter().any(|item| item == name) {
        return Ok(());
    }
    if stack.iter().skip(1).any(|item| item == name) {
        return Err(error(
            "SIM073",
            format!("fixture {} uses itself through {}", repr(name), stack[1..].join(" -> ")),
            location,
        ));
    }
    let Some(fixture) = fixtures.iter().find(|fixture| fixture.name == name) else {
        let names: Vec<&str> = fixtures.iter().map(|fixture| fixture.name.as_str()).collect();
        let what = if user.kind == DocumentKind::Fixture { "fixture" } else { "bench" };
        return Err(suggesting(
            error(
                "SIM073",
                format!(
                    "{what} {} uses fixture {}, which no fixture NAME for TARGET {{ ... }} in its directory declares",
                    repr(&user.name),
                    repr(name)
                ),
                location,
            ),
            name,
            &names,
        ));
    };
    let Some(Declaration::Bench(BenchStatement::Target(fixture_target))) = fixture.declarations.first() else {
        return Err(error("SIM073", "expected a fixture document: fixture NAME for TARGET { ... }", &fixture.location));
    };
    if &resolve_target(fixture, fixture_target, design)? != own {
        return Err(error(
            "SIM073",
            format!(
                "fixture {} is for {}, but {} is for another target; fixtures and the benches that use them share one target",
                repr(&fixture.name),
                repr(&fixture_target.board),
                repr(&user.name)
            ),
            location,
        ));
    }
    for declaration in &fixture.declarations[1..] {
        let bench_only = match declaration {
            Declaration::Bench(BenchStatement::Analysis(item)) => Some(("analysis", &item.location)),
            Declaration::Bench(BenchStatement::Step(item)) => Some(("step", &item.location)),
            Declaration::Bench(BenchStatement::Measure(item)) => Some(("measure", &item.location)),
            Declaration::Bench(BenchStatement::Plot(item)) => Some(("plot", &item.location)),
            Declaration::Bench(BenchStatement::Variation(item)) if item.kind != "tolerance" => {
                Some((item.kind.as_str(), &item.location))
            }
            _ => None,
        };
        if let Some((keyword, location)) = bench_only {
            return Err(error(
                "SIM073",
                format!(
                    "a fixture holds setup shared by benches; {keyword} belongs in the bench (fixture {})",
                    repr(&fixture.name)
                ),
                location,
            ));
        }
    }
    // The fixtures this one uses, first.
    stack.push(name.to_owned());
    let mut closure: Vec<String> = Vec::new();
    for (used, used_location) in fixture_target.fixtures.iter().zip(&fixture_target.fixture_locations) {
        include_fixture(used, used_location, fixture, fixtures, design, own, merged, stack)?;
        closure.push(used.clone());
        closure.extend(merged.uses.get(used).cloned().unwrap_or_default());
    }
    stack.pop();
    // Its own declarations: in place of a used fixture's declaration of the same key, else after.
    for declaration in &fixture.declarations[1..] {
        let place = overridable(declaration).and_then(|key| {
            merged
                .declarations
                .iter()
                .position(|(other, origin)| closure.contains(origin) && overridable(other) == Some(key))
        });
        match place {
            Some(index) => merged.declarations[index] = (declaration.clone(), name.to_owned()),
            None => merged.declarations.push((declaration.clone(), name.to_owned())),
        }
    }
    for library in &fixture.libraries {
        if !merged.libraries.contains(library) {
            merged.libraries.push(library.clone());
        }
    }
    merged.imports.extend(fixture.imports.iter().cloned());
    merged.uses.insert(name.to_owned(), closure);
    merged.included.push(name.to_owned());
    Ok(())
}

/// The effective params: the target's (board or module), then the bench's overrides and its own.
fn effective_params(
    document: &Document,
    target: &IndexMap<String, simulation::Param>,
) -> Result<IndexMap<String, BenchParam>> {
    let mut params: IndexMap<String, BenchParam> = target
        .values()
        .map(|param| {
            (
                param.name.clone(),
                BenchParam {
                    value: param.value.clone(),
                    origin: ParamOrigin::Board,
                    location: param.location.clone(),
                    derived: None,
                },
            )
        })
        .collect();
    let mut declared: Vec<&str> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Param(declaration) = declaration else { continue };
        if declared.contains(&declaration.name.as_str()) {
            return Err(error("SIM002", format!("duplicate param {}", repr(&declaration.name)), &declaration.location));
        }
        declared.push(&declaration.name);
        let (value, derived) = match &declaration.expression {
            Some(expression) => {
                let (value, checked) = derived_param(declaration, expression, &params)?;
                (value, Some(checked))
            }
            None => (simulation::param_value(declaration)?, None),
        };
        let origin = match params.get(&declaration.name) {
            Some(board) if board.value.dimension() != value.dimension() => {
                return Err(error(
                    "SIM003",
                    format!(
                        "param {} is a {} on the board; the bench cannot make it a {}",
                        repr(&declaration.name),
                        board.value.dimension().name(),
                        value.dimension().name()
                    ),
                    &declaration.location,
                ));
            }
            Some(_) => ParamOrigin::BenchOverride,
            None => ParamOrigin::Bench,
        };
        params.insert(
            declaration.name.clone(),
            BenchParam { value, origin, location: declaration.location.clone(), derived },
        );
    }
    Ok(params)
}

/// Checks a derived param's expression: earlier params and literals only.
struct ParamScope<'a> {
    params: &'a IndexMap<String, BenchParam>,
    /// What the expression is, for the message when it probes the circuit.
    what: &'static str,
}

impl Scope for ParamScope<'_> {
    fn swept(&self) -> bool {
        false
    }

    fn param(&self, name: &str) -> Option<Dimension> {
        self.params.get(name).map(|param| param.value.dimension())
    }

    fn let_type(&self, _name: &str) -> Option<ExpressionType> {
        None
    }

    fn names(&self) -> Vec<String> {
        self.params.keys().cloned().collect()
    }

    fn node(&self, _target: &str, location: &Location) -> Result<Node> {
        Err(self.probing(location))
    }

    fn current(&self, _target: &str, location: &Location) -> Result<(String, String)> {
        Err(self.probing(location))
    }
}

impl ParamScope<'_> {
    fn probing(&self, location: &Location) -> CopperScriptError {
        let code = if self.what == "a derived param" { "SIM001" } else { "SIM072" };
        error(code, format!("{} is computed from params and numbers; it cannot probe the circuit", self.what), location)
    }
}

/// Whether a checked expression uses numbers, quantities, params and `+ - * /` only.
fn arithmetic(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Number(_) | ExpressionKind::Quantity(_) | ExpressionKind::Param(_) => true,
        ExpressionKind::Negate(operand) => arithmetic(operand),
        ExpressionKind::Binary { left, right, .. } => arithmetic(left) && arithmetic(right),
        _ => false,
    }
}

/// The value of a derived param's checked expression, from the params' nominal values.
fn fold(expression: &Expression, params: &IndexMap<String, BenchParam>) -> std::result::Result<Decimal, String> {
    let failed = |error: copper_units::DecimalError| error.to_string();
    match &expression.kind {
        ExpressionKind::Number(number) => Ok(number.clone()),
        ExpressionKind::Quantity(quantity) => Ok(quantity.base_value().clone()),
        ExpressionKind::Param(name) => Ok(params[name].value.base_value().clone()),
        ExpressionKind::Negate(operand) => Ok(fold(operand, params)?.copy_negate()),
        ExpressionKind::Binary { operator, left, right } => {
            let (a, b) = (fold(left, params)?, fold(right, params)?);
            match operator {
                BinaryOperator::Add => a.add(&b),
                BinaryOperator::Subtract => a.sub(&b),
                BinaryOperator::Multiply => a.mul(&b),
                BinaryOperator::Divide => a.div(&b),
            }
            .map_err(failed)
        }
        _ => Err("only numbers, quantities, params and + - * / are allowed".to_owned()),
    }
}

/// The unit of `dimension` that shows `base` with one to three integer digits.
fn engineering_unit(dimension: Dimension, base: &Decimal) -> &'static str {
    let value: f64 = base.to_sci_string().parse().unwrap_or(0.0);
    let units = dimension.units();
    let scaled = |scale: &str| value.abs() / scale.parse::<f64>().unwrap_or(1.0);
    units
        .iter()
        .filter(|(_, scale)| (1.0..1000.0).contains(&scaled(scale)))
        .map(|(name, _)| *name)
        .next()
        .or_else(|| units.iter().find(|(_, scale)| *scale == "1").map(|(name, _)| *name))
        .unwrap_or(units[0].0)
}

/// A derived param: its checked expression and nominal value.
fn derived_param(
    declaration: &ParamDecl,
    expression: &Expr,
    params: &IndexMap<String, BenchParam>,
) -> Result<(Quantity, Expression)> {
    let checked = expression::check(expression, &ParamScope { params, what: "a derived param" })?;
    if !arithmetic(&checked) {
        return Err(error(
            "SIM001",
            format!(
                "derived param {} may use numbers, quantities, earlier params and + - * / only",
                repr(&declaration.name)
            ),
            &declaration.location,
        ));
    }
    let Some(dimension) = checked.ty.units.dimension() else {
        return Err(error(
            "SIM001",
            format!(
                "derived param {} has units {}, which is no quantity a param can hold",
                repr(&declaration.name),
                checked.ty.units.base_text()
            ),
            &declaration.location,
        ));
    };
    let base = fold(&checked, params).map_err(|reason| {
        error("SIM001", format!("derived param {}: {reason}", repr(&declaration.name)), &declaration.location)
    })?;
    let unit = engineering_unit(dimension, &base);
    let value = Quantity::from_base(dimension, base, unit).expect("the unit belongs to the dimension");
    Ok((value, checked))
}

struct Context<'a> {
    view: &'a FlatElectricalView,
    library: &'a IndexMap<String, PartDefinition>,
    components: &'a [SimComponent],
    bench_components: &'a [ComponentInstance],
    nets: &'a [BenchNet],
    /// The model kind of each replacement (by target) and bench model (by name), for `I(...)`.
    model_kinds: &'a [(String, SimKind)],
    /// The placed amplifier models that declare `quiescent_current`: their supply terminals carry current.
    supplied: &'a [String],
    /// Every supply, for `I(SUPPLY)`.
    supplies: &'a [SimSupply],
}

impl Context<'_> {
    fn part(&self, part: &str) -> Option<&PartDefinition> {
        self.library.get(part).or_else(|| self.view.library.get(part))
    }

    fn board_component(&self, reference: &str) -> Option<&ComponentInstance> {
        self.view.components.iter().find(|component| component.reference == reference)
    }

    fn bench_component(&self, reference: &str) -> Option<&ComponentInstance> {
        self.bench_components.iter().find(|component| component.reference == reference)
    }

    fn board_net(&self, name: &str) -> Option<String> {
        if self.view.nets.iter().any(|net| net.name == name) {
            return Some(name.to_owned());
        }
        self.view.net_aliases.get(name).cloned()
    }

    fn node_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.view.nets.iter().map(|net| net.name.clone()).collect();
        names.extend(self.nets.iter().filter(|net| net.join == NetJoin::New).map(|net| net.name.clone()));
        names
    }

    /// A board pin's node: the board net that connects it, or the pin itself.
    fn board_pin(&self, endpoint: &Endpoint, location: &Location) -> Result<Node> {
        if let Some(net) = self.view.nets.iter().find(|net| net.endpoints.contains(endpoint)) {
            return Ok(Node::Net(net.name.clone()));
        }
        let component = self.board_component(&endpoint.component).expect("checked by the caller");
        let part = self.part(&component.part);
        if part.is_some_and(|part| part.pins.contains_key(&endpoint.pin)) {
            return Ok(Node::Pin(endpoint.clone()));
        }
        let pins: Vec<&str> = part.map(|part| part.pins.keys().map(String::as_str).collect()).unwrap_or_default();
        Err(suggesting(
            error("SIM039", format!("{} has no pin {}", repr(&endpoint.component), repr(&endpoint.pin)), location),
            &endpoint.pin,
            &pins,
        ))
    }

    /// A node named in a replace role or a `V(...)` probe.
    fn node(&self, target: &str, location: &Location) -> Result<Node> {
        if let Some(net) = self.nets.iter().find(|net| net.name == target) {
            return Ok(net.node.clone());
        }
        if let Some(net) = self.board_net(target) {
            return Ok(Node::Net(net));
        }
        if let Ok(endpoint) = Endpoint::parse(target) {
            if self.board_component(&endpoint.component).is_some() {
                return self.board_pin(&endpoint, location);
            }
            if self.bench_component(&endpoint.component).is_some() {
                if let Some(net) = self.nets.iter().find(|net| net.endpoints.contains(&endpoint)) {
                    return Ok(net.node.clone());
                }
                return Err(error("SIM039", format!("bench pin {target} is on no bench net"), location));
            }
            return Err(error(
                "SIM039",
                format!("unknown node {}: {} is no board or bench component", repr(target), repr(&endpoint.component)),
                location,
            ));
        }
        let names = self.node_names();
        let candidates: Vec<&str> = names.iter().map(String::as_str).collect();
        Err(suggesting(
            error(
                "SIM039",
                format!("unknown node {}; expected a board net, a bench net or COMPONENT.PIN", repr(target)),
                location,
            ),
            target,
            &candidates,
        ))
    }

    /// The probe pin of `analysis loop`: a pin of a simulated board or bench
    /// component (a role pin of its `sim` block for a modelled part) that a
    /// net connects; returns the node of that net (`SIM098`).
    fn loop_probe(&self, endpoint: &Endpoint, location: &Location) -> Result<Node> {
        let refuse = |message: String| Err(error("SIM098", message, location));
        let Some(component) = self.components.iter().find(|component| component.reference == endpoint.component) else {
            let names: Vec<&str> = self.components.iter().map(|component| component.reference.as_str()).collect();
            return Err(suggesting(
                error(
                    "SIM098",
                    format!(
                        "analysis loop probe {endpoint}: {} is no component of the bench",
                        repr(&endpoint.component)
                    ),
                    location,
                ),
                &endpoint.component,
                &names,
            ));
        };
        match &component.treatment {
            Treatment::Ignored => {
                return refuse(format!(
                    "analysis loop probe {endpoint}: {} is ignored, so its pins are open; probe a pin of a simulated component",
                    endpoint.component
                ));
            }
            Treatment::Replaced(_) => {
                return refuse(format!(
                    "analysis loop probe {endpoint}: {} is replaced by a bench model, whose terminals are nets, not pins; probe a pin of another component on that net",
                    endpoint.component
                ));
            }
            Treatment::Passive { .. } | Treatment::PartModel => {}
        }
        if let Treatment::PartModel = component.treatment
            && let Some(part) = self.part(&component.part)
        {
            let modelled = part.sim.iter().any(|model| model.pins.values().any(|pin| pin == &endpoint.pin));
            if !modelled {
                let pins: Vec<&str> =
                    part.sim.iter().flat_map(|model| model.pins.values().map(String::as_str)).collect();
                return Err(suggesting(
                    error(
                        "SIM098",
                        format!(
                            "analysis loop probe {endpoint}: pin {} is no pin of {}'s sim model, so no loop passes it; probe one of {}",
                            repr(&endpoint.pin),
                            endpoint.component,
                            pins.join(", ")
                        ),
                        location,
                    ),
                    &endpoint.pin,
                    &pins,
                ));
            }
        }
        let node = if self.board_component(&endpoint.component).is_some() {
            self.board_pin(endpoint, location)?
        } else {
            match self.nets.iter().find(|net| net.endpoints.contains(endpoint)) {
                Some(net) => net.node.clone(),
                None => return refuse(format!("analysis loop probe {endpoint}: the bench pin is on no bench net")),
            }
        };
        match node {
            Node::Net(_) => Ok(node),
            Node::Pin(_) => refuse(format!(
                "analysis loop probe {endpoint}: the pin is on no net, so no loop passes it; probe a connected pin"
            )),
        }
    }

    /// `I(X)` or `I(X.ROLE)` of a model the bench places (a replacement or a
    /// bench model): the current into a role's terminal. `I(X)` of a
    /// two-terminal model is its first role's.
    fn model_current(
        &self,
        reference: &str,
        pin: Option<&str>,
        kind: SimKind,
        what: &str,
        location: &Location,
    ) -> Result<(String, String)> {
        let roles = kind.roles();
        // The terminals the lowering gives currents (`copper-simulate`, `lower`).
        let carrying: Vec<&str> = match kind {
            SimKind::Opamp | SimKind::Diffamp if self.supplied.iter().any(|name| name == reference) => {
                vec!["inp", "inn", "out", "vp", "vn"]
            }
            SimKind::Opamp | SimKind::Diffamp => vec!["inp", "inn", "out"],
            SimKind::Switch => vec!["a", "b"],
            // A rail model's enable draws no current.
            SimKind::Regulator | SimKind::Reference => vec!["in", "out", "gnd"],
            _ => roles.to_vec(),
        };
        let listed = carrying.join(", ");
        match pin {
            None if roles.len() == 2 => Ok((reference.to_owned(), roles[0].to_owned())),
            None => Err(error(
                "SIM045",
                format!(
                    "{} is {what} of kind {}, which has {} terminals; name one: I({reference}.ROLE) with ROLE one of {listed}",
                    repr(reference),
                    kind.name(),
                    roles.len()
                ),
                location,
            )),
            Some(role) if carrying.contains(&role) => Ok((reference.to_owned(), role.to_owned())),
            Some(role) if roles.contains(&role) || kind.optional_roles().contains(&role) => Err(error(
                "SIM045",
                format!(
                    "the {} terminal of the {} model of {} carries no current in this model; I(...) reads {listed}",
                    role,
                    kind.name(),
                    repr(reference)
                ),
                location,
            )),
            Some(pin) => Err(suggesting(
                error(
                    "SIM045",
                    format!(
                        "{} is {what} of kind {}: I(...) names the model's terminals, not the part's pins, so write I({reference}.ROLE) with ROLE one of {listed}{} (found {})",
                        repr(reference),
                        kind.name(),
                        if roles.len() == 2 {
                            format!(", or I({reference}) for the current into {}", roles[0])
                        } else {
                            String::new()
                        },
                        repr(pin)
                    ),
                    location,
                ),
                pin,
                &carrying,
            )),
        }
    }

    /// `I(SUPPLY)`: the current a supply delivers into its net (`SUPPLY_NET`
    /// of instance `supply NAME`); `I(SUPPLY.ROLE)` of a replaced supply: the
    /// current into a terminal of its model.
    fn supply_current(&self, supply: &SimSupply, pin: Option<&str>, location: &Location) -> Result<(String, String)> {
        let instance = format!("supply {}", supply.name);
        let refuse = |reason: String| Err(error("SIM045", format!("I({}): {reason}", supply.name), location));
        match (&supply.treatment, pin) {
            (SupplyTreatment::DcSource | SupplyTreatment::Replaced(_), None) => Ok((instance, SUPPLY_NET.to_owned())),
            (SupplyTreatment::Replaced(_), Some(role)) => {
                let Some((_, kind)) = self.model_kinds.iter().find(|(name, _)| name == &supply.name) else {
                    return refuse("the replacement has an unknown model kind".to_owned());
                };
                let (_, role) = self.model_current(&supply.name, Some(role), *kind, "replaced by a model", location)?;
                Ok((instance, role))
            }
            (SupplyTreatment::DcSource, Some(pin)) => refuse(format!(
                "a supply is one source on its net; write I({}) for the current it delivers, not {}",
                supply.name,
                repr(pin)
            )),
            (SupplyTreatment::Ground, _) => refuse(format!(
                "{} is the simulation ground, which takes the return of every source; probe another supply",
                repr(&supply.name)
            )),
            (SupplyTreatment::Component(component), _) => refuse(format!(
                "{} is driven by component {component}; probe the current of its pins, I({component}.PIN)",
                repr(&supply.name)
            )),
            (SupplyTreatment::Ignored, _) => {
                refuse(format!("{} is ignored in this bench; it delivers no current", repr(&supply.name)))
            }
        }
    }

    fn current(&self, target: &str, location: &Location) -> Result<(String, String)> {
        let (reference, pin) = match target.split_once('.') {
            Some((reference, pin)) => (reference, Some(pin)),
            None => (target, None),
        };
        if let Some(supply) = self.supplies.iter().find(|supply| supply.name == reference)
            && !self.components.iter().any(|component| component.reference == reference)
        {
            return self.supply_current(supply, pin, location);
        }
        let placed = self.model_kinds.iter().find(|(name, _)| name == reference);
        if let Some((_, kind)) = placed
            && !self.components.iter().any(|component| component.reference == reference)
        {
            return self.model_current(reference, pin, *kind, "a bench model", location);
        }
        let Some(component) = self.components.iter().find(|component| component.reference == reference) else {
            let mut names: Vec<&str> = self.components.iter().map(|component| component.reference.as_str()).collect();
            names.extend(self.supplies.iter().map(|supply| supply.name.as_str()));
            return Err(suggesting(
                error("SIM045", format!("unknown component or supply {} in I(...)", repr(reference)), location),
                reference,
                &names,
            ));
        };
        match component.treatment {
            Treatment::Ignored => {
                return Err(error(
                    "SIM045",
                    format!("{} is ignored in this bench; it carries no current", repr(reference)),
                    location,
                ));
            }
            Treatment::Replaced(_) => {
                let Some((_, kind)) = placed else {
                    return Err(error(
                        "SIM045",
                        format!("the replacement of {} has an unknown model kind", repr(reference)),
                        location,
                    ));
                };
                return self.model_current(reference, pin, *kind, "replaced by a model", location);
            }
            _ => {}
        }
        let part = self.part(&component.part).expect("component parts are known");
        let pins: Vec<&str> = part.pins.keys().map(String::as_str).collect();
        match pin {
            Some(pin) if part.pins.contains_key(pin) => Ok((reference.to_owned(), pin.to_owned())),
            Some(pin) => Err(suggesting(
                error("SIM045", format!("{} has no pin {}", repr(reference), repr(pin)), location),
                pin,
                &pins,
            )),
            None if pins.len() == 2 => Ok((reference.to_owned(), pins[0].to_owned())),
            None => Err(error(
                "SIM045",
                format!(
                    "I({reference}) needs a two-terminal component; {} has {} pins, so name one: I({reference}.PIN)",
                    repr(reference),
                    pins.len()
                ),
                location,
            )),
        }
    }
}

struct ExpressionScope<'a> {
    context: &'a Context<'a>,
    swept: bool,
    params: &'a IndexMap<String, Quantity>,
    lets: &'a IndexMap<String, ExpressionType>,
    /// The checked lets so far, for `noise(x)` of a let.
    definitions: &'a [Let],
    /// Lets that read a reference run.
    referencing: &'a [String],
    references: &'a [String],
    /// `analysis noise`, and the names `noise(X)` may use.
    noise: Option<expression::NoiseScope>,
    contributors: &'a [String],
    /// `analysis tran`: probes are real and swept over time.
    time_domain: bool,
    /// `analysis dc`: the units of the swept value.
    sweep: Option<Units>,
    /// The kind of the analysis (`ac`, `loop`, `pz`, ...).
    analysis: &'static str,
}

impl Scope for ExpressionScope<'_> {
    fn swept(&self) -> bool {
        self.swept
    }

    fn param(&self, name: &str) -> Option<Dimension> {
        self.params.get(name).map(Quantity::dimension)
    }

    fn let_type(&self, name: &str) -> Option<ExpressionType> {
        self.lets.get(name).copied()
    }

    fn let_expression(&self, name: &str) -> Option<Expression> {
        self.definitions.iter().find(|item| item.name == name).map(|item| item.expression.clone())
    }

    fn names(&self) -> Vec<String> {
        self.lets.keys().chain(self.params.keys()).cloned().collect()
    }

    fn node(&self, target: &str, location: &Location) -> Result<Node> {
        self.context.node(target, location)
    }

    fn current(&self, target: &str, location: &Location) -> Result<(String, String)> {
        self.context.current(target, location)
    }

    fn reference(&self, name: &str) -> bool {
        self.references.iter().any(|reference| reference == name)
    }

    fn let_uses_reference(&self, name: &str) -> bool {
        self.referencing.iter().any(|item| item == name)
    }

    fn noise(&self) -> Option<expression::NoiseScope> {
        self.noise
    }

    fn noise_source(&self, target: &str, location: &Location) -> Result<String> {
        crate::noise::contributor(target, self.contributors, location)
    }

    fn time_domain(&self) -> bool {
        self.time_domain
    }

    fn sweep_units(&self) -> Option<Units> {
        self.sweep
    }

    fn analysis(&self) -> Option<&'static str> {
        Some(self.analysis)
    }
}

/// The param that sets the temperature of a bench's run, degrees Celsius
/// (27 degC when no param of the name exists).
pub const TEMPERATURE_PARAM: &str = "TEMP";

/// The bench temperature `TEMP` (`SIM110`): a temperature in degC above
/// absolute zero, at every step; with it, a noise analysis takes its
/// temperature from it and declares none of its own.
fn check_temperature(params: &IndexMap<String, BenchParam>, analysis: &Analysis, steps: &[Step]) -> Result<()> {
    let Some(param) = params.get(TEMPERATURE_PARAM) else { return Ok(()) };
    if param.value.dimension() != Dimension::Temperature {
        return Err(error(
            "SIM110",
            format!(
                "param {TEMPERATURE_PARAM} is the temperature of the run, in degC (such as param {TEMPERATURE_PARAM} = 85degC;), but it is a {}",
                param.value.dimension().name()
            ),
            &param.location,
        ));
    }
    let absolute_zero = Decimal::parse("-273.15").expect("a decimal");
    let mut values = vec![(&param.value, &param.location)];
    for step in steps {
        for (index, name) in step.params().into_iter().enumerate() {
            if name != TEMPERATURE_PARAM {
                continue;
            }
            let stepped = if index == 0 { &step.values } else { &step.zip[index - 1].1 };
            values.extend(stepped.iter().map(|value| (value, &step.location)));
        }
    }
    for (value, location) in values {
        if *value.base_value() <= absolute_zero {
            return Err(error("SIM110", format!("{TEMPERATURE_PARAM} = {value} is not above absolute zero"), location));
        }
    }
    if let Analysis::Noise(noise) = analysis
        && let Some(own) = &noise.temperature
    {
        return Err(error(
            "SIM110",
            format!(
                "the bench sets its temperature with param {TEMPERATURE_PARAM}, so analysis noise takes it from there; remove temperature = {}",
                own.written
            ),
            &param.location,
        ));
    }
    Ok(())
}

/// How a component with `part` and `value` simulates when it is neither
/// replaced nor ignored; `None` when it has no model.
fn own_treatment(
    component: &ComponentInstance,
    part: &PartDefinition,
    param: Option<&str>,
    location: &Location,
) -> Result<Option<Treatment>> {
    if let Some(kind) = PassiveKind::of(part) {
        let value = match &component.value {
            Some(ComponentValue::Quantity(quantity)) if quantity.dimension() == kind.dimension() => quantity.clone(),
            Some(ComponentValue::Quantity(quantity)) => {
                return Err(error(
                    "SIM034",
                    format!(
                        "{} is a {} but its value {quantity} is a {}",
                        repr(&component.reference),
                        kind.name(),
                        quantity.dimension().name()
                    ),
                    location,
                ));
            }
            _ => {
                return Err(error(
                    "SIM034",
                    format!(
                        "{} is a {} without a {} value; give it one, or replace or ignore it in the bench",
                        repr(&component.reference),
                        kind.name(),
                        kind.dimension().name()
                    ),
                    location,
                ));
            }
        };
        let extras = Box::new(PassiveExtras::of(kind, component, part));
        return Ok(Some(Treatment::Passive {
            kind,
            value: SimQuantity { value, param: param.map(str::to_owned) },
            extras,
        }));
    }
    for key in [crate::tolerance::TEMPCO, crate::tolerance::TEMPCO2] {
        if component.properties.contains_key(key) {
            return Err(error(
                "SIM111",
                format!(
                    "{} declares {key}, but it simulates with its part's sim blocks; {key} applies to resistors, capacitors and inductors",
                    repr(&component.reference)
                ),
                location,
            ));
        }
    }
    if !part.sim.is_empty() {
        return Ok(Some(Treatment::PartModel));
    }
    Ok(None)
}

/// Check `document`, a bench that uses no fixtures, against the compiled board `design`.
pub fn compile_bench(document: &Document, design: &Design) -> Result<Bench> {
    compile_bench_with_fixtures(document, &[], design)
}

/// Check `document`, a bench, against the compiled board `design`, with the
/// fixture documents it may use (`using A, B`).
pub fn compile_bench_with_fixtures(document: &Document, fixtures: &[Document], design: &Design) -> Result<Bench> {
    let mut sections = compile_bench_sections(document, fixtures, design)?;
    if sections.len() > 1 {
        return Err(error(
            "SIM097",
            format!(
                "bench {} has {} analyses, which compile to one bench each; load them with compile_bench_sections",
                repr(&document.name),
                sections.len()
            ),
            &document.location,
        ));
    }
    Ok(sections.remove(0))
}

/// Compile the bench source `source`, one [`Bench`] per analysis (see [`compile_bench_sections`]).
pub fn compile_benches_source(source: &str, filename: &str, design: &Design) -> Result<Vec<Bench>> {
    compile_bench_sections(&parse(source, filename)?, &[], design)
}

/// The analysis statements of a bench, in order.
fn analysis_declarations(document: &Document) -> Vec<(usize, &AnalysisDecl)> {
    document
        .declarations
        .iter()
        .enumerate()
        .filter_map(|(index, declaration)| match declaration {
            Declaration::Bench(BenchStatement::Analysis(item)) => Some((index, item)),
            _ => None,
        })
        .collect()
}

/// The names an expression reads (params, lets and contributors alike).
fn expression_names(expression: &Expr, out: &mut Vec<String>) {
    match &expression.kind {
        ExprKind::Name(name) => out.push(name.clone()),
        ExprKind::Call { arguments, .. } => arguments.iter().for_each(|argument| expression_names(argument, out)),
        ExprKind::Negate(operand) => expression_names(operand, out),
        ExprKind::Binary { left, right, .. } | ExprKind::Compare { left, right, .. } => {
            expression_names(left, out);
            expression_names(right, out);
        }
        ExprKind::Number(_) | ExprKind::Quantity { .. } | ExprKind::Probe { .. } => {}
    }
}

/// Compile `document`, a bench, with the fixture documents it may use: one
/// [`Bench`] for a bench with one analysis, and one per analysis for a bench
/// with several (`SIM097`). In a bench with several analyses each `analysis`
/// starts its section: the `let`, `measure` and `plot` statements after it,
/// up to the next `analysis`, belong to it, and its name is the analysis's
/// (`analysis NAME: KIND`, or the kind). Everything else is shared, and so
/// are the lets before the first analysis (fixture lets among them); a
/// section sees the shared lets its own statements use, directly or through
/// other lets. `ic` statements go to the transient sections.
pub fn compile_bench_sections(document: &Document, fixtures: &[Document], design: &Design) -> Result<Vec<Bench>> {
    let merged = merge_fixtures(document, fixtures, design)?;
    let analyses = analysis_declarations(&merged);
    if analyses.len() <= 1 {
        return Ok(vec![compile_merged(&merged, design)?]);
    }
    let mut names: Vec<String> = Vec::new();
    for (_, item) in &analyses {
        let name = item.name.clone().unwrap_or_else(|| item.kind.clone());
        if names.contains(&name) {
            return Err(error(
                "SIM097",
                format!(
                    "bench {} has two analyses named {}; name each: analysis NAME: {} {{ ... }}",
                    repr(&merged.name),
                    repr(&name),
                    item.kind
                ),
                &item.location,
            ));
        }
        names.push(name);
    }
    let first = analyses[0].0;
    let section_of = |index: usize| analyses.iter().rposition(|(start, _)| *start <= index);
    let tran = analyses.iter().any(|(_, item)| item.kind == "tran");
    // The shared lets: before the first analysis.
    let mut shared: Vec<(String, Vec<String>)> = Vec::new();
    for (index, declaration) in merged.declarations.iter().enumerate() {
        match declaration {
            Declaration::Bench(BenchStatement::Let(item)) if index < first => {
                let mut used = Vec::new();
                expression_names(&item.expression, &mut used);
                shared.push((item.name.clone(), used));
            }
            Declaration::Bench(BenchStatement::Measure(item)) if index < first => {
                return Err(error(
                    "SIM097",
                    format!(
                        "measure {} comes before the first analysis; in a bench with several analyses a measure belongs to the analysis it follows",
                        item.name
                    ),
                    &item.location,
                ));
            }
            Declaration::Bench(BenchStatement::Plot(item)) if index < first => {
                return Err(error(
                    "SIM097",
                    "a plot comes before the first analysis; in a bench with several analyses a plot belongs to the analysis it follows",
                    &item.location,
                ));
            }
            Declaration::Bench(BenchStatement::InitialCondition(item)) if !tran => {
                return Err(error(
                    "SIM075",
                    "ic sets the start of a transient run, and no analysis of the bench is tran",
                    &item.location,
                ));
            }
            _ => {}
        }
    }
    let mut benches = Vec::new();
    for (k, (_, analysis)) in analyses.iter().enumerate() {
        // The names this section's own statements read, then the shared lets among them, transitively.
        let mut wanted: Vec<String> = Vec::new();
        for (index, declaration) in merged.declarations.iter().enumerate() {
            if index < first || section_of(index) != Some(k) {
                continue;
            }
            match declaration {
                Declaration::Bench(BenchStatement::Let(item)) => expression_names(&item.expression, &mut wanted),
                Declaration::Bench(BenchStatement::Measure(item)) => expression_names(&item.expression, &mut wanted),
                Declaration::Bench(BenchStatement::Plot(item)) => {
                    item.expressions.iter().for_each(|expression| expression_names(expression, &mut wanted))
                }
                _ => {}
            }
        }
        let mut needed: Vec<&str> = Vec::new();
        for (name, used) in shared.iter().rev() {
            if wanted.contains(name) {
                needed.push(name);
                wanted.extend(used.iter().cloned());
            }
        }
        let declarations: Vec<Declaration> = merged
            .declarations
            .iter()
            .enumerate()
            .filter(|(index, declaration)| match declaration {
                Declaration::Bench(BenchStatement::Analysis(_)) => *index == analyses[k].0,
                Declaration::Bench(BenchStatement::Let(item)) if *index < first => needed.contains(&item.name.as_str()),
                Declaration::Bench(BenchStatement::Let(_) | BenchStatement::Measure(_) | BenchStatement::Plot(_)) => {
                    section_of(*index) == Some(k)
                }
                Declaration::Bench(BenchStatement::InitialCondition(_)) => analysis.kind == "tran",
                _ => true,
            })
            .map(|(_, declaration)| declaration.clone())
            .collect();
        let section = Document { declarations, ..merged.clone() };
        let mut bench = compile_merged(&section, design)?;
        bench.name = format!("{}.{}", merged.name, names[k]);
        bench.section = Some(names[k].clone());
        benches.push(bench);
    }
    Ok(benches)
}

/// Compile a bench document whose fixtures are merged in and that has at most one analysis.
fn compile_merged(document: &Document, design: &Design) -> Result<Bench> {
    let target = bench_target(document)?;
    let board = &design.electrical;
    let module = resolve_target(document, target, design)?;
    // What the bench's own words call the target's parts, in messages.
    let owner = if module.is_some() { "module" } else { "board" };
    if let Some(import) = document.imports.first() {
        return Err(error(
            "SIM031",
            "bench package imports are not supported yet; import the part in the board, or use a library",
            &import.location,
        ));
    }
    let mut library: IndexMap<String, PartDefinition> = IndexMap::new();
    for name in &document.libraries {
        let Some(parts) = crate::library::library(name) else {
            return Err(error("CMP002", format!("unknown library {}", repr(name)), &document.location));
        };
        for (name, part) in parts {
            library.entry(name).or_insert(part);
        }
    }
    let view =
        target_view(design, module.as_deref()).map_err(|message| error("CMP036", message, &document.location))?;
    let target_params = match &module {
        Some(module) => &board.module_definitions[module].params,
        None => &board.params,
    };
    let params = effective_params(document, target_params)?;
    let values: IndexMap<String, Quantity> =
        params.iter().map(|(name, param)| (name.clone(), param.value.clone())).collect();
    let value_scope = ValueScope::Bench(&values);

    // Bench components.
    let mut bench_components: Vec<ComponentInstance> = Vec::new();
    let mut bench_locations: Vec<Location> = Vec::new();
    let mut bench_param_uses: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
    let mut used_library: IndexMap<String, PartDefinition> = IndexMap::new();
    for declaration in &document.declarations {
        let Declaration::Component(declaration) = declaration else { continue };
        let reference = &declaration.reference;
        if view.components.iter().any(|component| &component.reference == reference) {
            return Err(error(
                "SIM032",
                format!("bench component {} has the reference of a {owner} component", repr(reference)),
                &declaration.location,
            ));
        }
        if bench_components.iter().any(|component| &component.reference == reference) {
            return Err(error(
                "SIM032",
                format!("duplicate bench component {}", repr(reference)),
                &declaration.location,
            ));
        }
        let part = match (library.get(&declaration.part), view.library.get(&declaration.part)) {
            (Some(part), _) => {
                used_library.insert(declaration.part.clone(), part.clone());
                part
            }
            (None, Some(part)) => part,
            (None, None) => {
                let mut names: Vec<&str> = library.keys().map(String::as_str).collect();
                names.extend(view.library.keys().map(String::as_str));
                return Err(suggesting(
                    error(
                        "SIM033",
                        format!("unknown part {} for bench component {}", repr(&declaration.part), repr(reference)),
                        &declaration.location,
                    ),
                    &declaration.part,
                    &names,
                ));
            }
        };
        let (attributes, bound) = simulation::bind_params(declaration, &values)?;
        let lowered = crate::lower::component(&ComponentDecl { attributes, ..declaration.clone() }, &part.name)?;
        let lowered = ComponentInstance { part: declaration.part.clone(), ..lowered };
        if !bound.is_empty() {
            bench_param_uses.insert(reference.clone(), bound.into_iter().collect());
        }
        bench_components.push(lowered);
        bench_locations.push(declaration.location.clone());
    }

    // Bench supplies: ideal DC sources on target nets (a module's ports).
    let mut bench_supplies: Vec<Supply> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Supply(declaration) = declaration else { continue };
        let location = &declaration.location;
        if let Some(key) = ["source", "external"].into_iter().find(|key| declaration.attributes.contains_key(*key)) {
            return Err(error(
                "SIM048",
                format!("a bench supply is an ideal DC source on a net; it takes voltage and net, not {key}"),
                location,
            ));
        }
        let mut supply = crate::lower::supply(declaration)?;
        if view.supplies.iter().chain(&bench_supplies).any(|other| other.name == supply.name)
            || view.components.iter().chain(&bench_components).any(|component| component.reference == supply.name)
        {
            return Err(error(
                "SIM048",
                format!("bench supply {} has the name of a supply or component", repr(&supply.name)),
                location,
            ));
        }
        let canonical = if view.nets.iter().any(|net| net.name == supply.net) {
            Some(supply.net.clone())
        } else {
            view.net_aliases.get(&supply.net).cloned()
        };
        let Some(net) = canonical else {
            let names: Vec<&str> = view.nets.iter().map(|net| net.name.as_str()).collect();
            return Err(suggesting(
                error(
                    "SIM048",
                    format!(
                        "bench supply {} is on {}, which is no {owner} net; a bench supply drives a net of its target",
                        repr(&supply.name),
                        repr(&supply.net)
                    ),
                    location,
                ),
                &supply.net,
                &names,
            ));
        };
        if let Some(other) = view.supplies.iter().chain(&bench_supplies).find(|other| other.net == net) {
            return Err(error(
                "SIM048",
                format!(
                    "bench supply {} is on net {}, which supply {} drives",
                    repr(&supply.name),
                    repr(&net),
                    repr(&other.name)
                ),
                location,
            ));
        }
        supply.net = net;
        bench_supplies.push(supply);
    }

    // Ignored and replaced board components.
    let mut ignored: IndexMap<String, Location> = IndexMap::new();
    let mut replace_declarations: Vec<&ReplaceDecl> = Vec::new();
    let mut board_names: Vec<&str> = view.components.iter().map(|component| component.reference.as_str()).collect();
    board_names.extend(view.supplies.iter().map(|supply| supply.name.as_str()));
    let check_target = |target: &str, location: &Location, verb: &str| -> Result<()> {
        if view.components.iter().any(|component| component.reference == target)
            || view.supplies.iter().any(|supply| supply.name == target)
        {
            return Ok(());
        }
        if bench_components.iter().any(|component| component.reference == target) {
            return Err(error(
                "SIM038",
                format!("{} is a bench component; remove it from the bench instead of {verb} it", repr(target)),
                location,
            ));
        }
        if bench_supplies.iter().any(|supply| supply.name == target) {
            return Err(error(
                "SIM038",
                format!("{} is a bench supply; change it in the bench instead of {verb} it", repr(target)),
                location,
            ));
        }
        Err(suggesting(
            error("SIM038", format!("cannot {verb} {}: no such {owner} component or supply", repr(target)), location),
            target,
            &board_names,
        ))
    };
    for declaration in &document.declarations {
        match declaration {
            Declaration::Bench(BenchStatement::Ignore(item)) => {
                for (target, location) in item.targets.iter().zip(&item.target_locations) {
                    check_target(target, location, "ignore")?;
                    if ignored.insert(target.clone(), location.clone()).is_some() {
                        return Err(error("SIM038", format!("{} is ignored twice", repr(target)), location));
                    }
                }
            }
            Declaration::Bench(BenchStatement::Replace(item)) => {
                check_target(&item.target, &item.location, "replace")?;
                if replace_declarations.iter().any(|other| other.target == item.target) {
                    return Err(error("SIM038", format!("{} is replaced twice", repr(&item.target)), &item.location));
                }
                replace_declarations.push(item);
            }
            _ => {}
        }
    }
    if let Some(item) = replace_declarations.iter().find(|item| ignored.contains_key(&item.target)) {
        return Err(error("SIM038", format!("{} is both replaced and ignored", repr(&item.target)), &item.location));
    }

    // Supplies and the ground net.
    let all_supplies: Vec<(&Supply, bool)> = view
        .supplies
        .iter()
        .map(|supply| (supply, false))
        .chain(bench_supplies.iter().map(|supply| (supply, true)))
        .collect();
    let grounds: Vec<&Supply> =
        all_supplies.iter().map(|(supply, _)| *supply).filter(|supply| supply.voltage.base_value().is_zero()).collect();
    let ground = match grounds.as_slice() {
        [] if module.is_some() => {
            return Err(error(
                "SIM046",
                "a bench for a module needs a 0 V supply on one of its nets, whose net is the simulation ground: \
                 supply GROUND { voltage = 0V; net = GND; }",
                &target.location,
            ));
        }
        [] => {
            return Err(error(
                "SIM046",
                "a bench needs the board to declare a 0 V supply; its net is the simulation ground",
                &target.location,
            ));
        }
        [first, rest @ ..] if rest.iter().all(|supply| supply.net == first.net) => first.net.clone(),
        _ => {
            let nets: Vec<&str> = grounds.iter().map(|supply| supply.net.as_str()).collect();
            return Err(error(
                "SIM046",
                format!(
                    "the board has 0 V supplies on several nets ({}); a bench needs one ground net",
                    nets.join(", ")
                ),
                &target.location,
            ));
        }
    };
    let mut supplies: Vec<SimSupply> = Vec::new();
    for (supply, bench) in all_supplies {
        let treatment = if supply.net == ground {
            if ignored.contains_key(&supply.name) || replace_declarations.iter().any(|item| item.target == supply.name)
            {
                return Err(error(
                    "SIM046",
                    format!("supply {} is the simulation ground; it cannot be replaced or ignored", repr(&supply.name)),
                    &target.location,
                ));
            }
            SupplyTreatment::Ground
        } else if ignored.contains_key(&supply.name) {
            SupplyTreatment::Ignored
        } else if let Some(index) = replace_declarations.iter().position(|item| item.target == supply.name) {
            SupplyTreatment::Replaced(index)
        } else if let Some(source) = &supply.source {
            SupplyTreatment::Component(source.component.clone())
        } else {
            SupplyTreatment::DcSource
        };
        supplies.push(SimSupply {
            name: supply.name.clone(),
            net: supply.net.clone(),
            voltage: supply.voltage.clone(),
            treatment,
            bench,
        });
    }

    // Every component's treatment.
    let mut components: Vec<SimComponent> = Vec::new();
    let board_param = |reference: &str| -> Option<String> {
        target_params
            .values()
            .find(|param| param.uses.iter().any(|item| item.component == reference && item.attribute == "value"))
            .map(|param| param.name.clone())
    };
    for component in &view.components {
        let reference = &component.reference;
        let treatment = if ignored.contains_key(reference) {
            Treatment::Ignored
        } else if let Some(index) = replace_declarations.iter().position(|item| &item.target == reference) {
            Treatment::Replaced(index)
        } else {
            let part = view.library.get(&component.part).expect("board parts are known");
            let param = board_param(reference);
            // A bench override changes the board component's nominal value.
            let component = match &param {
                Some(name) => ComponentInstance {
                    value: Some(ComponentValue::Quantity(values[name].clone())),
                    ..component.clone()
                },
                None => component.clone(),
            };
            match own_treatment(&component, part, param.as_deref(), &target.location)? {
                Some(treatment) => treatment,
                None => {
                    return Err(error(
                        "SIM034",
                        format!(
                            "{owner} component {} (part {}) has no simulation model; add a sim block to the part, or replace or ignore it in the bench",
                            repr(reference),
                            repr(&component.part)
                        ),
                        &target.location,
                    ));
                }
            }
        };
        components.push(SimComponent {
            reference: reference.clone(),
            part: component.part.clone(),
            bench: false,
            treatment,
        });
    }
    for (component, location) in bench_components.iter().zip(&bench_locations) {
        let part = library.get(&component.part).or_else(|| view.library.get(&component.part)).expect("resolved above");
        let param = bench_param_uses.get(&component.reference).and_then(|uses| uses.get("value")).cloned();
        let Some(treatment) = own_treatment(component, part, param.as_deref(), location)? else {
            return Err(error(
                "SIM034",
                format!(
                    "bench component {} (part {}) has no simulation model; use a passive or a part with a sim block",
                    repr(&component.reference),
                    repr(&component.part)
                ),
                location,
            ));
        };
        components.push(SimComponent {
            reference: component.reference.clone(),
            part: component.part.clone(),
            bench: true,
            treatment,
        });
    }
    // Bench nets: declarations of one name are one net (a bench adds pins to a fixture's net).
    let mut net_declarations: Vec<NetDecl> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Net(declaration) = declaration else { continue };
        match net_declarations.iter_mut().find(|net| net.name == declaration.name) {
            Some(net) => {
                net.endpoints.extend(declaration.endpoints.iter().cloned());
                net.endpoint_locations.extend(declaration.endpoint_locations.iter().cloned());
            }
            None => net_declarations.push(declaration.clone()),
        }
    }
    let mut nets: Vec<BenchNet> = Vec::new();
    for declaration in &net_declarations {
        nets.push(bench_net(declaration, &view, &library, &bench_components, &nets)?);
    }
    for (component, location) in bench_components.iter().zip(&bench_locations) {
        let part = library.get(&component.part).or_else(|| view.library.get(&component.part)).expect("resolved above");
        let treatment =
            &components.iter().find(|item| item.reference == component.reference).expect("pushed").treatment;
        let required: Vec<&str> = match treatment {
            Treatment::PartModel => part.sim.iter().flat_map(|model| model.pins.values().map(String::as_str)).collect(),
            _ => part.pins.keys().map(String::as_str).collect(),
        };
        for pin in required {
            let endpoint = Endpoint::new(&component.reference, pin);
            if !nets.iter().any(|net| net.endpoints.contains(&endpoint)) {
                return Err(error("SIM036", format!("bench component pin {endpoint} is on no bench net"), location));
            }
        }
    }

    let mut model_kinds: Vec<(String, SimKind)> = replace_declarations
        .iter()
        .filter_map(|item| SimKind::parse(&item.kind).map(|kind| (item.target.clone(), kind)))
        .collect();
    for declaration in &document.declarations {
        if let Declaration::Bench(BenchStatement::Model(item)) = declaration
            && let Some(kind) = SimKind::parse(&item.kind)
        {
            model_kinds.push((item.name.clone(), kind));
        }
    }
    let declares_supply_current = |entries: &[SimEntry]| entries.iter().any(|entry| entry.key == "quiescent_current");
    let mut supplied: Vec<String> = replace_declarations
        .iter()
        .filter(|item| declares_supply_current(&item.entries))
        .map(|item| item.target.clone())
        .collect();
    for declaration in &document.declarations {
        if let Declaration::Bench(BenchStatement::Model(item)) = declaration
            && declares_supply_current(&item.entries)
        {
            supplied.push(item.name.clone());
        }
    }
    let context = Context {
        view: &view,
        library: &library,
        components: &components,
        bench_components: &bench_components,
        nets: &nets,
        model_kinds: &model_kinds,
        supplied: &supplied,
        supplies: &supplies,
    };

    // Replacements.
    let mut replacements = Vec::new();
    for item in &replace_declarations {
        let kind = simulation::sim_kind(&item.kind, &item.location)?;
        let lowered = simulation::lower_block(kind, &item.entries, &value_scope, false, &item.location)?;
        let mut nodes = IndexMap::new();
        for (role, (node, location)) in &lowered.roles {
            nodes.insert(role.clone(), context.node(node, location)?);
        }
        replacements.push(Replacement {
            target: item.target.clone(),
            supply: !view.components.iter().any(|component| component.reference == item.target),
            location: item.location.clone(),
            nodes,
            spec: lowered.spec,
        });
    }

    // Bench model instances.
    let mut models: Vec<BenchModel> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Bench(BenchStatement::Model(item)) = declaration else { continue };
        let taken = components.iter().any(|component| component.reference == item.name)
            || supplies.iter().any(|supply| supply.name == item.name)
            || models.iter().any(|model| model.name == item.name);
        if taken {
            return Err(error(
                "SIM049",
                format!("bench model {} has the name of a component, supply or model", repr(&item.name)),
                &item.location,
            ));
        }
        let kind = simulation::sim_kind(&item.kind, &item.location)?;
        let lowered = simulation::lower_block(kind, &item.entries, &value_scope, false, &item.location)?;
        let mut nodes = IndexMap::new();
        for (role, (node, location)) in &lowered.roles {
            nodes.insert(role.clone(), context.node(node, location)?);
        }
        models.push(BenchModel { name: item.name.clone(), location: item.location.clone(), nodes, spec: lowered.spec });
    }

    // The analysis.
    let analyses: Vec<&AnalysisDecl> = document
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Bench(BenchStatement::Analysis(item)) => Some(item),
            _ => None,
        })
        .collect();
    let analysis = match analyses.as_slice() {
        [] => {
            return Err(error(
                "SIM040",
                "a bench needs an analysis: analysis ac { ... } or analysis op;",
                &target.location,
            ));
        }
        [item] if item.kind == "noise" => crate::noise::analysis(
            item,
            &value_scope,
            &|target, location| context.node(target, location),
            &replacements,
            &models,
        )?,
        [item] if item.kind == "tran" => Analysis::Tran(Box::new(crate::transient::analysis(item, &value_scope)?)),
        [item] if item.kind == "dc" => {
            let sources =
                SweepSources { params: &params, supplies: &supplies, replacements: &replacements, models: &models };
            Analysis::Dc(Box::new(dc_analysis(item, &value_scope, &sources)?))
        }
        [item] if item.kind == "loop" => crate::stability::loop_analysis(item, &value_scope, &|endpoint, location| {
            context.loop_probe(endpoint, location)
        })?,
        [item] if item.kind == "pz" => crate::stability::pz_analysis(
            item,
            &|target, location| context.node(target, location),
            &replacements,
            &models,
        )?,
        [item] => analysis(item, &value_scope)?,
        [_, second, ..] => {
            return Err(error("SIM040", "a bench runs one analysis in this version", &second.location));
        }
    };
    let swept = matches!(
        analysis,
        Analysis::Ac { .. } | Analysis::Noise(_) | Analysis::Tran(_) | Analysis::Dc(_) | Analysis::Loop(_)
    );
    let time_domain = matches!(analysis, Analysis::Tran(_));
    let sweep = match &analysis {
        Analysis::Dc(dc) => Some(Units::of(dc.dimension)),
        _ => None,
    };
    let axis = if time_domain {
        "time"
    } else if sweep.is_some() {
        "the dc sweep"
    } else {
        "frequency"
    };
    let noise_scope = match &analysis {
        Analysis::Noise(noise) => Some(noise.scope()),
        _ => None,
    };
    // `noise(X)`: every simulated component and bench model.
    let contributors: Vec<String> = components
        .iter()
        .filter(|component| !matches!(component.treatment, Treatment::Ignored))
        .map(|component| component.reference.clone())
        .chain(models.iter().map(|model| model.name.clone()))
        .collect();

    // References: re-runs of the bench with overridden params.
    let mut references: Vec<Reference> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Bench(BenchStatement::Reference(item)) = declaration else { continue };
        if references.iter().any(|reference| reference.name == item.name) {
            return Err(error("SIM047", format!("duplicate reference {}", repr(&item.name)), &item.location));
        }
        let mut overrides: Vec<(String, SimQuantity)> = Vec::new();
        for entry in &item.entries {
            let Some(param) = values.get(&entry.key) else {
                let names: Vec<&str> = values.keys().map(String::as_str).collect();
                return Err(suggesting(
                    error(
                        "SIM047",
                        format!("reference {} sets {}, which is no param", repr(&item.name), repr(&entry.key)),
                        &entry.location,
                    ),
                    &entry.key,
                    &names,
                ));
            };
            let SimValue::Scalar(value) = &entry.value else {
                return Err(error("SIM047", format!("reference param {} takes one value", entry.key), &entry.location));
            };
            let label = format!("reference {} {}", item.name, entry.key);
            let value = simulation::typed_value(value, param.dimension(), &label, &value_scope, &entry.location)?;
            if overrides.iter().any(|(name, _)| name == &entry.key) {
                return Err(error(
                    "SIM047",
                    format!("reference {} sets {} twice", repr(&item.name), repr(&entry.key)),
                    &entry.location,
                ));
            }
            overrides.push((entry.key.clone(), value));
        }
        if overrides.is_empty() {
            return Err(error(
                "SIM047",
                format!(
                    "reference {} sets no param; a reference re-runs the bench with other params",
                    repr(&item.name)
                ),
                &item.location,
            ));
        }
        references.push(Reference { name: item.name.clone(), overrides, location: item.location.clone() });
    }
    let reference_names: Vec<String> = references.iter().map(|reference| reference.name.clone()).collect();
    let mut referencing: Vec<String> = Vec::new();

    // Steps, lets, measures and plots, in source order.
    let mut steps: Vec<Step> = Vec::new();
    let mut lets: Vec<Let> = Vec::new();
    let mut let_types: IndexMap<String, ExpressionType> = IndexMap::new();
    let mut measures: Vec<Measure> = Vec::new();
    let mut plots: Vec<Plot> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Bench(statement) = declaration else { continue };
        let scope = ExpressionScope {
            context: &context,
            swept,
            params: &values,
            lets: &let_types,
            definitions: &lets,
            referencing: &referencing,
            references: &reference_names,
            noise: noise_scope,
            contributors: &contributors,
            time_domain,
            sweep,
            analysis: analysis.name(),
        };
        match statement {
            BenchStatement::Step(item) => {
                let columns = std::iter::once((&item.param, &item.values)).chain(item.zip.iter().map(|(n, v)| (n, v)));
                let mut typed: Vec<(String, Vec<Quantity>)> = Vec::new();
                for (name, written) in columns {
                    let Some(param) = values.get(name) else {
                        let names: Vec<&str> = values.keys().map(String::as_str).collect();
                        return Err(suggesting(
                            error("SIM041", format!("step names {}, which is no param", repr(name)), &item.location),
                            name,
                            &names,
                        ));
                    };
                    if params[name].derived.is_some() {
                        return Err(error(
                            "SIM041",
                            format!(
                                "param {} is derived from other params; step the params it is computed from",
                                repr(name)
                            ),
                            &item.location,
                        ));
                    }
                    if steps.iter().any(|step| step.params().contains(&name.as_str()))
                        || typed.iter().any(|(other, _)| other == name)
                    {
                        return Err(error("SIM041", format!("param {} is stepped twice", repr(name)), &item.location));
                    }
                    let label = format!("step {name}");
                    let column = match &item.range {
                        Some(range) => step_range(range, param.dimension(), &label, &item.location)?,
                        None => written
                            .iter()
                            .map(|value| {
                                simulation::typed_value(
                                    value,
                                    param.dimension(),
                                    &label,
                                    &ValueScope::Part,
                                    &item.location,
                                )
                                .map(|value| value.value)
                            })
                            .collect::<Result<Vec<_>>>()?,
                    };
                    typed.push((name.clone(), column));
                }
                let mut typed = typed.into_iter();
                let (param, first) = typed.next().expect("a step names a param");
                steps.push(Step { param, values: first, zip: typed.collect(), location: item.location.clone() });
            }
            BenchStatement::Let(item) => {
                check_name(&item.name, &item.location, &let_types, &measures, &values)?;
                let expression = expression::check(&item.expression, &scope)?;
                crate::transient::check_spectrum_use(&expression, "a let", false)?;
                let uses_reference =
                    expression::uses_reference(&expression, &|name| referencing.iter().any(|item| item == name));
                let_types.insert(item.name.clone(), expression.ty);
                if uses_reference {
                    referencing.push(item.name.clone());
                }
                lets.push(Let { name: item.name.clone(), expression, location: item.location.clone(), uses_reference });
            }
            BenchStatement::Measure(item) => {
                check_name(&item.name, &item.location, &let_types, &measures, &values)?;
                let expression = expression::check(&item.expression, &scope)?;
                crate::transient::check_spectrum_use(&expression, "a measure", false)?;
                if expression.ty.swept {
                    return Err(error(
                        "SIM043",
                        if time_domain {
                            format!(
                                "measure {} varies over time; reduce it with min, max, mean, rms, pp or at(x, t)",
                                item.name
                            )
                        } else if sweep.is_some() {
                            format!(
                                "measure {} varies over the dc sweep; reduce it with min, max, at(x, v), cross or upto",
                                item.name
                            )
                        } else {
                            format!(
                                "measure {} is swept over frequency; reduce it with min, max, mean or at(x, f)",
                                item.name
                            )
                        },
                        &item.location,
                    ));
                }
                if expression.ty.complex {
                    return Err(error(
                        "SIM043",
                        format!("measure {} is complex; take mag, phase, re or im", item.name),
                        &item.location,
                    ));
                }
                let (min, max) = limits(item, expression.ty.units, &values)?;
                let conditions = step_conditions(item, &params, min.is_some() || max.is_some())?;
                measures.push(Measure {
                    name: item.name.clone(),
                    expression,
                    min,
                    max,
                    conditions,
                    location: item.location.clone(),
                });
            }
            BenchStatement::Plot(item) => {
                let mut expressions = Vec::new();
                for written in &item.expressions {
                    let expression = expression::check(written, &scope)?;
                    crate::transient::check_spectrum_use(&expression, "an expression of a plot", true)?;
                    if !expression.ty.swept {
                        let reason = if swept {
                            &format!("it does not vary with {axis}")
                        } else if matches!(analysis, Analysis::Pz(_)) {
                            "analysis pz gives single values (its poles are in the results and on the page)"
                        } else {
                            "analysis op gives single values"
                        };
                        return Err(error(
                            "SIM044",
                            format!("plot needs a swept value, but {reason}"),
                            &written.location,
                        ));
                    }
                    if expression.ty.complex {
                        return Err(error(
                            "SIM044",
                            "plot needs a real value; plot mag, phase, db, re or im of a complex value",
                            &written.location,
                        ));
                    }
                    expressions.push(expression);
                }
                plots.push(Plot { expressions, location: item.location.clone() });
            }
            BenchStatement::Target(_)
            | BenchStatement::Replace(_)
            | BenchStatement::Model(_)
            | BenchStatement::Ignore(_)
            | BenchStatement::Analysis(_)
            | BenchStatement::Reference(_)
            | BenchStatement::Variation(_)
            | BenchStatement::InitialCondition(_) => {}
        }
    }
    // A loop analysis that plots nothing shows its Bode plot.
    if let (Analysis::Loop(_), true) = (&analysis, plots.is_empty()) {
        let location = analyses.first().map_or(&document.location, |item| &item.location);
        plots.push(crate::stability::default_loop_plot(location));
    }
    // The built-in measure of a pole-zero analysis: no pole in the right half-plane.
    if let Analysis::Pz(_) = &analysis {
        if let Some(measure) = measures.iter().find(|measure| measure.name == crate::stability::STABLE_MEASURE) {
            return Err(error(
                "SIM100",
                "measure stable is the built-in measure of analysis pz (1 when no pole is in the right half-plane); name yours otherwise",
                &measure.location,
            ));
        }
        let location = analyses.first().map_or(&document.location, |item| &item.location);
        measures.push(crate::stability::stable_measure(location));
    }
    check_temperature(&params, &analysis, &steps)?;
    let initial_conditions = initial_conditions(document, &analysis, &context, &value_scope)?;
    let part_of = |name: &str| library.get(name).or_else(|| view.library.get(name));
    let lookup = crate::tolerance::Lookup {
        components: &components,
        instances: view.components.iter().chain(&bench_components).collect(),
        params: &params,
        replacements: &replacements,
        models: &models,
        part: &part_of,
    };
    let variation = crate::tolerance::variation(document, &lookup)?;

    // Module parts are not in the board's library: keep the ones the target's components use.
    let target_library = view
        .library
        .iter()
        .filter(|(name, _)| !board.library.contains_key(*name))
        .filter(|(name, _)| view.components.iter().any(|component| &component.part == *name))
        .map(|(name, part)| (name.clone(), part.clone()))
        .collect();
    Ok(Bench {
        name: document.name.clone(),
        board: board.name.clone(),
        module,
        location: document.location.clone(),
        params,
        bench_components,
        bench_param_uses,
        library: used_library,
        target_library,
        components,
        supplies,
        ground,
        nets,
        replacements,
        models,
        analysis,
        steps,
        references,
        lets,
        measures,
        plots,
        variation,
        initial_conditions,
        fixtures: target.fixtures.clone(),
        section: None,
        model_choice: simulation::ModelChoice::Default,
    })
}

/// The most values of a stepped range.
pub const STEP_RANGE_VALUES: usize = 1000;

/// The values of `step P from a to b by s;` (from `a` towards `b` by `s`,
/// with `b` itself when the steps do not land on it) or `step P from a to b
/// points n;` (`n` evenly spaced values, both ends included), exactly in
/// decimals (`SIM115`).
fn step_range(range: &StepRange, dimension: Dimension, label: &str, location: &Location) -> Result<Vec<Quantity>> {
    let typed = |value: &Scalar| {
        simulation::typed_value(value, dimension, label, &ValueScope::Part, location).map(|value| value.value)
    };
    let (from, to) = (typed(&range.from)?, typed(&range.to)?);
    let unit = from.display_unit();
    let quantity = |base: Decimal| {
        Quantity::from_base(dimension, base, unit).map_err(|failure| error("SIM115", failure.to_string(), location))
    };
    let arithmetic = |failure: copper_units::DecimalError| error("SIM115", failure.to_string(), location);
    let (a, b) = (from.base_value().clone(), to.base_value().clone());
    if a == b {
        return Err(error(
            "SIM115",
            format!("{label} runs from {from} to {to}; a range needs two different ends"),
            location,
        ));
    }
    let span = b.sub(&a).map_err(arithmetic)?;
    let mut values = Vec::new();
    match (&range.by, &range.points) {
        (Some(by), _) => {
            let by = typed(by)?;
            let step = by.base_value().clone();
            if step.is_negative() || step.is_zero() {
                return Err(error("SIM115", format!("{label} steps by {by}; the spacing must be positive"), location));
            }
            let count = span.abs().div(&step).map_err(arithmetic)?;
            let whole = count.to_integral_value(copper_units::Rounding::Down);
            if whole.to_i64().is_none_or(|n| n as usize >= STEP_RANGE_VALUES) {
                return Err(error(
                    "SIM115",
                    format!("{label} from {from} to {to} by {by} is more than {STEP_RANGE_VALUES} values"),
                    location,
                ));
            }
            let signed = if span.is_negative() { step.copy_negate() } else { step };
            let mut value = a.clone();
            for _ in 0..=whole.to_i64().unwrap_or(0) {
                values.push(quantity(value.clone())?);
                value = value.add(&signed).map_err(arithmetic)?;
            }
            if values.last().is_none_or(|last| *last.base_value() != b) {
                values.push(quantity(b.clone())?);
            }
        }
        (None, Some(points)) => {
            let n = match points {
                Scalar::Int(n) => usize::try_from(n.clone()).ok().filter(|n| (2..=STEP_RANGE_VALUES).contains(n)),
                _ => None,
            };
            let Some(n) = n else {
                return Err(error(
                    "SIM115",
                    format!("{label} points is a whole number of values from 2 to {STEP_RANGE_VALUES}, ends included"),
                    location,
                ));
            };
            let intervals = Decimal::from_i64(n as i64 - 1);
            for k in 0..n {
                let offset =
                    span.mul(&Decimal::from_i64(k as i64)).map_err(arithmetic)?.div(&intervals).map_err(arithmetic)?;
                values.push(quantity(if k + 1 == n { b.clone() } else { a.add(&offset).map_err(arithmetic)? })?);
            }
        }
        (None, None) => {
            return Err(error("SIM115", format!("{label} needs by or points"), location));
        }
    }
    Ok(values)
}

/// The `where` of a measure's limits: comparisons of params, checked like
/// derived params (numbers, quantities, params and `+ - * /`; `SIM072`).
fn step_conditions(
    item: &MeasureDecl,
    params: &IndexMap<String, BenchParam>,
    limited: bool,
) -> Result<Vec<Expression>> {
    if item.conditions.is_empty() {
        return Ok(Vec::new());
    }
    if !limited {
        return Err(error(
            "SIM072",
            format!("measure {} has a where but no min or max for it to scope", item.name),
            &item.conditions[0].location,
        ));
    }
    let scope = ParamScope { params, what: "a limit's where" };
    let mut out = Vec::new();
    for condition in &item.conditions {
        let checked = expression::check_comparison(condition, &scope)?;
        let ExpressionKind::Compare { left, right, .. } = &checked.kind else { unreachable!("a comparison") };
        if !arithmetic(left) || !arithmetic(right) {
            return Err(error(
                "SIM072",
                format!("measure {}: a where compares params, numbers and quantities with + - * / only", item.name),
                &condition.location,
            ));
        }
        out.push(checked);
    }
    Ok(out)
}

/// What `analysis dc` may sweep.
struct SweepSources<'a> {
    params: &'a IndexMap<String, BenchParam>,
    supplies: &'a [SimSupply],
    replacements: &'a [Replacement],
    models: &'a [BenchModel],
}

/// `analysis dc { sweep = NAME; from = q; to = q; step = q; }` or `points = N` (`SIM074`).
fn dc_analysis(item: &AnalysisDecl, scope: &ValueScope, sources: &SweepSources) -> Result<DcAnalysis> {
    let properties = crate::sim_reference::option_names("dc");
    let mut name: Option<(String, Location)> = None;
    let mut written: IndexMap<&str, (&Scalar, &Location)> = IndexMap::new();
    for entry in &item.entries {
        let SimValue::Scalar(value) = &entry.value else {
            return Err(error("SIM074", format!("analysis dc {} takes one value", entry.key), &entry.location));
        };
        match entry.key.as_str() {
            "sweep" => match value {
                Scalar::Str(text) => name = Some((text.clone(), entry.location.clone())),
                _ => {
                    return Err(error(
                        "SIM074",
                        "analysis dc sweep names a param, a source model or a supply: sweep = V_IN;",
                        &entry.location,
                    ));
                }
            },
            key @ ("from" | "to" | "step" | "points") => {
                written.insert(key, (value, &entry.location));
            }
            other => {
                return Err(suggesting(
                    error(
                        "SIM074",
                        format!(
                            "unknown analysis dc property {}; expected one of: {}",
                            repr(other),
                            properties.join(", ")
                        ),
                        &entry.location,
                    ),
                    other,
                    &properties,
                ));
            }
        }
    }
    let Some((name, name_location)) = name else {
        return Err(error(
            "SIM074",
            "analysis dc needs what it sweeps: analysis dc { sweep = V_IN; from = -1V; to = 1V; step = 10mV; }",
            &item.location,
        ));
    };
    let source_dimension = |spec: &SimSpec| spec.output.unwrap_or(simulation::SourceOutput::Voltage).dimension();
    let (sweep, dimension) = if let Some(param) = sources.params.get(&name) {
        if param.derived.is_some() {
            return Err(error(
                "SIM074",
                format!("param {} is derived from other params; sweep a param it is computed from", repr(&name)),
                &name_location,
            ));
        }
        (DcSweep::Param(name.clone()), param.value.dimension())
    } else if let Some(model) = sources.models.iter().find(|model| model.name == name) {
        if model.spec.kind != SimKind::Source {
            return Err(error(
                "SIM074",
                format!(
                    "bench model {} is a {}; analysis dc sweeps a source's dc value",
                    repr(&name),
                    model.spec.kind.name()
                ),
                &name_location,
            ));
        }
        (DcSweep::Source(name.clone()), source_dimension(&model.spec))
    } else if let Some(replacement) = sources.replacements.iter().find(|item| item.target == name) {
        if replacement.spec.kind != SimKind::Source {
            return Err(error(
                "SIM074",
                format!(
                    "{} is replaced with a {}; analysis dc sweeps a source's dc value",
                    repr(&name),
                    replacement.spec.kind.name()
                ),
                &name_location,
            ));
        }
        let sweep =
            if replacement.supply { DcSweep::ReplacedSupply(name.clone()) } else { DcSweep::Source(name.clone()) };
        (sweep, source_dimension(&replacement.spec))
    } else if let Some(supply) = sources.supplies.iter().find(|supply| supply.name == name) {
        if supply.treatment != SupplyTreatment::DcSource {
            return Err(error(
                "SIM074",
                format!(
                    "supply {} is {} in this bench; analysis dc sweeps a supply simulated as its DC source",
                    repr(&name),
                    supply.treatment.name()
                ),
                &name_location,
            ));
        }
        (DcSweep::Supply(name.clone()), Dimension::Voltage)
    } else {
        let mut names: Vec<&str> = sources.params.keys().map(String::as_str).collect();
        names.extend(sources.models.iter().map(|model| model.name.as_str()));
        names.extend(sources.replacements.iter().map(|item| item.target.as_str()));
        names.extend(sources.supplies.iter().map(|supply| supply.name.as_str()));
        return Err(suggesting(
            error(
                "SIM074",
                format!(
                    "analysis dc sweeps {}, which is no param, source model, replaced source or supply of the bench",
                    repr(&name)
                ),
                &name_location,
            ),
            &name,
            &names,
        ));
    };
    let typed = |key: &str| -> Result<Option<SimQuantity>> {
        let Some((value, location)) = written.get(key) else { return Ok(None) };
        simulation::typed_value(value, dimension, &format!("analysis dc {key}"), scope, location).map(Some)
    };
    let (Some(from), Some(to)) = (typed("from")?, typed("to")?) else {
        return Err(error("SIM074", "analysis dc needs from and to", &item.location));
    };
    let step = typed("step")?;
    let points = match written.get("points") {
        None => None,
        Some((Scalar::Int(number), _)) if *number >= 2.into() && *number <= 100000.into() => {
            Some(u32::try_from(number.clone()).expect("bounded"))
        }
        Some((_, location)) => {
            return Err(error("SIM074", "analysis dc points must be a whole number from 2 to 100000", location));
        }
    };
    match (&step, points) {
        (Some(_), Some(_)) => {
            return Err(error("SIM074", "analysis dc takes step or points, not both", &item.location));
        }
        (None, None) => {
            return Err(error("SIM074", "analysis dc needs step (the spacing) or points (the count)", &item.location));
        }
        (Some(step), None) if !(step.value.base_value() > &Decimal::parse("0").expect("a decimal")) => {
            return Err(error(
                "SIM074",
                format!("analysis dc step must be positive, found {}; the sweep runs from from to to", step.value),
                &item.location,
            ));
        }
        _ => {}
    }
    if from.value.base_value() == to.value.base_value() {
        return Err(error(
            "SIM074",
            format!("analysis dc needs from and to to differ, both are {}", from.value),
            &item.location,
        ));
    }
    Ok(DcAnalysis { name, sweep, dimension, from, to, step, points })
}

/// The bench's `ic` statements: a node voltage, a capacitor's voltage or an
/// inductor's current at the start of `analysis tran` (`SIM075`).
fn initial_conditions(
    document: &Document,
    analysis: &Analysis,
    context: &Context,
    scope: &ValueScope,
) -> Result<Vec<InitialCondition>> {
    let mut out: Vec<InitialCondition> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::Bench(BenchStatement::InitialCondition(item)) = declaration else { continue };
        let Analysis::Tran(tran) = analysis else {
            return Err(error(
                "SIM075",
                format!("ic sets the start of a transient run; the bench's analysis is {}", analysis.name()),
                &item.location,
            ));
        };
        if tran.steady.is_some() {
            return Err(error(
                "SIM112",
                "analysis tran steady finds the periodic state the run starts in, so it takes no ic; leave the ic out or the steady",
                &item.location,
            ));
        }
        let (target, dimension, what) = match &item.target {
            InitialConditionTarget::Node(node) => {
                if tran.uic {
                    return Err(error(
                        "SIM075",
                        "with uic the run starts from capacitor voltages and inductor currents, not node voltages; \
                         set the capacitor on the node instead: ic C1 = 1V;",
                        &item.location,
                    ));
                }
                (
                    InitialTarget::Node(context.node(node, &item.target_location)?),
                    Dimension::Voltage,
                    format!("ic V({node})"),
                )
            }
            InitialConditionTarget::Component(reference) => {
                let component = context.components.iter().find(|component| &component.reference == reference);
                match component.map(|component| &component.treatment) {
                    Some(Treatment::Passive { kind: PassiveKind::Capacitor, .. }) => {
                        (InitialTarget::Capacitor(reference.clone()), Dimension::Voltage, format!("ic {reference}"))
                    }
                    Some(Treatment::Passive { kind: PassiveKind::Inductor, .. }) => {
                        (InitialTarget::Inductor(reference.clone()), Dimension::Current, format!("ic {reference}"))
                    }
                    Some(_) => {
                        return Err(error(
                            "SIM075",
                            format!(
                                "ic {reference} sets a capacitor's voltage or an inductor's current; {} is neither in this bench",
                                repr(reference)
                            ),
                            &item.target_location,
                        ));
                    }
                    None => {
                        let names: Vec<&str> = context
                            .components
                            .iter()
                            .filter(|component| {
                                matches!(
                                    component.treatment,
                                    Treatment::Passive { kind: PassiveKind::Capacitor | PassiveKind::Inductor, .. }
                                )
                            })
                            .map(|component| component.reference.as_str())
                            .collect();
                        return Err(suggesting(
                            error(
                                "SIM075",
                                format!(
                                    "unknown component {} in ic; expected a capacitor, an inductor or V(NODE)",
                                    repr(reference)
                                ),
                                &item.target_location,
                            ),
                            reference,
                            &names,
                        ));
                    }
                }
            }
        };
        if out.iter().any(|other| other.target == target) {
            return Err(error("SIM075", format!("{what} is set twice"), &item.location));
        }
        let value = simulation::typed_value(&item.value, dimension, &what, scope, &item.location)?;
        out.push(InitialCondition { target, value, location: item.location.clone() });
    }
    Ok(out)
}

fn check_name(
    name: &str,
    location: &Location,
    lets: &IndexMap<String, ExpressionType>,
    measures: &[Measure],
    params: &IndexMap<String, Quantity>,
) -> Result<()> {
    if lets.contains_key(name) || measures.iter().any(|measure| measure.name == name) {
        return Err(error("SIM042", format!("{} is already a let or a measure", repr(name)), location));
    }
    if params.contains_key(name) {
        return Err(error("SIM042", format!("{} is already a param", repr(name)), location));
    }
    Ok(())
}

fn bench_net(
    declaration: &NetDecl,
    view: &FlatElectricalView,
    library: &IndexMap<String, PartDefinition>,
    bench_components: &[ComponentInstance],
    earlier: &[BenchNet],
) -> Result<BenchNet> {
    let location = &declaration.location;
    let name = &declaration.name;
    if earlier.iter().any(|net| &net.name == name) {
        return Err(error("SIM037", format!("duplicate bench net {}", repr(name)), location));
    }
    let part = |part: &str| library.get(part).or_else(|| view.library.get(part));
    let mut endpoints: Vec<Endpoint> = Vec::new();
    for (text, endpoint_location) in declaration.endpoints.iter().zip(&declaration.endpoint_locations) {
        if text.ends_with('*') {
            return Err(error("SIM035", "pin patterns are not supported in bench nets", endpoint_location));
        }
        let endpoint = Endpoint::parse(text).map_err(|message| error("SIM035", message, endpoint_location))?;
        let Some(component) = bench_components.iter().find(|component| component.reference == endpoint.component)
        else {
            let reason = if view.components.iter().any(|component| component.reference == endpoint.component) {
                format!(
                    "{} is a board component; a bench net joins the board by its name (net {text} {{ ... }}), not by listing board pins",
                    repr(&endpoint.component)
                )
            } else {
                format!("{} is no bench component", repr(&endpoint.component))
            };
            return Err(error("SIM035", format!("bench net {} lists {text}: {reason}", repr(name)), endpoint_location));
        };
        let definition = part(&component.part).expect("bench parts are known");
        if !definition.pins.contains_key(&endpoint.pin) {
            let pins: Vec<&str> = definition.pins.keys().map(String::as_str).collect();
            return Err(suggesting(
                error(
                    "SIM035",
                    format!("{} has no pin {}", repr(&endpoint.component), repr(&endpoint.pin)),
                    endpoint_location,
                ),
                &endpoint.pin,
                &pins,
            ));
        }
        if endpoints.contains(&endpoint) || earlier.iter().any(|net| net.endpoints.contains(&endpoint)) {
            return Err(error("SIM035", format!("bench pin {endpoint} is on two bench nets"), endpoint_location));
        }
        endpoints.push(endpoint);
    }
    let join = if view.nets.iter().any(|net| &net.name == name) {
        NetJoin::BoardNet(name.clone())
    } else if let Some(canonical) = view.net_aliases.get(name) {
        NetJoin::BoardNet(canonical.clone())
    } else if let Ok(endpoint) = Endpoint::parse(name) {
        let Some(component) = view.components.iter().find(|component| component.reference == endpoint.component) else {
            let reason = if bench_components.iter().any(|component| component.reference == endpoint.component) {
                "bench components join by being listed in a net, so give this net a name"
            } else {
                "a net named COMPONENT.PIN joins the board net at that board pin"
            };
            return Err(error(
                "SIM037",
                format!("bench net {}: {} is no board component; {reason}", repr(name), repr(&endpoint.component)),
                location,
            ));
        };
        match view.nets.iter().find(|net| net.endpoints.contains(&endpoint)) {
            Some(net) => NetJoin::BoardNet(net.name.clone()),
            None => {
                let definition = part(&component.part).expect("board parts are known");
                if !definition.pins.contains_key(&endpoint.pin) {
                    let pins: Vec<&str> = definition.pins.keys().map(String::as_str).collect();
                    return Err(suggesting(
                        error(
                            "SIM037",
                            format!(
                                "bench net {}: {} has no pin {}",
                                repr(name),
                                repr(&endpoint.component),
                                repr(&endpoint.pin)
                            ),
                            location,
                        ),
                        &endpoint.pin,
                        &pins,
                    ));
                }
                NetJoin::BoardPin(endpoint)
            }
        }
    } else {
        if endpoints.len() < 2 {
            let names: Vec<&str> = view.nets.iter().map(|net| net.name.as_str()).collect();
            return Err(suggesting(
                error(
                    "SIM037",
                    format!(
                        "bench net {} is no board net, so it is a new net, and a new net needs two or more pins",
                        repr(name)
                    ),
                    location,
                ),
                name,
                &names,
            ));
        }
        NetJoin::New
    };
    let node = match &join {
        NetJoin::BoardNet(net) => Node::Net(net.clone()),
        NetJoin::BoardPin(endpoint) => Node::Pin(endpoint.clone()),
        NetJoin::New => Node::Net(name.clone()),
    };
    if let Some(other) = earlier.iter().find(|net| net.node == node) {
        return Err(error(
            "SIM037",
            format!("bench nets {} and {} both join {node}; list their pins in one net", repr(&other.name), repr(name)),
            location,
        ));
    }
    Ok(BenchNet { name: name.clone(), location: location.clone(), join, node, endpoints })
}

fn analysis(item: &AnalysisDecl, scope: &ValueScope) -> Result<Analysis> {
    match item.kind.as_str() {
        "op" => {
            if let Some(entry) = item.entries.first() {
                return Err(error("SIM040", "analysis op takes no properties", &entry.location));
            }
            Ok(Analysis::Op)
        }
        "ac" => {
            let mut from = None;
            let mut to = None;
            let mut points = None;
            for entry in &item.entries {
                let SimValue::Scalar(value) = &entry.value else {
                    return Err(error("SIM040", format!("analysis ac {} takes one value", entry.key), &entry.location));
                };
                match entry.key.as_str() {
                    "from" => {
                        from = Some(simulation::typed_value(
                            value,
                            Dimension::Frequency,
                            "analysis ac from",
                            scope,
                            &entry.location,
                        )?)
                    }
                    "to" => {
                        to = Some(simulation::typed_value(
                            value,
                            Dimension::Frequency,
                            "analysis ac to",
                            scope,
                            &entry.location,
                        )?)
                    }
                    "points_per_decade" => match value {
                        Scalar::Int(number) if *number > 0.into() && *number <= 10000.into() => {
                            points = Some(u32::try_from(number.clone()).expect("bounded"));
                        }
                        _ => {
                            return Err(error(
                                "SIM040",
                                "analysis ac points_per_decade must be a whole number from 1 to 10000",
                                &entry.location,
                            ));
                        }
                    },
                    other => {
                        return Err(suggesting(
                            error("SIM040", format!("unknown analysis ac property {}", repr(other)), &entry.location),
                            other,
                            &crate::sim_reference::option_names("ac"),
                        ));
                    }
                }
            }
            let (Some(from), Some(to), Some(points_per_decade)) = (from, to, points) else {
                return Err(error("SIM040", "analysis ac needs from, to and points_per_decade", &item.location));
            };
            if from.value.base_value().is_zero()
                || from.value.base_value().is_negative()
                || to.value.base_value() <= from.value.base_value()
            {
                return Err(error(
                    "SIM040",
                    format!("analysis ac needs 0 < from < to, found from {} to {}", from.value, to.value),
                    &item.location,
                ));
            }
            Ok(Analysis::Ac { from, to, points_per_decade })
        }
        other => Err(suggesting(
            error(
                "SIM040",
                format!("unknown analysis {}; expected ac, dc, loop, noise, op, pz or tran", repr(other)),
                &item.location,
            ),
            other,
            &crate::sim_reference::ANALYSES.iter().map(|kind| kind.name).collect::<Vec<_>>(),
        )),
    }
}

/// A measure's `min` and `max`, of the measure's units.
fn limits(
    item: &MeasureDecl,
    units: Units,
    params: &IndexMap<String, Quantity>,
) -> Result<(Option<SimQuantity>, Option<SimQuantity>)> {
    let mut min = None;
    let mut max = None;
    for entry in &item.limits {
        let SimValue::Scalar(value) = &entry.value else {
            return Err(error("SIM043", format!("measure limit {} takes one value", entry.key), &entry.location));
        };
        let limit = match value {
            Scalar::Quantity { value, unit } => match crate::expression::sim_quantity(value, unit) {
                Some(quantity) => SimQuantity { value: quantity, param: None },
                None => {
                    return Err(error(
                        "SIM043",
                        format!("unknown unit {} in measure {}", repr(unit), item.name),
                        &entry.location,
                    ));
                }
            },
            Scalar::Int(number) => SimQuantity { value: ratio(&number.to_string()), param: None },
            Scalar::Float(number) => SimQuantity { value: ratio(&number.to_string()), param: None },
            Scalar::Str(name) if params.contains_key(name) => {
                SimQuantity { value: params[name].clone(), param: Some(name.clone()) }
            }
            _ => {
                return Err(error(
                    "SIM043",
                    format!("measure {} {} must be a quantity, a number or a param", item.name, entry.key),
                    &entry.location,
                ));
            }
        };
        let limit_units = Units::of(limit.value.dimension());
        if limit_units != units {
            return Err(error(
                "SIM043",
                format!(
                    "measure {} is {units}, but its {} limit {} is {limit_units}",
                    item.name, entry.key, limit.value
                ),
                &entry.location,
            ));
        }
        match entry.key.as_str() {
            "min" => min = Some(limit),
            "max" => max = Some(limit),
            other => {
                return Err(suggesting(
                    error(
                        "SIM043",
                        format!("unknown measure limit {}; expected min or max", repr(other)),
                        &entry.location,
                    ),
                    other,
                    &["min", "max"],
                ));
            }
        }
    }
    if let (Some(low), Some(high)) = (&min, &max)
        && low.value.base_value() > high.value.base_value()
    {
        return Err(error(
            "SIM043",
            format!("measure {} has min {} above max {}", item.name, low.value, high.value),
            &item.location,
        ));
    }
    Ok((min, max))
}

/// A plain number as a dimensionless quantity (base value the number itself).
fn ratio(text: &str) -> Quantity {
    let number = Decimal::parse(text).expect("numbers are valid decimals");
    Quantity::from_base(Dimension::Ratio, number, "%").expect("% is the ratio unit")
}

// JSON.

fn text(value: &str) -> Value {
    Value::from(value)
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn sim_quantity_value(value: &SimQuantity) -> Value {
    let mut entries = vec![("quantity", quantity_value(&value.value))];
    if let Some(param) = &value.param {
        entries.push(("param", text(param)));
    }
    object(entries)
}

fn optional<T>(value: &Option<T>, convert: impl Fn(&T) -> Value) -> Value {
    value.as_ref().map(convert).unwrap_or(Value::Null)
}

/// The JSON of a [`SimSpec`]: kind, output, parameters and waveform.
pub fn spec_fields(spec: &SimSpec) -> Vec<(&'static str, Value)> {
    let mut entries = vec![("kind", text(spec.kind.name()))];
    if let Some(model) = &spec.spice {
        let (source, sha256, member, file) = match &model.source {
            simulation::SpiceSource::Url { url, sha256, member } => {
                (text(url), text(sha256), member.as_deref().map_or(Value::Null, text), Value::Null)
            }
            simulation::SpiceSource::Registered { sha256, member, .. } => {
                (Value::Null, text(sha256), member.as_deref().map_or(Value::Null, text), Value::Null)
            }
            simulation::SpiceSource::File(file) => (Value::Null, Value::Null, Value::Null, text(file)),
        };
        let mut fields = vec![
            ("source", source),
            ("sha256", sha256),
            ("member", member),
            ("file", file),
            ("subckt", text(&model.subckt)),
            ("dialect", text(&model.dialect)),
        ];
        if let simulation::SpiceSource::Registered { download, .. } = &model.source {
            fields.push(("download", text(download)));
        }
        if let Some(note) = &model.note {
            fields.push(("note", text(note)));
        }
        entries.push(("spice", object(fields)));
    }
    if let Some(output) = spec.output {
        entries.push(("output", text(output.name())));
    } else if spec.kind == SimKind::Source {
        entries.push(("output", text("voltage")));
    }
    entries.push((
        "parameters",
        Value::Object(spec.parameters.iter().map(|(key, value)| (key.clone(), sim_quantity_value(value))).collect()),
    ));
    let waveform = match &spec.waveform {
        None => None,
        Some(Waveform::Sine { offset, amplitude, frequency, phase, delay }) => {
            let mut fields = vec![
                ("kind", text("sine")),
                ("offset", optional(offset, sim_quantity_value)),
                ("amplitude", sim_quantity_value(amplitude)),
                ("frequency", sim_quantity_value(frequency)),
                ("phase", optional(phase, sim_quantity_value)),
            ];
            if let Some(delay) = delay {
                fields.push(("delay", sim_quantity_value(delay)));
            }
            Some(object(fields))
        }
        Some(Waveform::Pulse { initial, pulsed, delay, rise, fall, width, period, cycles }) => Some(object(vec![
            ("kind", text("pulse")),
            ("initial", sim_quantity_value(initial)),
            ("pulsed", sim_quantity_value(pulsed)),
            ("delay", optional(delay, sim_quantity_value)),
            ("rise", optional(rise, sim_quantity_value)),
            ("fall", optional(fall, sim_quantity_value)),
            ("width", sim_quantity_value(width)),
            ("period", optional(period, sim_quantity_value)),
            ("cycles", cycles.map(|n| Value::from(i64::from(n))).unwrap_or(Value::Null)),
        ])),
        Some(Waveform::Pwl { points }) => Some(object(vec![
            ("kind", text("pwl")),
            (
                "points",
                Value::Array(
                    points
                        .iter()
                        .map(|(time, value)| Value::Array(vec![sim_quantity_value(time), sim_quantity_value(value)]))
                        .collect(),
                ),
            ),
        ])),
        Some(Waveform::Step { initial, final_value, at, rise }) => Some(object(vec![
            ("kind", text("step")),
            ("initial", optional(initial, sim_quantity_value)),
            ("final", sim_quantity_value(final_value)),
            ("at", optional(at, sim_quantity_value)),
            ("rise", optional(rise, sim_quantity_value)),
        ])),
    };
    if let Some(waveform) = waveform {
        entries.push(("waveform", waveform));
    }
    entries
}

fn node_value(node: &Node) -> Value {
    match node {
        Node::Net(name) => object(vec![("net", text(name))]),
        Node::Pin(endpoint) => object(vec![("pin", text(&endpoint.to_string()))]),
    }
}

fn type_value(ty: &ExpressionType) -> Value {
    object(vec![
        ("units", text(&ty.units.base_text())),
        ("dimension", ty.units.dimension().map(|dimension| text(dimension.name())).unwrap_or(Value::Null)),
        ("complex", Value::Bool(ty.complex)),
        ("swept", Value::Bool(ty.swept)),
    ])
}

/// The JSON of a typed expression tree.
pub fn expression_value(expression: &Expression) -> Value {
    let mut entries = match &expression.kind {
        ExpressionKind::Number(number) => vec![("number", text(&number.decimal_text()))],
        ExpressionKind::Quantity(quantity) => vec![("quantity", quantity_value(quantity))],
        ExpressionKind::Param(name) => vec![("param", text(name))],
        ExpressionKind::Let(name) => vec![("let", text(name))],
        ExpressionKind::Reference(name) => vec![("reference", text(name))],
        ExpressionKind::Probe(Probe::Voltage { node, reference }) => {
            vec![("probe", text("V")), ("node", node_value(node)), ("reference", optional(reference, node_value))]
        }
        ExpressionKind::Probe(Probe::Current { component, pin }) => {
            vec![("probe", text("I")), ("component", text(component)), ("pin", text(pin))]
        }
        ExpressionKind::Probe(Probe::Noise { contributor, quantity }) => {
            let mut entries = vec![("probe", text("noise")), ("contributor", optional(contributor, |name| text(name)))];
            if let Some(quantity) = quantity {
                entries.push(("quantity", expression_value(quantity)));
            }
            entries
        }
        ExpressionKind::Probe(Probe::LoopGain) => vec![("probe", text("loop_gain"))],
        ExpressionKind::Probe(Probe::InputNoise { quantity }) => {
            let mut entries = vec![("probe", text("input_noise"))];
            if let Some(quantity) = quantity {
                entries.push(("quantity", expression_value(quantity)));
            }
            entries
        }
        ExpressionKind::Call { function, arguments } => vec![
            ("call", text(function.name())),
            ("arguments", Value::Array(arguments.iter().map(expression_value).collect())),
        ],
        ExpressionKind::Negate(operand) => vec![("negate", expression_value(operand))],
        ExpressionKind::Binary { operator, left, right } => vec![
            ("operator", text(operator.symbol())),
            ("left", expression_value(left)),
            ("right", expression_value(right)),
        ],
        ExpressionKind::Compare { operator, left, right } => vec![
            ("compare", text(operator.symbol())),
            ("left", expression_value(left)),
            ("right", expression_value(right)),
        ],
    };
    entries.push(("type", type_value(&expression.ty)));
    object(entries)
}

fn supply_value(supply: &SimSupply) -> Value {
    let mut entries = vec![
        ("name", text(&supply.name)),
        ("net", text(&supply.net)),
        ("voltage", quantity_value(&supply.voltage)),
        ("treatment", text(supply.treatment.name())),
    ];
    if supply.bench {
        entries.push(("bench", Value::Bool(true)));
    }
    match &supply.treatment {
        SupplyTreatment::Component(reference) => entries.push(("component", text(reference))),
        SupplyTreatment::Replaced(index) => entries.push(("replacement", Value::from(*index as i64))),
        _ => {}
    }
    object(entries)
}

/// The JSON of a checked bench (schema [`SCHEMA`]).
pub fn bench_value(bench: &Bench) -> Value {
    let params = bench
        .params
        .iter()
        .map(|(name, param)| {
            let mut entries = vec![("value", quantity_value(&param.value)), ("origin", text(param.origin.name()))];
            if let Some(derived) = &param.derived {
                entries.push(("derived", expression_value(derived)));
            }
            (name.clone(), object(entries))
        })
        .collect();
    let components = bench
        .components
        .iter()
        .map(|component| {
            let mut entries = vec![
                ("ref", text(&component.reference)),
                ("part", text(&component.part)),
                ("bench", Value::Bool(component.bench)),
                ("treatment", text(component.treatment.name())),
                ("fidelity", component.fidelity().map(|fidelity| text(fidelity.name())).unwrap_or(Value::Null)),
            ];
            match &component.treatment {
                Treatment::Passive { kind, value, extras } => {
                    entries.push(("passive", text(kind.name())));
                    entries.push(("value", sim_quantity_value(value)));
                    for (key, quantity) in [
                        ("tempco", &extras.tempco),
                        ("tempco2", &extras.tempco2),
                        ("esr", &extras.esr),
                        ("esl", &extras.esl),
                        ("self_resonance", &extras.self_resonance),
                    ] {
                        if let Some(quantity) = quantity {
                            entries.push((key, quantity_value(quantity)));
                        }
                    }
                }
                Treatment::Replaced(index) => entries.push(("replacement", Value::from(*index as i64))),
                Treatment::PartModel | Treatment::Ignored => {}
            }
            object(entries)
        })
        .collect();
    let nets = bench
        .nets
        .iter()
        .map(|net| {
            let join = match &net.join {
                NetJoin::BoardNet(name) => object(vec![("board_net", text(name))]),
                NetJoin::BoardPin(endpoint) => object(vec![("board_pin", text(&endpoint.to_string()))]),
                NetJoin::New => object(vec![("new", Value::Bool(true))]),
            };
            object(vec![
                ("name", text(&net.name)),
                ("join", join),
                ("node", node_value(&net.node)),
                ("endpoints", Value::Array(net.endpoints.iter().map(|endpoint| text(&endpoint.to_string())).collect())),
            ])
        })
        .collect();
    let replacements = bench
        .replacements
        .iter()
        .map(|replacement| {
            let mut entries = vec![("target", text(&replacement.target)), ("supply", Value::Bool(replacement.supply))];
            entries.extend(spec_fields(&replacement.spec));
            entries.push((
                "nodes",
                Value::Object(replacement.nodes.iter().map(|(role, node)| (role.clone(), node_value(node))).collect()),
            ));
            object(entries)
        })
        .collect();
    let models: Vec<Value> = bench
        .models
        .iter()
        .map(|model| {
            let mut entries = vec![("name", text(&model.name))];
            entries.extend(spec_fields(&model.spec));
            entries.push((
                "nodes",
                Value::Object(model.nodes.iter().map(|(role, node)| (role.clone(), node_value(node))).collect()),
            ));
            object(entries)
        })
        .collect();
    let analysis = match &bench.analysis {
        Analysis::Op => object(vec![("kind", text("op"))]),
        Analysis::Ac { from, to, points_per_decade } => object(vec![
            ("kind", text("ac")),
            ("from", sim_quantity_value(from)),
            ("to", sim_quantity_value(to)),
            ("points_per_decade", Value::from(i64::from(*points_per_decade))),
        ]),
        Analysis::Noise(noise) => crate::noise::analysis_value(noise, &node_value, &sim_quantity_value),
        Analysis::Tran(tran) => {
            let mut entries = vec![
                ("kind", text("tran")),
                ("stop", sim_quantity_value(&tran.stop)),
                ("start", optional(&tran.start, sim_quantity_value)),
                ("max_step", optional(&tran.max_step, sim_quantity_value)),
                ("reltol", optional(&tran.reltol, sim_quantity_value)),
            ];
            if tran.uic {
                entries.push(("uic", Value::Bool(true)));
            }
            if let Some(steady) = &tran.steady {
                entries.push(("steady", sim_quantity_value(steady)));
            }
            if tran.noise {
                entries.push(("noise", Value::Bool(true)));
                entries.push((
                    "seed",
                    Value::from(tran.seed.unwrap_or(crate::transient::DEFAULT_SEED).min(i64::MAX as u64) as i64),
                ));
            }
            if let Some(window) = tran.window {
                entries.push(("window", text(window.name())));
            }
            if let Some(accuracy) = tran.accuracy {
                entries.push(("accuracy", text(accuracy.name())));
            }
            object(entries)
        }
        Analysis::Loop(item) => crate::stability::loop_value(item, &node_value, &sim_quantity_value),
        Analysis::Pz(item) => crate::stability::pz_value(item, &node_value),
        Analysis::Dc(dc) => object(vec![
            ("kind", text("dc")),
            ("sweep", text(&dc.name)),
            ("dimension", text(dc.dimension.name())),
            ("from", sim_quantity_value(&dc.from)),
            ("to", sim_quantity_value(&dc.to)),
            ("step", optional(&dc.step, sim_quantity_value)),
            ("points", dc.points.map(|n| Value::from(i64::from(n))).unwrap_or(Value::Null)),
        ]),
    };
    let bench_components = bench.bench_components.iter().map(crate::serializer::component_value).collect();
    let mut target = vec![("board", text(&bench.board))];
    if let Some(module) = &bench.module {
        target.push(("module", text(module)));
    }
    let mut extra = Vec::new();
    if !models.is_empty() {
        extra.push(("models", Value::Array(models)));
    }
    if !bench.fixtures.is_empty() {
        extra.push(("fixtures", Value::Array(bench.fixtures.iter().map(|name| text(name)).collect())));
    }
    if let Some(section) = &bench.section {
        extra.push(("section", text(section)));
    }
    if !bench.initial_conditions.is_empty() {
        let items = bench
            .initial_conditions
            .iter()
            .map(|item| {
                let target = match &item.target {
                    InitialTarget::Node(node) => ("node", node_value(node)),
                    InitialTarget::Capacitor(reference) => ("capacitor", text(reference)),
                    InitialTarget::Inductor(reference) => ("inductor", text(reference)),
                };
                object(vec![target, ("value", sim_quantity_value(&item.value))])
            })
            .collect();
        extra.push(("initial_conditions", Value::Array(items)));
    }
    let variation = bench.variation.as_ref().map(crate::tolerance::variation_value);
    let mut entries = vec![("schema", text(SCHEMA)), ("name", text(&bench.name))];
    entries.extend(target);
    entries.extend(vec![
        ("params", Value::Object(params)),
        ("bench_components", Value::Array(bench_components)),
        (
            "bench_param_uses",
            Value::Object(
                bench
                    .bench_param_uses
                    .iter()
                    .map(|(reference, uses)| {
                        (
                            reference.clone(),
                            Value::Object(
                                uses.iter().map(|(attribute, param)| (attribute.clone(), text(param))).collect(),
                            ),
                        )
                    })
                    .collect(),
            ),
        ),
        ("library", Value::Array(bench.library.values().map(crate::serializer::part_value).collect())),
        ("components", Value::Array(components)),
        ("supplies", Value::Array(bench.supplies.iter().map(supply_value).collect())),
        ("ground", text(&bench.ground)),
        ("nets", Value::Array(nets)),
        ("replacements", Value::Array(replacements)),
    ]);
    entries.extend(extra);
    entries.extend(vec![
        ("analysis", analysis),
        (
            "steps",
            Value::Array(
                bench
                    .steps
                    .iter()
                    .map(|step| {
                        let mut entries = vec![
                            ("param", text(&step.param)),
                            ("values", Value::Array(step.values.iter().map(quantity_value).collect())),
                        ];
                        if !step.zip.is_empty() {
                            let zip = step
                                .zip
                                .iter()
                                .map(|(param, values)| {
                                    object(vec![
                                        ("param", text(param)),
                                        ("values", Value::Array(values.iter().map(quantity_value).collect())),
                                    ])
                                })
                                .collect();
                            entries.push(("zip", Value::Array(zip)));
                        }
                        object(entries)
                    })
                    .collect(),
            ),
        ),
        (
            "references",
            Value::Array(
                bench
                    .references
                    .iter()
                    .map(|reference| {
                        object(vec![
                            ("name", text(&reference.name)),
                            (
                                "overrides",
                                Value::Object(
                                    reference
                                        .overrides
                                        .iter()
                                        .map(|(name, value)| (name.clone(), sim_quantity_value(value)))
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "lets",
            Value::Array(
                bench
                    .lets
                    .iter()
                    .map(|item| {
                        object(vec![("name", text(&item.name)), ("expression", expression_value(&item.expression))])
                    })
                    .collect(),
            ),
        ),
        (
            "measures",
            Value::Array(
                bench
                    .measures
                    .iter()
                    .map(|item| {
                        let mut entries = vec![
                            ("name", text(&item.name)),
                            ("expression", expression_value(&item.expression)),
                            ("min", optional(&item.min, sim_quantity_value)),
                            ("max", optional(&item.max, sim_quantity_value)),
                        ];
                        if !item.conditions.is_empty() {
                            entries
                                .push(("where", Value::Array(item.conditions.iter().map(expression_value).collect())));
                        }
                        object(entries)
                    })
                    .collect(),
            ),
        ),
        (
            "plots",
            Value::Array(
                bench
                    .plots
                    .iter()
                    .map(|plot| Value::Array(plot.expressions.iter().map(expression_value).collect()))
                    .collect(),
            ),
        ),
    ]);
    if let Some(variation) = variation {
        entries.push(("variation", variation));
    }
    object(entries)
}

/// [`bench_value`] as sorted, indented JSON with a trailing newline.
pub fn bench_json(bench: &Bench) -> String {
    format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&bench_value(bench)))
}
