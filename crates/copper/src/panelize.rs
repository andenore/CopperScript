//! `copper panelize PANEL -o OUTPUT [--report REPORT]`: compose a `panel`
//! document's routed boards into one panel `.kicad_pcb` (with its
//! `.kicad_pro` and `.kicad_dru`) for the native fill, DRC and plot, and
//! check it against the panel's fab preset. `--dump-config` prints the
//! effective panel values with their origins and the preset's rules.

use std::io::Write;
use std::path::Path;

use copper_json::Dumps;
use copper_lang::compiler::load_design;
use copper_lang::elaborate::elaborate;
use copper_lang::footprints::FootprintResolver;
use copper_lang::model::ConstraintKind;
use copper_lang::panel::Side;
use copper_panel::compose::Point;
use copper_panel::intent::{ANTENNA_TAB_MARGIN, BoardIntent, KeepClear, OpenEnd, Overhang, Radiator, side_of};
use copper_pcb::antenna::{chosen_geometry, layout, resolve_sites};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};

use crate::option_parser::{self, flag, short_value, value};

/// A board's edge intent from its source: outline, antenna regions opened to
/// the perimeter, body overhangs and the radiators' open ends.
fn board_intent(source: &Path, locked: bool, offline: bool) -> Result<BoardIntent, String> {
    let design = load_design(source, locked, offline).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut bare = design.clone();
    bare.electrical.constraints.retain(|c| c.kind != ConstraintKind::Antenna);
    for module in bare.electrical.module_definitions.values_mut() {
        module.constraints.retain(|c| c.kind != ConstraintKind::Antenna);
    }
    let fail = |e: copper_pcb::Error| match e {
        copper_pcb::Error::Value(message) | copper_pcb::Error::NotImplemented(message) => {
            format!("{}: {message}", source.display())
        }
    };
    let options = PhysicalOptions::new(100.0, 80.0, 2, "generic").map_err(fail)?;
    let base = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut resolver = FootprintResolver::new(&base, &[], false, locked, offline);
    let physical = physicalize(&bare, Footprints::Resolved(&mut resolver), &options, locked, offline).map_err(fail)?;
    let ring: Vec<Point> = physical.outline.vertices.iter().map(|p| (p.x_nm, p.y_nm)).collect();
    if !physical.outline.cutouts.is_empty() || physical.outline.circular_boundary.is_some() {
        return Err(format!("{}: panels support plain polygon outlines without cutouts so far", source.display()));
    }
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for (x, y) in &ring {
        b = [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)];
    }
    let mut intent = BoardIntent {
        outline: vec![ring],
        // The export adds a footprint for every mechanical hole and slot.
        references: physical
            .placements
            .iter()
            .map(|p| p.reference.clone())
            .chain(physical.mechanical_holes.iter().map(|h| copper_pcb::kicad_pcb::mechanical_hole_reference(&h.id)))
            .chain(physical.mechanical_slots.iter().map(|s| copper_pcb::kicad_pcb::mechanical_slot_reference(&s.id)))
            .collect(),
        ..BoardIntent::default()
    };
    for overhang in &physical.body_overhangs {
        let Some(edge) = physical.boundary_edges.iter().find(|e| e.id == overhang.edge) else { continue };
        let unit = ((edge.end.x_nm - edge.start.x_nm).signum(), (edge.end.y_nm - edge.start.y_nm).signum());
        let at = |t: i64| (edge.start.x_nm + unit.0 * t, edge.start.y_nm + unit.1 * t);
        if let Some((side, start, end)) = side_of(b, at(overhang.start_nm), at(overhang.end_nm)) {
            intent.overhangs.push(Overhang {
                reference: overhang.reference.clone(),
                side,
                start,
                end,
                distance: overhang.distance_nm,
            });
        }
    }
    let flat = elaborate(&design.electrical).map_err(|e| format!("{}: {e}", source.display()))?;
    for site in resolve_sites(&physical, &flat).map_err(fail)? {
        let area = site.area;
        // Every side the region reaches or comes within the margin of keeps
        // tabs off its stretch plus the margin, round a corner too.
        let m = ANTENNA_TAB_MARGIN;
        let along_x = (area.min_x - b[0], area.max_x - b[0]);
        let along_y = (area.min_y - b[1], area.max_y - b[1]);
        for (side, distance, (start, end)) in [
            (Side::Top, area.min_y - b[1], along_x),
            (Side::Bottom, b[3] - area.max_y, along_x),
            (Side::Left, area.min_x - b[0], along_y),
            (Side::Right, b[2] - area.max_x, along_y),
        ] {
            if distance <= m {
                intent.keep_clear.push(KeepClear {
                    side,
                    start: start - m,
                    end: end + m,
                    reason: if distance <= 0 {
                        format!("antenna {}'s region opened to the perimeter", site.name)
                    } else {
                        format!(
                            "antenna {}'s region, {} mm from this side",
                            site.name,
                            copper_panel::compose::mm(distance)
                        )
                    },
                });
            }
        }
        // The geometry the source binds: a stale tuned_for (an input of the
        // full-wave model changed, or a new model revision) fails here as it
        // does in route-board, rather than leaving the radiator unchecked.
        let (geometry, _) = chosen_geometry(&physical, &site).map_err(fail)?;
        let realized = layout(&site, &geometry).map_err(fail)?;
        if realized.tiles.is_empty()
            && let Some((u, v)) = realized.path.last()
        {
            let end = site.frame.to_board(*u, *v);
            intent.open_ends.push(OpenEnd {
                at: (end.x_nm, end.y_nm),
                net: site.feed_net.clone(),
                layer: "F.Cu".to_owned(),
            });
            intent.radiators.push(Radiator {
                name: site.name.clone(),
                net: site.feed_net.clone(),
                layer: "F.Cu".to_owned(),
                path: realized
                    .path
                    .iter()
                    .map(|(u, v)| {
                        let p = site.frame.to_board(*u, *v);
                        (p.x_nm, p.y_nm)
                    })
                    .collect(),
            });
        }
    }
    Ok(intent)
}

const USAGE: &str =
    "usage: copper panelize [-h] (-o OUTPUT | --dump-config) [--report REPORT] [--strict] [--locked] [--offline] panel";

const ABOUT: &str = "compose a panel document's routed boards into one panel .kicad_pcb with rails, \
mouse-bite tabs, fiducials and tooling holes; nets and references are prefixed per board, zone fills \
are dropped (refill the panel with copper fill), and the .kicad_pro and .kicad_dru are written beside it. \
Each board's .copper source is compiled for its edge intent: tabs may not touch an antenna region opened \
to the perimeter or a body overhang, and an overhanging body may not reach a rail or another board unless \
the panel lists it in assemble_after_depanel. With preset = NAME (jlcpcb, jlcpcb-smt, pcbway, \
eurocircuits) the preset fills the properties the panel leaves out and the panel is checked against \
the fab's documented rules: each warn or fail is printed with its source and kept in the report; \
--strict exits 1 on a fail. --dump-config prints the effective values with their origins and the \
preset's rules without composing. Run copper fill, then copper drc --expected-open-ends REPORT \
(the radiators' open ends from this report) on the result before plotting.";

/// The effective panel values with origins, and the preset's rules.
fn dump_config(spec: &copper_lang::panel::PanelSpec, out: &mut dyn Write) {
    let preset = spec.preset.as_deref().and_then(copper_lang::panel_presets::preset);
    let _ = writeln!(
        out,
        "panel {} ({})",
        spec.name,
        preset.map_or_else(|| "no preset".to_owned(), |p| format!("preset {}: {}", p.name, p.summary))
    );
    for (key, value, origin) in copper_panel::checks::config(spec) {
        let _ = writeln!(out, "  {key} = {value};  // {origin}");
    }
    if let Some(preset) = preset {
        let _ = writeln!(out, "rules:");
        for rule in preset.rules {
            let _ = writeln!(
                out,
                "  {} {} {}: {} ({})",
                rule.level.as_str(),
                rule.kind.name(),
                copper_panel::checks::limit(rule.kind),
                rule.note,
                rule.source
            );
        }
    }
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper panelize: error: {message}");
        2
    };
    let specs = [
        short_value("--output", "-o"),
        value("--report"),
        flag("--dump-config"),
        flag("--strict"),
        flag("--locked"),
        flag("--offline"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\n{ABOUT}");
        return 0;
    }
    let [panel] = parsed.positionals.as_slice() else {
        return usage_error(err, "exactly one panel file is required");
    };
    let spec = match copper_lang::panel::load_panel(Path::new(panel)) {
        Ok(spec) => spec,
        Err(error) => {
            let _ = writeln!(err, "PANEL ERROR: {error}");
            return 1;
        }
    };
    if parsed.flag("--dump-config") {
        dump_config(&spec, out);
        return 0;
    }
    let Some(output) = parsed.value("--output") else {
        return usage_error(err, "the following arguments are required: -o/--output");
    };
    let output = Path::new(output);
    if output.extension().and_then(|e| e.to_str()) != Some("kicad_pcb") {
        return usage_error(err, "the output must be a .kicad_pcb file");
    }
    let base = Path::new(panel).parent().unwrap_or(Path::new("."));
    let stem = output.file_stem().and_then(|s| s.to_str()).unwrap_or("panel");
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let intents: Result<Vec<BoardIntent>, String> =
        spec.boards.iter().map(|board| board_intent(&base.join(&board.source), locked, offline)).collect();
    let result = intents.and_then(|intents| {
        copper_panel::read_boards(&spec, base).and_then(|files| copper_panel::panelize(&spec, &files, &intents, stem))
    });
    let panel_files = match result {
        Ok(files) => files,
        Err(message) => {
            let _ = writeln!(err, "PANEL ERROR: {message}");
            return 1;
        }
    };
    let report_path = match parsed.value("--report") {
        Some(path) => Path::new(path).to_path_buf(),
        None => output.with_extension("panel.json"),
    };
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let _ = writeln!(err, "PANEL ERROR: cannot create {}: {e}", parent.display());
        return 1;
    }
    let writes = [
        (output.to_path_buf(), panel_files.pcb.clone()),
        (output.with_extension("kicad_pro"), panel_files.pro.clone()),
        (output.with_extension("kicad_dru"), panel_files.dru.clone()),
        (report_path.clone(), Dumps::new().indent(2).dumps(&panel_files.report) + "\n"),
    ];
    for (path, text) in writes {
        if let Err(e) = std::fs::write(&path, text) {
            let _ = writeln!(err, "PANEL ERROR: cannot write {}: {e}", path.display());
            return 1;
        }
    }
    let summary = |key: &str| match &panel_files.report {
        copper_json::Value::Object(o) => o.get(key).map(|v| Dumps::compact().dumps(v)).unwrap_or_default(),
        _ => String::new(),
    };
    let _ = writeln!(
        out,
        "PANEL {}: {} boards, size {} mm, {} tabs -> {}\nReport -> {}\nNext: copper fill {} -o FILLED.kicad_pcb, then copper drc FILLED.kicad_pcb --project PANEL.kicad_pro --rules PANEL.kicad_dru --expected-open-ends REPORT",
        spec.name,
        spec.boards.len(),
        summary("size_mm"),
        spec.tabs.len(),
        output.display(),
        report_path.display(),
        output.display()
    );
    if let Some(preset) = &spec.preset {
        use copper_panel::checks::Status;
        let count = |status: Status| panel_files.checks.iter().filter(|c| c.status == status).count();
        let _ = writeln!(
            out,
            "Checks against preset {preset}: {} rule(s), {} fail, {} warn",
            panel_files.checks.len(),
            count(Status::Fail),
            count(Status::Warn)
        );
        for check in panel_files.checks.iter().filter(|c| c.status != Status::Pass) {
            let _ = writeln!(
                out,
                "  {} {}: {}, limit {} ({}; {})",
                check.status.as_str(),
                check.rule,
                check.value,
                check.limit,
                check.note,
                check.source
            );
        }
        if parsed.flag("--strict") && count(Status::Fail) > 0 {
            let _ = writeln!(
                err,
                "PANEL ERROR: the panel fails {} rule(s) of preset {preset} (--strict)",
                count(Status::Fail)
            );
            return 1;
        }
    }
    0
}
