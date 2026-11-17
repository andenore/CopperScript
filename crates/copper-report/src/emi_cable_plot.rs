//! The per-driver voltage spectra at a cable connector (`copper emi-check --run RUN --plot-spectrum OUT.svg` when the
//! report has `EMI-CABLE-COMMON-MODE` findings, and the thumbnail of the connector page of `--plot-summary`): one
//! panel per driver with a published voltage (each differential pair's common-mode voltage at the connector, each
//! ground-noise source with a voltage), the lines on a logarithmic frequency axis in dBuV.
//!
//! **Voltages at the connector, not a field.** There is no limit line, no margin and no field in these pictures: a
//! field would need the bundle's net current, which the board model does not have, so there is nothing to compare a
//! limit with. Every spectrum quantity of a driver (the continuous bound, each named pattern's lines, random data in a
//! resolution bandwidth, a clock's lines) is its own mark and is never merged into one curve; a driver whose
//! quantity is a first-arrival voltage says so in its title, because that is the first wavefront before any
//! reflection and not a steady-state value. Only values already in the `measures` of the findings are drawn.

use crate::emi_field::{FieldPicture, text_px, wrap_px};
use crate::emi_plot::{escape, n};
use crate::emi_signal::{FLOOR_DBUV, Line, Spectrum};

/// Width of the full-size picture and of the thumbnail, px.
pub const WIDTH: f64 = 900.0;
pub const THUMB_WIDTH: f64 = 664.0;
/// Panels drawn at most per picture; the rest are named.
pub const MAX_PANELS: usize = 12;
pub const MAX_THUMB_PANELS: usize = 4;

const MARGIN: f64 = 24.0;
const LEFT: f64 = 70.0;
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const GRID: &str = "#d9e0e7";
const AXIS: &str = "#aab4be";
const CONTINUOUS: &str = "#1d5fa8";
const PATTERN: &str = "#2a9d8f";
const RANDOM: &str = "#8e44ad";
const CLOCK: &str = "#c2410c";

/// How a spectrum quantity is drawn: its family name, colour and marker.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Continuous,
    Pattern,
    Random,
    Clock,
}

fn family(quantity: &str) -> Family {
    if quantity == "continuous_bound" {
        Family::Continuous
    } else if quantity.starts_with("pattern_lines") {
        Family::Pattern
    } else if quantity.starts_with("random_data") {
        Family::Random
    } else {
        Family::Clock
    }
}

impl Family {
    fn colour(self) -> &'static str {
        match self {
            Family::Continuous => CONTINUOUS,
            Family::Pattern => PATTERN,
            Family::Random => RANDOM,
            Family::Clock => CLOCK,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Family::Continuous => "continuous bound (diamond)",
            Family::Pattern => "pattern lines, each named (circle)",
            Family::Random => "random data in the resolution bandwidth (square)",
            Family::Clock => "clock lines (triangle)",
        }
    }
}

fn marker(family: Family, cx: f64, cy: f64, hollow: bool) -> String {
    let fill = if hollow { "#ffffff".to_owned() } else { family.colour().to_owned() };
    let style = format!("fill=\"{fill}\" stroke=\"{}\" stroke-width=\"1.5\"", family.colour());
    match family {
        Family::Continuous => format!(
            "<polygon points=\"{},{} {},{} {},{} {},{}\" {style}/>",
            n(cx),
            n(cy - 5.0),
            n(cx + 5.0),
            n(cy),
            n(cx),
            n(cy + 5.0),
            n(cx - 5.0),
            n(cy)
        ),
        Family::Pattern => format!("<circle cx=\"{}\" cy=\"{}\" r=\"4.5\" {style}/>", n(cx), n(cy)),
        Family::Random => {
            format!("<rect x=\"{}\" y=\"{}\" width=\"9\" height=\"9\" {style}/>", n(cx - 4.5), n(cy - 4.5))
        }
        Family::Clock => format!(
            "<polygon points=\"{},{} {},{} {},{}\" {style}/>",
            n(cx),
            n(cy - 5.5),
            n(cx + 5.0),
            n(cy + 4.5),
            n(cx - 5.0),
            n(cy + 4.5)
        ),
    }
}

fn text_element(x: f64, y: f64, size: f64, fill: &str, class: &str, body: &str, extra: &str) -> String {
    format!(
        "<text class=\"{class}\" x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{fill}\"{extra}>{}</text>",
        n(x),
        n(y),
        n(size),
        escape(body)
    )
}

/// `1 GHz`, `100 MHz`, `10 kHz`.
fn hertz(value: f64) -> String {
    if value >= 1e9 {
        format!("{} GHz", n(value / 1e9))
    } else if value >= 1e6 {
        format!("{} MHz", n(value / 1e6))
    } else {
        format!("{} kHz", n(value / 1e3))
    }
}

/// The title of a driver's panel: the driver, the quantity label of the finding and what it means.
fn panel_title(spectrum: &Spectrum) -> String {
    let what = match spectrum.driver {
        "ground_noise" => "ground-noise voltage, current driven (a voltage between the loops, not a common-mode voltage at the connector)".to_owned(),
        _ if spectrum.first_arrival() => {
            let cable = spectrum
                .cable_impedance_ohm
                .map_or_else(String::new, |ohm| format!(", cable {} ohm declared, not matched", n(ohm)));
            format!("first_arrival_voltage: the first wavefront before any reflection, NOT a steady-state value{cable}")
        }
        _ => match &spectrum.termination {
            Some(termination) => format!("{} ({termination})", spectrum.quantity),
            None => spectrum.quantity.clone(),
        },
    };
    format!("{}: {what}", spectrum.name)
}

fn line_text(line: &Line) -> String {
    if line.dbuv <= FLOOR_DBUV {
        format!("{}: no line", line.quantity)
    } else {
        format!("{}: {} dBuV at {}", line.quantity, n(line.dbuv), hertz(line.frequency_hz))
    }
}

/// The picture of `spectra` (all of one connector): `compact` is the thumbnail form for the summary page (fewer
/// panels, no value lists, no footer).
pub fn connector_picture(connector: &str, spectra: &[Spectrum], compact: bool) -> FieldPicture {
    let width = if compact { THUMB_WIDTH } else { WIDTH };
    let limit = if compact { MAX_THUMB_PANELS } else { MAX_PANELS };
    let shown: Vec<&Spectrum> = spectra.iter().take(limit).collect();
    let text_size = if compact { 10.0 } else { 11.0 };
    let mut out: Vec<String> = Vec::new();
    let mut y = if compact { 20.0 } else { 28.0 };
    out.push(text_element(
        MARGIN,
        y,
        if compact { 14.0 } else { 17.0 },
        INK,
        "title",
        &format!("{connector}: voltage spectra at the connector, per driver (dBuV)"),
        " font-weight=\"bold\"",
    ));
    y += if compact { 16.0 } else { 20.0 };
    let intro = if compact {
        "Voltages against the board reference at the connector, not a field; no limit line."
    } else {
        "Voltages between the conductors and the board reference at the connector's ground pins, single-sided peak amplitude of each spectrum line. Not a field: no limit line and no margin are drawn, because there is no field estimate to compare with one. Each spectrum quantity is its own mark, never merged into one curve."
    };
    for line in wrap_px(intro, text_size + 1.0, width - 2.0 * MARGIN) {
        out.push(text_element(MARGIN, y, text_size + 1.0, MUTED, "intro", &line, ""));
        y += (text_size + 1.0) * 1.35;
    }
    y += 4.0;

    // The axes are common to the panels of one picture: the frequency decades around the lines, 60 dB below the strongest.
    let drawn = |line: &&Line| line.dbuv > FLOOR_DBUV && line.frequency_hz > 0.0;
    let all: Vec<&Line> = shown.iter().flat_map(|s| s.lines.iter()).filter(drawn).collect();
    let (f_lo, f_hi) =
        all.iter().fold((f64::INFINITY, 0.0_f64), |(lo, hi), l| (lo.min(l.frequency_hz), hi.max(l.frequency_hz)));
    let decade_lo = if all.is_empty() { 1e6 } else { 10f64.powf(f_lo.log10().floor()) };
    let mut decade_hi = if all.is_empty() { 1e9 } else { 10f64.powf(f_hi.log10().ceil()) };
    if decade_hi <= decade_lo * 1.0001 {
        decade_hi = decade_lo * 10.0;
    }
    let top_db = all.iter().map(|l| l.dbuv).fold(f64::NEG_INFINITY, f64::max);
    let y_max = if all.is_empty() { 60.0 } else { ((top_db + 1.0) / 10.0).ceil() * 10.0 };
    let span = if compact { 50.0 } else { 60.0 };
    let y_min = y_max - span;
    let plot_w = width - LEFT - MARGIN;
    let plot_h = if compact { 54.0 } else { 78.0 };
    let x_of = |f: f64| LEFT + (f.log10() - decade_lo.log10()) / (decade_hi.log10() - decade_lo.log10()) * plot_w;

    for spectrum in &shown {
        out.push(format!(
            "<g class=\"driver-panel\" data-connector=\"{}\" data-driver=\"{}\" data-driver-kind=\"{}\" data-quantity=\"{}\">",
            escape(connector),
            escape(&spectrum.name),
            spectrum.driver,
            escape(&spectrum.quantity)
        ));
        for line in wrap_px(&panel_title(spectrum), text_size + 1.0, width - 2.0 * MARGIN) {
            out.push(text_element(MARGIN, y, text_size + 1.0, INK, "panel-title", &line, " font-weight=\"bold\""));
            y += (text_size + 1.0) * 1.3;
        }
        y += 4.0;
        let top = y;
        let bottom = top + plot_h;
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fbfcfd\" stroke=\"{AXIS}\"/>",
            n(LEFT),
            n(top),
            n(plot_w),
            n(plot_h)
        ));
        // Gridlines: each decade and its 2 and 5, labelled at the decades; 10 dB lines labelled at the left.
        let mut decade = decade_lo;
        while decade < decade_hi * 0.9999 {
            for (k, multiple) in [1.0, 2.0, 5.0].into_iter().enumerate() {
                let f = decade * multiple;
                if f > decade_hi * 1.0001 {
                    continue;
                }
                let x = x_of(f);
                out.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"{}\"/>",
                    n(x),
                    n(top),
                    n(x),
                    n(bottom),
                    if k == 0 { "1" } else { "0.5" }
                ));
                if k == 0 {
                    out.push(text_element(x, bottom + 12.0, 9.0, MUTED, "tick", &hertz(f), " text-anchor=\"middle\""));
                }
            }
            decade *= 10.0;
        }
        out.push(text_element(
            x_of(decade_hi),
            bottom + 12.0,
            9.0,
            MUTED,
            "tick",
            &hertz(decade_hi),
            " text-anchor=\"middle\"",
        ));
        let mut level = y_min;
        while level <= y_max + 0.001 {
            let yy = bottom - (level - y_min) / span * plot_h;
            out.push(format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\" stroke-width=\"0.5\"/>",
                n(LEFT),
                n(yy),
                n(LEFT + plot_w),
                n(yy)
            ));
            if (level - y_min) % 20.0 < 0.001 {
                out.push(text_element(
                    LEFT - 6.0,
                    yy + 3.0,
                    9.0,
                    MUTED,
                    "tick",
                    &format!("{} dBuV", n(level)),
                    " text-anchor=\"end\"",
                ));
            }
            level += 10.0;
        }
        for line in spectrum.lines.iter().filter(|l| l.dbuv > FLOOR_DBUV && l.frequency_hz > 0.0) {
            let family = family(&line.quantity);
            let x = x_of(line.frequency_hz).clamp(LEFT, LEFT + plot_w);
            let below = line.dbuv < y_min;
            let yy = if below { bottom } else { bottom - (line.dbuv.min(y_max) - y_min) / span * plot_h };
            out.push(format!(
                "<g class=\"driver-line\" data-driver=\"{}\" data-quantity=\"{}\" data-frequency-hz=\"{}\" data-dbuv=\"{}\"{}>",
                escape(&spectrum.name),
                escape(&line.quantity),
                n(line.frequency_hz),
                n(line.dbuv),
                if below { " data-below-axis=\"true\"" } else { "" }
            ));
            if !below {
                out.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
                    n(x),
                    n(bottom),
                    n(x),
                    n(yy),
                    family.colour()
                ));
            }
            out.push(marker(family, x, yy, below));
            out.push("</g>".to_owned());
        }
        // The value lines start below the frequency tick labels (drawn at `bottom + 12`).
        y = bottom + if compact { 16.0 } else { 28.0 };
        if !compact {
            let values: Vec<String> = spectrum.lines.iter().map(line_text).collect();
            for line in wrap_px(&values.join("; "), 10.0, width - 2.0 * MARGIN) {
                out.push(text_element(MARGIN, y, 10.0, INK, "values", &line, ""));
                y += 13.0;
            }
        }
        y += if compact { 4.0 } else { 10.0 };
        out.push("</g>".to_owned());
    }
    if spectra.len() > shown.len() {
        out.push(text_element(
            MARGIN,
            y,
            text_size + 1.0,
            MUTED,
            "more",
            &format!(
                "+{} more driver(s) with a voltage: the full-size picture (--plot-spectrum) draws up to {MAX_PANELS}, the findings hold every one",
                spectra.len() - shown.len()
            ),
            "",
        ));
        y += 16.0;
    }
    // Legend of the families present.
    let mut x = MARGIN;
    let mut seen: Vec<Family> = Vec::new();
    for line in spectra.iter().flat_map(|s| s.lines.iter()) {
        let f = family(&line.quantity);
        if !seen.contains(&f) {
            seen.push(f);
        }
    }
    for f in seen {
        let label = f.name();
        if x + 20.0 + text_px(label, 10.0) > width - MARGIN {
            x = MARGIN;
            y += 16.0;
        }
        out.push(marker(f, x + 6.0, y - 3.0, false));
        out.push(text_element(x + 16.0, y, 10.0, INK, "legend", label, ""));
        x += 28.0 + text_px(label, 10.0);
    }
    y += 20.0;
    if !compact {
        let footer = "Voltages at the connector, not a field, a current or a margin; nothing here is a pass or a fail and none of it is correlated with a measurement. Axis: dBuV of the single-sided peak amplitude of one spectrum line, 60 dB below the strongest drawn line at the bottom; a line below the axis is drawn hollow on its bottom edge. A first-arrival voltage is the first wavefront before any reflection, not a steady-state value. Not modelled: the bundle's net current, the cable's routing, far-end termination and resonance, the shield's transfer impedance, and radiation.";
        for line in wrap_px(footer, 11.0, width - 2.0 * MARGIN) {
            out.push(text_element(MARGIN, y, 11.0, MUTED, "footer", &line, ""));
            y += 15.0;
        }
    }
    FieldPicture { content: out.join("\n"), width, height: y + 6.0 }
}

/// The full-size pictures of the connector spectra of `document` (one per connector with a published voltage), in the
/// order of the findings; empty when no connector has one.
pub fn connector_pictures(document: &copper_json::Value) -> Vec<FieldPicture> {
    crate::emi_signal::connector_spectra(document)
        .iter()
        .map(|(connector, spectra)| connector_picture(connector, spectra, false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum(name: &str, quantity: &str, driver: &'static str) -> Spectrum {
        Spectrum {
            connector: "J1".to_owned(),
            driver,
            name: name.to_owned(),
            quantity: quantity.to_owned(),
            termination: Some("matched_cable_termination".to_owned()),
            cable_impedance_ohm: (quantity == "first_arrival_voltage").then_some(150.0),
            lines: vec![
                Line { quantity: "continuous_bound".to_owned(), frequency_hz: 1.25e9, dbuv: 84.87 },
                Line { quantity: "pattern_lines:alternating".to_owned(), frequency_hz: 1.25e9, dbuv: 84.44 },
                Line { quantity: "pattern_lines:square_runs_4".to_owned(), frequency_hz: 0.9375e9, dbuv: 72.6 },
                Line { quantity: "random_data_in_rbw".to_owned(), frequency_hz: 1.25e9, dbuv: 41.26 },
                Line { quantity: "pattern_lines:square_runs_2".to_owned(), frequency_hz: 1.25e9, dbuv: -200.0 },
            ],
        }
    }

    /// The text of every `<text>` element, joined by a space (a paragraph is wrapped over several).
    fn plain(content: &str) -> String {
        content
            .split("<text ")
            .skip(1)
            .map(|part| part.split_once('>').unwrap().1.split("</text>").next().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn each_quantity_is_its_own_mark_and_no_limit_is_drawn() {
        let spectra = [
            spectrum("A_P/A_N", "common_mode_voltage_at_connector", "differential_pair"),
            spectrum("B_P/B_N", "first_arrival_voltage", "differential_pair"),
        ];
        let picture = connector_picture("J1", &spectra, false);
        assert_eq!(picture.width, WIDTH);
        // Four drawn lines per panel (the fifth has no content), two panels.
        assert_eq!(picture.content.matches("class=\"driver-line\"").count(), 8);
        assert_eq!(picture.content.matches("class=\"driver-panel\"").count(), 2);
        assert!(picture.content.contains("data-quantity=\"pattern_lines:square_runs_4\""));
        assert_eq!(
            panel_title(&spectra[1]),
            "B_P/B_N: first_arrival_voltage: the first wavefront before any reflection, NOT a steady-state value, cable 150 ohm declared, not matched"
        );
        assert_eq!(panel_title(&spectra[0]), "A_P/A_N: common_mode_voltage_at_connector (matched_cable_termination)");
        let text = plain(&picture.content);
        assert!(text.contains("pattern_lines:square_runs_2: no line"), "{text}");
        assert!(text.contains("Not a field"), "{text}");
        // The only words "limit" and "margin" are in the statements that none is drawn.
        for word in ["limit", "margin"] {
            for (at, _) in text.match_indices(word) {
                assert!(text[..at].ends_with("no ") || text[..at].ends_with("or a "), "{word} at {at}: {text}");
            }
        }
        assert!(!picture.content.contains("class=\"limit"));
        // Every text starts inside the picture.
        for part in picture.content.split("<text ").skip(1) {
            let x: f64 = part.split("x=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
            let y: f64 = part.split("y=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
            assert!((0.0..=picture.width).contains(&x) && (0.0..=picture.height).contains(&y), "{part}");
        }
    }

    #[test]
    fn the_thumbnail_is_narrower_and_draws_at_most_four_panels() {
        let spectra: Vec<Spectrum> = (0..6)
            .map(|k| spectrum(&format!("P{k}"), "common_mode_voltage_at_connector", "differential_pair"))
            .collect();
        let compact = connector_picture("J1", &spectra, true);
        assert_eq!(compact.width, THUMB_WIDTH);
        assert_eq!(compact.content.matches("class=\"driver-panel\"").count(), MAX_THUMB_PANELS);
        assert!(compact.content.contains("+2 more driver(s)"));
    }
}
