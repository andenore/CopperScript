//! The pieces the simplified figures of signals, pairs and cable connectors share (the mode-conversion page, the
//! connector charts, the return-path map): colours, a text element, number formats with at most three significant
//! digits, a text shortened to a width, and the one footer line of a document.
//!
//! **Screening evidence, never a pass or a fail.** A figure answers one question with a chart; the numbers it leaves
//! out stay in the JSON, in `copper issues --json` and in `copper explain`.

use crate::emi_field::{FieldPicture, text_px, wrap_px};
use crate::emi_plot::{escape, n};

pub(crate) const INK: &str = "#1f2933";
pub(crate) const MUTED: &str = "#52606d";
pub(crate) const GRID: &str = "#d9e0e7";
pub(crate) const AXIS: &str = "#aab4be";
/// Anything that is ranking only or not evaluated.
pub(crate) const GREY: &str = "#9aa5b1";
pub(crate) const BAND: &str = "#eef2f6";
pub(crate) const PAIR: &str = "#1d5fa8";
pub(crate) const GROUND_NOISE: &str = "#2a9d8f";

/// The footer line of a document with signal, pair or connector figures: said once, at its end.
pub const FOOTER: &str = "Screening evidence, never a pass or a fail. Voltages and ratios, not a field; no limit applies. Details: copper issues RUN --stage emi; what is not modelled: copper explain CODE.";

/// One `<text>` element.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_el(
    x: f64,
    y: f64,
    size: f64,
    fill: &str,
    class: &str,
    anchor: &str,
    bold: bool,
    body: &str,
) -> String {
    format!(
        "<text class=\"{class}\" x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{fill}\"{}{}>{}</text>",
        n(x),
        n(y),
        n(size),
        if bold { " font-weight=\"bold\"" } else { "" },
        if anchor.is_empty() { String::new() } else { format!(" text-anchor=\"{anchor}\"") },
        escape(body)
    )
}

/// `value` with at most three significant digits and no trailing zeros: `1.25`, `938`, `0.0235`, `-35.1`.
pub(crate) fn sig3(value: f64) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).clamp(0, 6) as usize;
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text };
    if text == "-0" { "0".to_owned() } else { text }
}

/// `1.25 GHz`, `938 MHz`, `10 kHz`.
pub(crate) fn hertz(value: f64) -> String {
    if value >= 1e9 {
        format!("{} GHz", sig3(value / 1e9))
    } else if value >= 1e6 {
        format!("{} MHz", sig3(value / 1e6))
    } else {
        format!("{} kHz", sig3(value / 1e3))
    }
}

/// One decimal: the dB and dBuV of a label.
pub(crate) fn one_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    if text == "-0.0" { "0.0".to_owned() } else { text }
}

/// `text` cut to fit `width` px at `size` px, with `...` when something is cut.
pub(crate) fn fit_text(text: &str, size: f64, width: f64) -> String {
    if text_px(text, size) <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for c in text.chars() {
        if text_px(&format!("{cut}{c}..."), size) > width {
            break;
        }
        cut.push(c);
    }
    format!("{}...", cut.trim_end())
}

/// The footer of a document as lines that fit `width` px at `size` px.
pub fn footer_lines(width: f64, size: f64) -> Vec<String> {
    wrap_px(FOOTER, size, width - 48.0)
}

/// The footer as a page of its own, `width` px wide.
pub fn footer_picture(width: f64) -> FieldPicture {
    let size = 12.0;
    let mut out: Vec<String> = Vec::new();
    let mut y = 26.0;
    for line in footer_lines(width, size) {
        out.push(text_el(24.0, y, size, MUTED, "footer", "", false, &line));
        y += size + 4.0;
    }
    FieldPicture { content: out.join("\n"), width, height: y + 6.0 }
}

/// What a figure's `<text>` elements take up, for the tests: no text leaves its page, no two texts overlap.
#[cfg(test)]
pub(crate) mod check {
    use super::*;

    /// The estimated box of one text: its extent along the line from the same advance the figures use, one font size
    /// high above the baseline.
    #[derive(Clone, Debug)]
    pub(crate) struct TextBox {
        pub left: f64,
        pub right: f64,
        pub top: f64,
        pub bottom: f64,
        pub body: String,
    }

    fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let key = format!(" {name}=\"");
        let start = tag.find(&key)? + key.len();
        tag[start..].split('"').next()
    }

    fn unescape(text: &str) -> String {
        text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
    }

    /// Every `<text>` element of `content` (a figure's SVG body, in its own coordinates).
    pub(crate) fn text_boxes(content: &str) -> Vec<TextBox> {
        let mut found = Vec::new();
        for part in content.split("<text ").skip(1) {
            let (tag, rest) = part.split_once('>').expect("a text tag");
            let body = unescape(rest.split("</text>").next().expect("a text body"));
            let number = |name: &str| -> f64 { attribute(tag, name).expect(name).parse().expect("a number") };
            let (x, y, size) = (number("x"), number("y"), number("font-size"));
            let width = text_px(&body, size);
            let (left, right) = match attribute(tag, "text-anchor") {
                Some("middle") => (x - width / 2.0, x + width / 2.0),
                Some("end") => (x - width, x),
                _ => (x, x + width),
            };
            found.push(TextBox { left, right, top: y - size, bottom: y, body });
        }
        found
    }

    /// No text extends past `width` or starts left of 0.
    pub(crate) fn assert_inside(content: &str, width: f64, what: &str) {
        for text in text_boxes(content) {
            assert!(
                text.left >= -0.01 && text.right <= width + 0.01,
                "{what}: {:?} spans {:.1} to {:.1} on a page {width} wide",
                text.body,
                text.left,
                text.right
            );
        }
    }

    /// No two texts overlap by more than half a pixel in each direction.
    pub(crate) fn assert_no_overlap(content: &str, what: &str) {
        let boxes = text_boxes(content);
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let overlap = a.left < b.right - 0.5
                    && b.left < a.right - 0.5
                    && a.top < b.bottom - 0.5
                    && b.top < a.bottom - 0.5;
                assert!(
                    !overlap,
                    "{what}: {:?} ({:.0}..{:.0}, {:.0}..{:.0}) overlaps {:?} ({:.0}..{:.0}, {:.0}..{:.0})",
                    a.body, a.left, a.right, a.top, a.bottom, b.body, b.left, b.right, b.top, b.bottom
                );
            }
        }
    }

    /// The text of every `<text>` element, joined by a space.
    pub(crate) fn plain(content: &str) -> String {
        text_boxes(content).into_iter().map(|t| t.body).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn the_numbers_have_three_significant_digits() {
        assert_eq!(sig3(1.25), "1.25");
        assert_eq!(sig3(937.5), "938");
        assert_eq!(sig3(-35.149), "-35.1");
        assert_eq!(sig3(0.02351), "0.0235");
        assert_eq!(sig3(12.0), "12");
        assert_eq!(hertz(1.25e9), "1.25 GHz");
        assert_eq!(hertz(937.5e6), "938 MHz");
        assert_eq!(one_decimal(-23.25), "-23.2");
        assert_eq!(one_decimal(-0.04), "0.0");
    }

    #[test]
    fn a_text_is_cut_to_its_width_and_the_boxes_are_found() {
        let cut = fit_text("a rather long label", 11.0, 60.0);
        assert!(cut.ends_with("...") && text_px(&cut, 11.0) <= 60.0, "{cut}");
        assert_eq!(fit_text("short", 11.0, 60.0), "short");
        let content = [
            text_el(10.0, 20.0, 10.0, INK, "a", "", false, "left & right"),
            text_el(100.0, 20.0, 10.0, INK, "b", "middle", false, "mid"),
            text_el(100.0, 40.0, 10.0, INK, "c", "end", true, "end"),
        ]
        .join("\n");
        let boxes = text_boxes(&content);
        assert_eq!(boxes.len(), 3);
        assert_eq!(boxes[0].body, "left & right");
        assert!(boxes[1].left < 100.0 && boxes[1].right > 100.0 && boxes[2].right == 100.0);
        assert_no_overlap(&content, "three");
        assert_inside(&content, 200.0, "three");
        let crowded = [
            text_el(10.0, 20.0, 10.0, INK, "a", "", false, "abcdef"),
            text_el(20.0, 24.0, 10.0, INK, "b", "", false, "abcdef"),
        ]
        .join("\n");
        assert!(std::panic::catch_unwind(|| assert_no_overlap(&crowded, "crowded")).is_err());
    }

    #[test]
    fn the_footer_says_it_once_and_fits() {
        for width in [900.0, 1400.0] {
            let picture = footer_picture(width);
            assert_inside(&picture.content, width, "footer");
            assert!(plain(&picture.content).contains("no limit applies"));
        }
        assert_eq!(footer_lines(1400.0, 12.0).len(), 1);
    }
}
