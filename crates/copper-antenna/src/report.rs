//! The `copperscript-antenna-synthesis/v0.1` report.

use copper_json::{Dumps, Value as Json};

use copper_pcb::antenna::{AntennaSite, GENERATOR};

use crate::matching::{Element, return_loss_db};
use crate::model::{Problem, SOLVER};
use crate::optimize::Outcome;

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn number(value: f64, decimals: usize) -> Json {
    Json::from(format!("{value:.decimals$}"))
}

/// The assumptions every synthesized result rests on.
pub fn assumptions(problem: &Problem) -> Vec<String> {
    let site = problem.site;
    vec![
        format!(
            "Model holds the board outline, the declared stack-up and the {} copper zones only; components, other \
             nets, zone holes, the enclosure, battery and cables are absent",
            site.ground
        ),
        problem.port_description(),
        if problem.stitching.is_empty() {
            "One ground layer: no stitching vias modelled".to_owned()
        } else {
            format!(
                "Ground layers stitched by {} vias at {:.1} mm pitch (a tenth of the guided wavelength); the \
                 board needs comparable stitching for this result to apply",
                problem.stitching.len(),
                problem.stitch_pitch
            )
        },
        "Zero-thickness PEC copper; dielectric loss from each layer's loss tangent at the band centre".to_owned(),
        match &site.short {
            Some(link) => format!(
                "The short link {} is modelled as ideal copper from its feed land through its ground land and via \
                 to the ground plane; a 0402 0 ohm link adds about 0.4 nH, a tuning inductor its own value",
                link.reference
            ),
            None => "No DC short to ground: the radiator is fed-net copper only".to_owned(),
        },
        "Matching values are ideal lossless parts; part Q, parasitics, tolerance and E-series snapping are not \
         modelled"
            .to_owned(),
    ]
}

/// The report document for `outcome`.
pub fn document(problem: &Problem, site: &AntennaSite, digest: &str, outcome: &Outcome) -> Json {
    let evaluation = &outcome.evaluation;
    let score = &outcome.score;
    let network = &score.network;
    let curve: Vec<Json> = evaluation
        .freqs
        .iter()
        .zip(&evaluation.z)
        .zip(&evaluation.s11_db)
        .map(|((f, z), s11)| {
            object(vec![
                ("frequency_hz", number(*f, 0)),
                ("s11_db", number(*s11, 2)),
                ("matched_s11_db", number(return_loss_db(network.apply(*z, *f), 50.0), 2)),
                ("z_ohm", Json::Array(vec![number(z.re, 2), number(z.im, 2)])),
            ])
        })
        .collect();
    let elements: Vec<Json> = network
        .elements
        .iter()
        .map(|element| {
            let (position, kind, value, unit) = match *element {
                Element::SeriesL(h) => ("series", "inductor", h * 1e9, "nH"),
                Element::SeriesC(c) => ("series", "capacitor", c * 1e12, "pF"),
                Element::ShuntL(h) => ("shunt", "inductor", h * 1e9, "nH"),
                Element::ShuntC(c) => ("shunt", "capacitor", c * 1e12, "pF"),
            };
            object(vec![
                ("position", Json::from(position)),
                ("kind", Json::from(kind)),
                ("value", number(value, 3)),
                ("unit", Json::from(unit)),
            ])
        })
        .collect();
    let trials: Vec<Json> = outcome
        .trials
        .iter()
        .map(|t| {
            object(vec![
                ("stage", Json::from(t.stage)),
                ("fidelity", Json::from(t.fidelity)),
                ("geometry", Json::from(t.geometry.as_str())),
                ("resonance_hz", t.resonance_hz.map_or(Json::Null, |f| number(f, 0))),
                ("bare_worst_db", number(t.raw_worst_db, 2)),
                ("matched_worst_db", number(t.worst_db, 2)),
                ("cells", Json::from(t.cells as i64)),
                ("seconds", number(t.seconds, 1)),
                ("converged", Json::Bool(t.converged)),
            ])
        })
        .collect();
    let bands: Vec<Json> = score
        .bands
        .iter()
        .map(|b| {
            object(vec![
                ("band", Json::from(b.band.as_str())),
                ("bare_worst_db", number(b.raw_worst_db, 2)),
                ("matched_worst_db", number(b.worst_db, 2)),
                ("target_db", number(site.max_return_loss_db, 1)),
                ("meets_target", Json::Bool(b.worst_db <= site.max_return_loss_db)),
            ])
        })
        .collect();
    let efficiency: Vec<Json> = evaluation
        .efficiency
        .iter()
        .map(|(f, e)| object(vec![("frequency_hz", number(*f, 0)), ("radiation_efficiency", number(*e, 3))]))
        .collect();
    object(vec![
        ("schema", Json::from("copperscript-antenna-synthesis/v0.1")),
        ("generator", Json::from(GENERATOR)),
        ("solver", Json::from(SOLVER)),
        ("antenna", Json::from(site.name.as_str())),
        ("declaration", Json::from(site.declaration.as_str())),
        ("feed", Json::from(site.feed.to_string())),
        ("site_digest", Json::from(digest)),
        ("method", Json::from(outcome.method)),
        ("geometry", Json::from(outcome.geometry.to_source())),
        (
            "source_lines",
            Json::Array(vec![
                Json::from(format!("geometry = \"{}\";", outcome.geometry.to_source())),
                Json::from(format!("tuned_for = \"{digest}\";")),
            ]),
        ),
        ("final_fidelity", Json::from(evaluation.fidelity)),
        ("converged", Json::Bool(evaluation.converged)),
        ("resonance_hz", score.resonance_hz.map_or(Json::Null, |f| number(f, 0))),
        ("bands", Json::Array(bands)),
        ("matching_network", Json::Array(elements)),
        ("radiation_efficiency", Json::Array(efficiency)),
        ("assumptions", Json::Array(assumptions(problem).into_iter().map(Json::from).collect())),
        (
            "unresolved",
            Json::Array(vec![
                Json::from("Bench return-loss measurement in the final enclosure"),
                Json::from("Matching-part selection, tolerance and tuning on the assembled board"),
                Json::from("Radiated performance and regulatory testing"),
            ]),
        ),
        ("trials", Json::Array(trials)),
        ("curve", Json::Array(curve)),
        ("solver_warnings", Json::Array(evaluation.warnings.iter().map(|w| Json::from(w.as_str())).collect())),
    ])
}

/// The document as text.
pub fn render(document: &Json) -> String {
    Dumps::new().indent(2).dumps(document) + "\n"
}
