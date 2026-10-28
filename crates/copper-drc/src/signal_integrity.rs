//! Signal-integrity screening of routing intent (`pcbir.signal_integrity`,
//! with the closed-form estimates of `pcbir.engineering`).
//!
//! Every result is screening evidence: closed-form impedance estimates,
//! adjacency of declared reference planes and routed-length bookkeeping.
//! None of it replaces a field solver, the fabricator's stack-up
//! qualification or channel sign-off. The estimates use the same float
//! operations in the same order as Python, and round like `round(x, 9)`.

use std::f64::consts::PI;

use copper_json::{Dumps, Object, Value};
use copper_lang::mechanical::stackup::StackupLayer;
use copper_pcb::Result;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, Stackup};
use copper_physical::CopperLayer;
use copper_physical::pyfloat::round_int;
use copper_units::Decimal;
use indexmap::{IndexMap, IndexSet};

const SI_IMPEDANCE: &str = "SI-IMPEDANCE";
const SI_NO_STACKUP: &str = "SI-NO-STACKUP";
const SI_NO_REFERENCE_PLANE: &str = "SI-NO-REFERENCE-PLANE";
const SI_LAYER_GROUP: &str = "SI-LAYER-GROUP";

const FIELD_SOLVER: &str = "requires field-solver qualification for fabrication release";
const MICROSTRIP_IGNORES: &str = "ignores solder mask, roughness, weave and local copper";

fn differential(kind: RouteKind) -> bool {
    matches!(kind, RouteKind::Differential | RouteKind::CanBus)
}

fn needs_reference_plane(kind: RouteKind) -> bool {
    matches!(kind, RouteKind::Differential | RouteKind::Clock | RouteKind::RfFeed)
}

fn decimal(value: i64) -> Decimal {
    Decimal::from_i64(value)
}

fn decimal_ok(result: std::result::Result<Decimal, copper_units::DecimalError>) -> Decimal {
    result.expect("screening arithmetic stays within the decimal context")
}

/// `format(value.normalize(), "f")`.
fn text(value: &Decimal) -> String {
    decimal_ok(value.normalize()).to_fixed_string()
}

/// `_mm`: nanometres as normalized millimetres.
fn mm(value_nm: i64) -> String {
    text(&decimal_ok(decimal(value_nm).div(&decimal(1_000_000))))
}

/// `_ohm`: one decimal place.
fn ohm(value: &Decimal) -> String {
    format!("{} ohm", value.quantize(-1).to_sci_string())
}

/// Python's `round(value, ndigits)`.
fn round_digits(value: f64, ndigits: u32) -> f64 {
    copper_physical::pyfloat::round_digits(value, ndigits)
}

/// `_ohms`: `Decimal(str(round(value, 9)))`.
fn ohms(value: f64) -> Decimal {
    let text = copper_json::python_float_repr(round_digits(value, 9));
    Decimal::parse(&text).expect("a finite float repr")
}

fn to_float(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("a decimal string")
}

/// A closed-form estimate: the value and its validity notes.
struct Estimate {
    value: Decimal,
    validity: Vec<&'static str>,
}

/// `_hammerstad_microstrip`: (single-ended ohms, effective permittivity).
fn hammerstad_microstrip(width_nm: i64, thickness_nm: i64, height_nm: i64, er: &Decimal) -> Result<(f64, f64)> {
    if width_nm.min(thickness_nm).min(height_nm) <= 0 || *er <= decimal(1) {
        return Err("microstrip geometry and permittivity are invalid".to_owned().into());
    }
    let width = width_nm as f64 / height_nm as f64;
    let thickness = thickness_nm as f64 / height_nm as f64;
    let effective_width = width + thickness / PI * (1.0 + (4.0 * PI / thickness.max(1e-12)).ln());
    let er = to_float(er);
    let effective_er = (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / effective_width).sqrt());
    let impedance = if effective_width <= 1.0 {
        60.0 / effective_er.sqrt() * (8.0 / effective_width + effective_width / 4.0).ln()
    } else {
        120.0 * PI / (effective_er.sqrt() * (effective_width + 1.393 + 0.667 * (effective_width + 1.444).ln()))
    };
    Ok((impedance, effective_er))
}

fn microstrip_impedance(width_nm: i64, thickness_nm: i64, height_nm: i64, er: &Decimal) -> Result<Estimate> {
    let (impedance, _) = hammerstad_microstrip(width_nm, thickness_nm, height_nm, er)?;
    Ok(Estimate { value: ohms(impedance), validity: vec![FIELD_SOLVER, MICROSTRIP_IGNORES] })
}

fn edge_coupled_microstrip(
    width_nm: i64,
    gap_nm: i64,
    thickness_nm: i64,
    height_nm: i64,
    er: &Decimal,
) -> Result<Estimate> {
    if gap_nm <= 0 {
        return Err("differential gap must be positive".to_owned().into());
    }
    let (single, _) = hammerstad_microstrip(width_nm, thickness_nm, height_nm, er)?;
    let differential = 2.0 * single * (1.0 - 0.48 * (-0.96 * gap_nm as f64 / height_nm as f64).exp());
    let mut validity = vec![FIELD_SOLVER, MICROSTRIP_IGNORES];
    let (w_h, s_h) = (width_nm as f64 / height_nm as f64, gap_nm as f64 / height_nm as f64);
    if !((0.1..=2.0).contains(&w_h) && (0.2..=3.0).contains(&s_h)) {
        validity.push("outside the IPC-2141A fitted range 0.1 <= w/h <= 2, 0.2 <= s/h <= 3");
    }
    Ok(Estimate { value: ohms(differential), validity })
}

/// `_symmetric_stripline`: IPC-2141A centred stripline of plane spacing `spacing_nm`.
fn symmetric_stripline(width_nm: i64, thickness_nm: i64, spacing_nm: i64, er: &Decimal) -> Result<f64> {
    let argument = (4 * spacing_nm) as f64 / (0.67 * PI * (0.8 * width_nm as f64 + thickness_nm as f64));
    if argument <= 1.0 {
        return Err("stripline trace is too wide for the IPC-2141A estimate".to_owned().into());
    }
    Ok(60.0 / to_float(er).sqrt() * argument.ln())
}

fn stripline_impedance(
    width_nm: i64,
    thickness_nm: i64,
    above_nm: i64,
    below_nm: i64,
    er: &Decimal,
) -> Result<Estimate> {
    if width_nm.min(thickness_nm).min(above_nm).min(below_nm) <= 0 || *er < decimal(1) {
        return Err("stripline geometry and permittivity are invalid".to_owned().into());
    }
    let spacing = above_nm + below_nm + thickness_nm;
    let impedance = if above_nm == below_nm {
        symmetric_stripline(width_nm, thickness_nm, spacing, er)?
    } else {
        let above = symmetric_stripline(width_nm, thickness_nm, 2 * above_nm + thickness_nm, er)?;
        let below = symmetric_stripline(width_nm, thickness_nm, 2 * below_nm + thickness_nm, er)?;
        2.0 * above * below / (above + below)
    };
    let mut validity =
        vec![FIELD_SOLVER, "assumes solid reference planes above and below and a homogeneous dielectric"];
    if width_nm as f64 / spacing as f64 >= 0.35 || thickness_nm as f64 / spacing as f64 >= 0.25 {
        validity.push("outside the IPC-2141A fitted range w/b < 0.35, t/b < 0.25");
    }
    Ok(Estimate { value: ohms(impedance), validity })
}

fn edge_coupled_stripline(
    width_nm: i64,
    gap_nm: i64,
    thickness_nm: i64,
    above_nm: i64,
    below_nm: i64,
    er: &Decimal,
) -> Result<Estimate> {
    if gap_nm <= 0 {
        return Err("differential gap must be positive".to_owned().into());
    }
    let single = stripline_impedance(width_nm, thickness_nm, above_nm, below_nm, er)?;
    let spacing = above_nm + below_nm + thickness_nm;
    let differential = 2.0 * to_float(&single.value) * (1.0 - 0.347 * (-2.9 * gap_nm as f64 / spacing as f64).exp());
    Ok(Estimate { value: ohms(differential), validity: single.validity })
}

/// `LayerGeometry`: the transmission-line structure of one copper layer.
struct LayerGeometry {
    microstrip: bool,
    copper_thickness_nm: i64,
    height_above_nm: Option<i64>,
    height_below_nm: Option<i64>,
    relative_permittivity: Decimal,
}

impl LayerGeometry {
    fn dielectric_height_nm(&self) -> i64 {
        if self.microstrip {
            return self.height_above_nm.or(self.height_below_nm).filter(|h| *h != 0).unwrap_or(0);
        }
        self.height_above_nm.unwrap_or(0) + self.height_below_nm.unwrap_or(0) + self.copper_thickness_nm
    }

    fn structure(&self) -> &'static str {
        if self.microstrip {
            "microstrip"
        } else if self.height_above_nm == self.height_below_nm {
            "symmetric_stripline"
        } else {
            "offset_stripline"
        }
    }
}

fn dielectric(layer: &StackupLayer) -> Option<&Decimal> {
    if layer.copper_layer.is_some() { None } else { layer.relative_permittivity.as_ref() }
}

/// `layer_geometry`: None without adjacent declared dielectrics.
fn layer_geometry(stackup: &Stackup, layer: CopperLayer) -> Option<LayerGeometry> {
    let layers = &stackup.physical_layers;
    let index = layers.iter().position(|item| item.copper_layer == Some(layer))?;
    let above = index.checked_sub(1).map(|i| &layers[i]);
    let below = layers.get(index + 1);
    let neighbours: Vec<&StackupLayer> = [above, below].into_iter().flatten().collect();
    if neighbours.is_empty() || neighbours.iter().any(|item| dielectric(item).is_none()) {
        return None;
    }
    let thickness = layers[index].thickness_nm;
    if neighbours.len() == 1 {
        return Some(LayerGeometry {
            microstrip: true,
            copper_thickness_nm: thickness,
            height_above_nm: above.map(|item| item.thickness_nm),
            height_below_nm: below.map(|item| item.thickness_nm),
            relative_permittivity: dielectric(neighbours[0]).expect("checked").clone(),
        });
    }
    let (above, below) = (above.expect("two neighbours"), below.expect("two neighbours"));
    let total = above.thickness_nm + below.thickness_nm;
    let weighted = decimal_ok(
        decimal_ok(dielectric(above).expect("checked").mul(&decimal(above.thickness_nm)))
            .add(&decimal_ok(dielectric(below).expect("checked").mul(&decimal(below.thickness_nm)))),
    );
    let er = decimal_ok(decimal_ok(weighted.div(&decimal(total))).quantize(-6).normalize());
    Some(LayerGeometry {
        microstrip: false,
        copper_thickness_nm: thickness,
        height_above_nm: Some(above.thickness_nm),
        height_below_nm: Some(below.thickness_nm),
        relative_permittivity: er,
    })
}

/// `ImpedanceEstimate`.
struct ImpedanceEstimate {
    nets: Vec<String>,
    layer: &'static str,
    structure: &'static str,
    width_nm: i64,
    gap_nm: Option<i64>,
    single_ended_ohms: Option<Decimal>,
    differential_ohms: Option<Decimal>,
    target_single_ended_ohms: Option<i64>,
    target_differential_ohms: Option<i64>,
    tolerance_percent: Decimal,
    status: &'static str,
    validity: Vec<String>,
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn optional_text(value: &Option<Decimal>) -> Value {
    value.as_ref().map_or(Value::Null, |value| Value::from(text(value).as_str()))
}

impl ImpedanceEstimate {
    fn to_value(&self) -> Value {
        let mut item = Object::new();
        item.insert("nets".into(), strings(&self.nets));
        item.insert("layer".into(), Value::from(self.layer));
        item.insert("structure".into(), Value::from(self.structure));
        item.insert("width_nm".into(), Value::from(self.width_nm));
        item.insert("gap_nm".into(), optional_int(self.gap_nm));
        item.insert("single_ended_ohms".into(), optional_text(&self.single_ended_ohms));
        item.insert("differential_ohms".into(), optional_text(&self.differential_ohms));
        item.insert("target_single_ended_ohms".into(), optional_int(self.target_single_ended_ohms));
        item.insert("target_differential_ohms".into(), optional_int(self.target_differential_ohms));
        item.insert("tolerance_percent".into(), Value::from(text(&self.tolerance_percent).as_str()));
        item.insert("status".into(), Value::from(self.status));
        item.insert("validity".into(), strings(&self.validity));
        item.insert("evidence_grade".into(), Value::from("screening"));
        Value::Object(item)
    }

    fn line(&self) -> String {
        let tolerance = text(&self.tolerance_percent);
        let mut parts = Vec::new();
        if let Some(value) = &self.differential_ohms {
            let target = self.target_differential_ohms.map(|t| format!(" (target {t} ±{tolerance}%)"));
            parts.push(format!("diff {}{}", ohm(value), target.unwrap_or_default()));
        }
        if let Some(value) = &self.single_ended_ohms {
            let target = self.target_single_ended_ohms.map(|t| format!(" (target {t} ±{tolerance}%)"));
            parts.push(format!("SE {}{}", ohm(value), target.unwrap_or_default()));
        }
        let mut geometry = format!("w={}mm", mm(self.width_nm));
        if let Some(gap) = self.gap_nm.filter(|gap| *gap != 0) {
            geometry.push_str(&format!(" g={}mm", mm(gap)));
        }
        let screened = if parts.is_empty() { "not screened".to_owned() } else { parts.join("; ") };
        format!(
            "IMPEDANCE {} {} {} {geometry}: {screened}: {} (screening)",
            self.nets.join(","),
            self.layer,
            self.structure,
            self.status
        )
    }
}

/// `SiWarning`.
struct Warning {
    code: &'static str,
    message: String,
    nets: Vec<String>,
    layers: Vec<String>,
}

impl Warning {
    fn new(code: &'static str, message: String, nets: &[String], layers: Vec<String>) -> Warning {
        Warning { code, message, nets: nets.to_vec(), layers }
    }

    fn to_value(&self) -> Value {
        let mut item = Object::new();
        item.insert("code".into(), Value::from(self.code));
        item.insert("severity".into(), Value::from("warning"));
        item.insert("message".into(), Value::from(self.message.as_str()));
        item.insert("nets".into(), strings(&self.nets));
        item.insert("layers".into(), strings(&self.layers));
        Value::Object(item)
    }
}

/// `_within`: whether `value` is within `tolerance` percent of `target`.
fn within(value: Option<&Decimal>, target: Option<i64>, tolerance: &Decimal) -> Option<bool> {
    let (value, target) = (value?, decimal(target?));
    let difference = decimal_ok(value.sub(&target)).abs();
    let limit = decimal_ok(decimal_ok(target.mul(tolerance)).div(&decimal(100)));
    Some(difference <= limit)
}

/// `_rule_sets`: differential partners whose screening inputs are identical
/// screen once.
fn rule_sets<'a, K: PartialEq>(
    rules: &[&'a NetRoutingRule],
    key: impl Fn(&NetRoutingRule) -> K,
) -> Vec<(Vec<String>, &'a NetRoutingRule)> {
    let mut by_net: IndexMap<&str, &NetRoutingRule> = IndexMap::new();
    for rule in rules {
        by_net.insert(rule.net.as_str(), rule);
    }
    let mut names: Vec<&str> = by_net.keys().copied().collect();
    names.sort();
    let mut done: IndexSet<String> = IndexSet::new();
    let mut result = Vec::new();
    for net in names {
        if done.contains(net) {
            continue;
        }
        let rule = by_net[net];
        let partner = rule.differential_partner.as_deref().and_then(|name| by_net.get(name).copied());
        let nets = match partner {
            Some(partner)
                if !done.contains(&partner.net)
                    && partner.differential_partner.as_deref() == Some(rule.net.as_str())
                    && key(partner) == key(rule) =>
            {
                vec![rule.net.clone(), partner.net.clone()]
            }
            _ => vec![rule.net.clone()],
        };
        done.extend(nets.iter().cloned());
        result.push((nets, rule));
    }
    result
}

fn tolerance(rule: &NetRoutingRule) -> Decimal {
    rule.impedance_tolerance_percent.clone().unwrap_or_else(|| decimal(10))
}

fn allowed_layers<'a>(rule: &'a NetRoutingRule, board: &'a PhysicalBoard) -> &'a [CopperLayer] {
    if rule.allowed_layers.is_empty() { &board.stackup.copper_layers } else { &rule.allowed_layers }
}

#[allow(clippy::too_many_arguments)]
fn estimate(
    stackup: &Stackup,
    layer: CopperLayer,
    width: i64,
    gap: Option<i64>,
    nets: &[String],
    target_se: Option<i64>,
    target_diff: Option<i64>,
    tolerance: &Decimal,
) -> ImpedanceEstimate {
    let base = |structure: &'static str, validity: Vec<String>| ImpedanceEstimate {
        nets: nets.to_vec(),
        layer: layer.value(),
        structure,
        width_nm: width,
        gap_nm: gap,
        single_ended_ohms: None,
        differential_ohms: None,
        target_single_ended_ohms: target_se,
        target_differential_ohms: target_diff,
        tolerance_percent: tolerance.clone(),
        status: "indeterminate",
        validity,
    };
    let Some(geometry) = layer_geometry(stackup, layer) else {
        return base("unknown", vec!["layer has no adjacent declared dielectric".to_owned()]);
    };
    let er = &geometry.relative_permittivity;
    let thickness = geometry.copper_thickness_nm;
    let computed = (|| -> Result<(Estimate, Option<Estimate>)> {
        if geometry.microstrip {
            let height = geometry.dielectric_height_nm();
            let single = microstrip_impedance(width, thickness, height, er)?;
            let pair = gap.map(|gap| edge_coupled_microstrip(width, gap, thickness, height, er)).transpose()?;
            Ok((single, pair))
        } else {
            let (above, below) = (geometry.height_above_nm.unwrap_or(0), geometry.height_below_nm.unwrap_or(0));
            let single = stripline_impedance(width, thickness, above, below, er)?;
            let pair = gap.map(|gap| edge_coupled_stripline(width, gap, thickness, above, below, er)).transpose()?;
            Ok((single, pair))
        }
    })();
    let (single, pair) = match computed {
        Ok(result) => result,
        Err(failure) => {
            let structure = if geometry.microstrip { "microstrip" } else { "stripline" };
            return base(structure, vec![format!("cannot be screened: {failure}")]);
        }
    };
    let checks = [
        within(Some(&single.value), target_se, tolerance),
        within(pair.as_ref().map(|pair| &pair.value), target_diff, tolerance),
    ];
    let known: Vec<bool> = checks.into_iter().flatten().collect();
    let status = if known.is_empty() {
        "indeterminate"
    } else if known.iter().all(|passed| *passed) {
        "pass"
    } else {
        "fail"
    };
    let mut validity: Vec<String> = Vec::new();
    for note in single.validity.iter().chain(pair.iter().flat_map(|pair| pair.validity.iter())) {
        if !validity.iter().any(|known| known == note) {
            validity.push((*note).to_owned());
        }
    }
    ImpedanceEstimate {
        structure: geometry.structure(),
        single_ended_ohms: Some(single.value),
        differential_ohms: pair.map(|pair| pair.value),
        status,
        validity,
        ..base("", Vec::new())
    }
}

fn impedance_warnings(estimate: &ImpedanceEstimate, label: &str) -> Vec<Warning> {
    let layers = vec![estimate.layer.to_owned()];
    if estimate.status == "indeterminate" {
        if estimate.single_ended_ohms.is_none() {
            let message = format!("{label} on {}: impedance {}", estimate.layer, estimate.validity.join("; "));
            return vec![Warning::new(SI_IMPEDANCE, message, &estimate.nets, layers)];
        }
        return Vec::new();
    }
    let mut geometry = format!("{}, width {} mm", estimate.structure, mm(estimate.width_nm));
    if let Some(gap) = estimate.gap_nm.filter(|gap| *gap != 0) {
        geometry.push_str(&format!(", gap {} mm", mm(gap)));
    }
    let mut result = Vec::new();
    for (kind, value, target) in [
        ("differential", &estimate.differential_ohms, estimate.target_differential_ohms),
        ("single-ended", &estimate.single_ended_ohms, estimate.target_single_ended_ohms),
    ] {
        if within(value.as_ref(), target, &estimate.tolerance_percent) == Some(false) {
            let message = format!(
                "{label} on {}: {kind} estimate {} is outside {} ohm ±{}% ({geometry}); screening estimate, not sign-off",
                estimate.layer,
                ohm(value.as_ref().expect("compared")),
                target.expect("compared"),
                text(&estimate.tolerance_percent)
            );
            result.push(Warning::new(SI_IMPEDANCE, message, &estimate.nets, layers.clone()));
        }
    }
    result
}

/// `screen_impedance`: every rule with an impedance target on every allowed layer.
fn screen_impedance(board: &PhysicalBoard) -> (Vec<ImpedanceEstimate>, Vec<Warning>) {
    let targeted: Vec<&NetRoutingRule> = board
        .net_routing_rules
        .iter()
        .filter(|rule| rule.target_impedance_ohms.is_some() || rule.target_single_ended_ohms.is_some())
        .collect();
    let key = |rule: &NetRoutingRule| {
        (
            rule.kind,
            rule.width_nm,
            rule.pair_gap_nm,
            rule.allowed_layers.clone(),
            rule.target_impedance_ohms,
            rule.target_single_ended_ohms,
            tolerance(rule),
        )
    };
    let (mut estimates, mut warnings) = (Vec::new(), Vec::new());
    for (nets, rule) in rule_sets(&targeted, key) {
        let is_differential = differential(rule.kind);
        let target_diff = if is_differential { rule.target_impedance_ohms } else { None };
        // Python's `or` treats a zero single-ended target as absent.
        let target_se = if is_differential {
            rule.target_single_ended_ohms
        } else {
            rule.target_single_ended_ohms.filter(|t| *t != 0).or(rule.target_impedance_ohms)
        };
        let tolerance = tolerance(rule);
        let label = nets.join("/");
        if board.stackup.physical_layers.is_empty() {
            let declared: Vec<String> = [
                target_diff.map(|t| format!("{t} ohm differential")),
                target_se.map(|t| format!("{t} ohm single-ended")),
            ]
            .into_iter()
            .flatten()
            .collect();
            let message = format!(
                "{label} declares an impedance target ({}) but the board has no declared stack-up; \
                 impedance is not screened (declare mechanical stackup)",
                declared.join(", ")
            );
            warnings.push(Warning::new(SI_NO_STACKUP, message, &nets, Vec::new()));
            continue;
        }
        let width = rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
        let gap = if is_differential { rule.pair_gap_nm } else { None };
        for layer in allowed_layers(rule, board) {
            let item = estimate(&board.stackup, *layer, width, gap, &nets, target_se, target_diff, &tolerance);
            warnings.extend(impedance_warnings(&item, &label));
            estimates.push(item);
        }
    }
    (estimates, warnings)
}

/// `reference_plane_warnings`: a differential, clock or RF rule may use a
/// layer without a declared copper zone on an adjacent copper layer.
fn reference_plane_warnings(board: &PhysicalBoard) -> Vec<Warning> {
    let copper = &board.stackup.copper_layers;
    let zoned: IndexSet<CopperLayer> = board.zones.iter().flat_map(|zone| zone.layers.iter().copied()).collect();
    let rules: Vec<&NetRoutingRule> =
        board.net_routing_rules.iter().filter(|rule| needs_reference_plane(rule.kind)).collect();
    let mut warnings = Vec::new();
    for (nets, rule) in rule_sets(&rules, |item| (item.kind, item.allowed_layers.clone())) {
        let mut missing: Vec<(CopperLayer, Vec<CopperLayer>)> = Vec::new();
        for layer in allowed_layers(rule, board) {
            let Some(index) = copper.iter().position(|item| item == layer) else { continue };
            let adjacent: Vec<CopperLayer> = [index.checked_sub(1), Some(index + 1)]
                .into_iter()
                .flatten()
                .filter_map(|i| copper.get(i).copied())
                .collect();
            if !adjacent.iter().any(|item| zoned.contains(item)) {
                missing.push((*layer, adjacent));
            }
        }
        if missing.is_empty() {
            continue;
        }
        let detail: Vec<String> = missing
            .iter()
            .map(|(layer, adjacent)| {
                let names: Vec<&str> = adjacent.iter().map(|item| item.value()).collect();
                format!("{} (adjacent: {})", layer.value(), names.join(", "))
            })
            .collect();
        let message = format!(
            "{} rule for {} allows a layer with no copper_zone on an adjacent copper layer: {}; \
             declare a reference plane or remove the layer from allowed_layers",
            rule.kind.value(),
            nets.join("/"),
            detail.join("; ")
        );
        let layers = missing.iter().map(|(layer, _)| layer.value().to_owned()).collect();
        warnings.push(Warning::new(SI_NO_REFERENCE_PLANE, message, &nets, layers));
    }
    warnings
}

/// `routed_track_length_nm`.
fn routed_track_length_nm(board: &PhysicalBoard, net: &str) -> i64 {
    board
        .tracks
        .iter()
        .filter(|track| track.net == net)
        .map(|t| round_int(((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64)))
        .sum()
}

/// `MatchGroupCheck`: one group's member `(net, length, connected)`, skew and status.
pub struct MatchGroup {
    pub id: String,
    pub max_skew_nm: i64,
    pub members: Vec<(String, i64, bool)>,
    pub skew_nm: Option<i64>,
    pub status: &'static str,
}

/// `verify_match_groups`: member lengths and skew once every member is connected.
pub fn verify_match_groups(board: &PhysicalBoard) -> Result<Vec<MatchGroup>> {
    if board.match_groups.is_empty() {
        return Ok(Vec::new());
    }
    let members: IndexSet<String> = board.match_groups.iter().flat_map(|group| group.nets.iter().cloned()).collect();
    let candidate = crate::connectivity::Candidate { only_nets: Some(&members), ..Default::default() };
    let graph = crate::connectivity::explicit_copper_connectivity(board, candidate)?;
    let mut results = Vec::new();
    for group in &board.match_groups {
        let mut lengths = Vec::new();
        for net in &group.nets {
            let Some(physical) = board.nets.iter().find(|item| item.name == *net) else {
                return Err(format!("match group {} names unknown net {net}", group.id).into());
            };
            lengths.push((net.clone(), routed_track_length_nm(board, net), graph.net_connected(physical)));
        }
        let (skew, status) = if lengths.iter().all(|(_, _, routed)| *routed) {
            let longest = lengths.iter().map(|(_, length, _)| *length).max().unwrap_or(0);
            let shortest = lengths.iter().map(|(_, length, _)| *length).min().unwrap_or(0);
            let skew = longest - shortest;
            (Some(skew), if skew <= group.max_skew_nm { "pass" } else { "fail" })
        } else {
            (None, "incomplete")
        };
        results.push(MatchGroup {
            id: group.id.clone(),
            max_skew_nm: group.max_skew_nm,
            members: lengths,
            skew_nm: skew,
            status,
        });
    }
    Ok(results)
}

impl MatchGroup {
    pub fn to_value(&self) -> Value {
        let longest = self.members.iter().map(|(_, length, _)| *length).max().unwrap_or(0);
        let members = self
            .members
            .iter()
            .map(|(net, length, routed)| {
                let mut item = Object::new();
                item.insert("net".into(), Value::from(net.as_str()));
                item.insert("length_nm".into(), Value::from(*length));
                item.insert("routed".into(), Value::Bool(*routed));
                item.insert("shortfall_nm".into(), Value::from(longest - length));
                Value::Object(item)
            })
            .collect();
        let mut item = Object::new();
        item.insert("id".into(), Value::from(self.id.as_str()));
        item.insert("max_skew_nm".into(), Value::from(self.max_skew_nm));
        item.insert("skew_nm".into(), optional_int(self.skew_nm));
        item.insert("status".into(), Value::from(self.status));
        item.insert("members".into(), Value::Array(members));
        Value::Object(item)
    }

    fn line(&self) -> String {
        let skew =
            self.skew_nm.map_or("n/a until every member is routed".to_owned(), |skew| format!("{} mm", mm(skew)));
        format!(
            "MATCH GROUP {}: {} nets, max skew {} mm, skew {skew}: {}",
            self.id,
            self.members.len(),
            mm(self.max_skew_nm),
            self.status
        )
    }
}

/// `layer_groups`: rules by layer group, groups and members sorted.
fn layer_groups(board: &PhysicalBoard) -> Vec<(String, Vec<&NetRoutingRule>)> {
    let mut rules: Vec<&NetRoutingRule> = board.net_routing_rules.iter().collect();
    rules.sort_by(|a, b| a.net.cmp(&b.net));
    let mut groups: IndexMap<String, Vec<&NetRoutingRule>> = IndexMap::new();
    for rule in rules {
        if let Some(group) = &rule.layer_group {
            groups.entry(group.clone()).or_default().push(rule);
        }
    }
    let mut result: Vec<(String, Vec<&NetRoutingRule>)> = groups.into_iter().collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// `_ordered`: unique layers in stack-up order.
fn ordered(board: &PhysicalBoard, layers: &[CopperLayer]) -> Vec<CopperLayer> {
    let copper = &board.stackup.copper_layers;
    let order = |layer: &CopperLayer| copper.iter().position(|item| item == layer).unwrap_or(copper.len());
    let mut unique: Vec<CopperLayer> = Vec::new();
    for layer in layers {
        if !unique.contains(layer) {
            unique.push(*layer);
        }
    }
    unique.sort_by_key(order);
    unique
}

/// `layer_group_warnings`: members of one layer group must allow the same layers.
fn layer_group_warnings(board: &PhysicalBoard) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for (name, rules) in layer_groups(board) {
        let nets: Vec<String> = rules.iter().map(|rule| rule.net.clone()).collect();
        let shown = copper_json::python_str_repr(&name);
        if rules.len() == 1 {
            let message = format!("layer group {shown} has a single member ({})", nets[0]);
            warnings.push(Warning::new(SI_LAYER_GROUP, message, &nets, Vec::new()));
            continue;
        }
        // A dict keyed by net: a repeated net keeps its first position and last value.
        let mut allowed: IndexMap<&str, Vec<CopperLayer>> = IndexMap::new();
        for rule in &rules {
            allowed.insert(rule.net.as_str(), ordered(board, allowed_layers(rule, board)));
        }
        let mut distinct: Vec<&Vec<CopperLayer>> = Vec::new();
        for layers in allowed.values() {
            if !distinct.contains(&layers) {
                distinct.push(layers);
            }
        }
        if distinct.len() > 1 {
            let detail: Vec<String> = allowed
                .iter()
                .map(|(net, layers)| {
                    let names: Vec<&str> = layers.iter().map(|layer| layer.value()).collect();
                    format!("{net}: {}", names.join(","))
                })
                .collect();
            let mut layers: Vec<String> = allowed
                .values()
                .flatten()
                .map(|layer| layer.value().to_owned())
                .collect::<IndexSet<_>>()
                .into_iter()
                .collect();
            layers.sort();
            let message = format!("layer group {shown} members allow different layer sets: {}", detail.join("; "));
            warnings.push(Warning::new(SI_LAYER_GROUP, message, &nets, layers));
        }
    }
    warnings
}

/// `si_check(board).to_dict()` and `.lines()`.
pub struct SiCheckReport {
    pub document: Value,
    pub lines: Vec<String>,
}

/// `si_check`: pre-route screening of impedance, reference planes, layer
/// groups and a summary of length-match groups.
pub fn si_check(board: &PhysicalBoard) -> Result<SiCheckReport> {
    let (estimates, mut warnings) = screen_impedance(board);
    warnings.extend(reference_plane_warnings(board));
    warnings.extend(layer_group_warnings(board));
    let match_groups = verify_match_groups(board)?;
    let groups: Vec<(String, Vec<String>)> = layer_groups(board)
        .into_iter()
        .map(|(name, rules)| (name, rules.iter().map(|rule| rule.net.clone()).collect()))
        .collect();
    let copper: Vec<&str> = board.stackup.copper_layers.iter().map(|layer| layer.value()).collect();
    let declared = !board.stackup.physical_layers.is_empty();
    let mut stackup = Object::new();
    stackup.insert("declared".into(), Value::Bool(declared));
    stackup.insert("copper_layers".into(), Value::Array(copper.iter().map(|layer| Value::from(*layer)).collect()));
    stackup.insert("thickness_nm".into(), Value::from(board.stackup.thickness_nm));
    let mut document = Object::new();
    document.insert("schema".into(), Value::from("copperscript-si-check/v0.1"));
    document.insert("board".into(), Value::from(board.name.as_str()));
    document.insert("evidence_grade".into(), Value::from("screening"));
    document.insert("stackup".into(), Value::Object(stackup));
    document.insert("impedance".into(), Value::Array(estimates.iter().map(ImpedanceEstimate::to_value).collect()));
    document.insert("match_groups".into(), Value::Array(match_groups.iter().map(MatchGroup::to_value).collect()));
    document.insert(
        "layer_groups".into(),
        Value::Array(
            groups
                .iter()
                .map(|(name, nets)| {
                    let mut item = Object::new();
                    item.insert("name".into(), Value::from(name.as_str()));
                    item.insert("nets".into(), strings(nets));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    document.insert("warnings".into(), Value::Array(warnings.iter().map(Warning::to_value).collect()));
    let stack = if declared {
        format!("declared stack-up {} mm", mm(board.stackup.thickness_nm))
    } else {
        "no declared stack-up".to_owned()
    };
    let mut lines = vec![format!(
        "SI screening for {}: {} copper layers, {stack} (screening evidence, not sign-off)",
        board.name,
        copper.len()
    )];
    lines.extend(estimates.iter().map(ImpedanceEstimate::line));
    lines.extend(match_groups.iter().map(MatchGroup::line));
    lines.extend(groups.iter().map(|(name, nets)| format!("LAYER GROUP {name}: {}", nets.join(", "))));
    lines.extend(warnings.iter().map(|warning| format!("WARNING {}: {}", warning.code, warning.message)));
    lines.push(format!("SI check: {} warning(s)", warnings.len()));
    Ok(SiCheckReport { document: Value::Object(document), lines })
}

impl SiCheckReport {
    /// `to_json()`.
    pub fn json(&self) -> String {
        Dumps::new().indent(2).sort_keys(true).dumps(&self.document) + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_python() {
        assert_eq!(round_digits(0.0009765625, 9), 0.000976562);
        assert_eq!(round_digits(50.1234567894999, 9), 50.123456789);
        assert_eq!(round_digits(-2.5e-10, 9), -0.0);
        assert_eq!(copper_json::python_float_repr(round_digits(89.99999999999, 9)), "90.0");
        assert_eq!(ohms(49.3456789012345).to_sci_string(), "49.345678901");
    }
}
