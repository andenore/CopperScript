//! The interactive layer of the bench pages: the chart data each figure
//! embeds as JSON, and the page's inline script and style that draw it.
//!
//! The script (`chart.js`, inlined into every page; no network, no library)
//! replaces each static figure with an interactive SVG drawn from the data:
//!
//! - drag across a panel to zoom the x axis to that range, shift-drag (or
//!   the arrow keys) to pan, double-click or `Reset` for the whole axis; the
//!   y axes follow the visible data;
//! - click to place cursor A, click again for cursor B (later clicks move
//!   the nearer cursor); the readout lists every visible curve at both
//!   cursors with the differences (`delta x`, `delta y`, and `1/delta x` on
//!   time axes); without cursors it follows the pointer;
//! - the legend toggles curves per step, and per stepped value when the
//!   steps are styled by their params; `All` and `None` show or hide every step;
//! - `x: log` and `y: log` buttons switch a frequency, time or positive
//!   value axis between linear and logarithmic.
//!
//! Without scripting the static SVG stays as it is. The data holds every
//! sample of a frequency sweep; a time record of more than
//! [`MOST_POINTS`] samples per step is thinned to evenly spaced samples
//! (the script draws the lowest and highest sample of each pixel column of
//! what it holds; the CSV keeps every point).

use copper_lang::expression::Units;

/// The page's script.
pub const SCRIPT: &str = include_str!("chart.js");

/// The style of the interactive figure (toolbar, legend, readout).
pub const STYLE: &str = "figure.chart .live{display:none}figure.chart.ready .static{display:none}figure.chart.ready .live{display:block}\
.live .bar{display:flex;flex-wrap:wrap;gap:6px;align-items:center;margin:4px 0 6px 0;font-size:12px;color:#52606d}\
.live button{font:12px Helvetica,Arial,sans-serif;padding:2px 8px;border:1px solid #aab4be;background:#ffffff;color:#1f2933;border-radius:3px;cursor:pointer}\
.live button:hover{border-color:#52606d}.live button.on{background:#1f2933;color:#ffffff;border-color:#1f2933}\
.live .legend{display:flex;flex-wrap:wrap;gap:2px 10px;font-size:12px;margin:2px 0 4px 0;align-items:center}\
.live .legend .title{color:#52606d;margin-right:2px}\
.live .item{display:inline-flex;align-items:center;gap:5px;cursor:pointer;user-select:none;padding:1px 2px;border-radius:3px}\
.live .item:hover{background:#eef1f4}.live .item.off{opacity:.35}.live .item svg{display:inline}\
.live details{font-size:12px;margin:2px 0 4px}.live summary{cursor:pointer;color:#52606d}\
.live svg.plot{display:block;width:100%;height:auto;touch-action:none;cursor:crosshair;user-select:none}\
.live .readout{font-size:12px;font-variant-numeric:tabular-nums;margin:4px 0 0;overflow-x:auto}\
.live .readout table{border-collapse:collapse}.live .readout td,.live .readout th{padding:1px 10px 1px 0;border-bottom:1px solid #eef1f4;text-align:right;white-space:nowrap}\
.live .readout td:first-child,.live .readout th:first-child{text-align:left}\
.live .sw{display:inline-block;width:16px;height:0;border-top:2px solid;vertical-align:middle;margin-right:5px}\
.live .hint{color:#52606d;font-size:12px}";

/// The most samples per step a time record keeps in the chart data.
pub const MOST_POINTS: usize = 6000;

/// A JSON string.
pub fn string(text: &str) -> String {
    copper_json::encode_str(text, true)
}

/// A number with 6 significant digits, compact but strict JSON (`null` when not finite).
pub fn num(value: f64) -> String {
    digits(value, 6)
}

/// A number with `significant` digits in the shortest of fixed and exponent form.
fn digits(value: f64, significant: usize) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let exponent = format!("{:.*e}", significant - 1, value);
    // `1.23450e-3` -> `1.2345e-3`.
    let (mantissa, power) = exponent.split_once('e').unwrap_or((&exponent, "0"));
    let mantissa = if mantissa.contains('.') { mantissa.trim_end_matches('0').trim_end_matches('.') } else { mantissa };
    let short = if power == "0" { mantissa.to_owned() } else { format!("{mantissa}e{power}") };
    let magnitude = value.abs().log10().floor() as i32;
    if (-3..6).contains(&magnitude) {
        let decimals = (significant as i32 - 1 - magnitude).max(0) as usize;
        let fixed = format!("{value:.decimals$}");
        let fixed =
            if fixed.contains('.') { fixed.trim_end_matches('0').trim_end_matches('.').to_owned() } else { fixed };
        if fixed.len() <= short.len() {
            return fixed;
        }
    }
    short
}

/// The values at `indices` as a JSON array with `significant` digits.
pub fn array(indices: &[usize], values: &[f64], significant: usize) -> String {
    let items: Vec<String> = indices.iter().filter_map(|&i| values.get(i)).map(|v| digits(*v, significant)).collect();
    format!("[{}]", items.join(","))
}

/// The samples of an axis the chart keeps: every one up to [`MOST_POINTS`], else evenly spaced ones with both ends.
pub fn decimate_indices(axis: &[f64]) -> Vec<usize> {
    if axis.len() <= MOST_POINTS {
        return (0..axis.len()).collect();
    }
    let mut indices: Vec<usize> = (0..MOST_POINTS).map(|k| k * (axis.len() - 1) / (MOST_POINTS - 1)).collect();
    indices.dedup();
    indices
}

/// How the script writes values of these units: `{"u":[[name, scale], ...], "pct": bool, "base": text}`.
pub fn units(units: Units) -> String {
    let (choices, percent, base) = crate::format::unit_choices(units);
    let choices: Vec<String> =
        choices.iter().map(|(name, scale)| format!("[{},{}]", string(name), num(*scale))).collect();
    format!("{{\"u\":[{}],\"pct\":{percent},\"base\":{}}}", choices.join(","), string(&base))
}

/// Plain numbers (harmonic numbers, periods).
pub fn plain_units() -> String {
    "{\"u\":[],\"pct\":false,\"base\":\"\"}".to_owned()
}

/// Numbers in one fixed unit (`dB`, `dBc`).
pub fn named_units(name: &str) -> String {
    format!("{{\"u\":[[{},1]],\"pct\":false,\"base\":\"\"}}", string(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-180.0), "-180");
        assert_eq!(num(0.0012345678), "0.00123457");
        assert_eq!(num(1.5e-9), "1.5e-9");
        assert_eq!(num(123456789.0), "1.23457e8");
        assert_eq!(num(f64::NAN), "null");
        assert_eq!(digits(52.0193, 4), "52.02");
        assert_eq!(digits(-0.5, 4), "-0.5");
    }
}
