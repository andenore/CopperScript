//! What the text of an EMI figure takes up, read from its SVG: where each `<text>` element starts and ends (the same
//! estimate the figures lay themselves out with: half an em of advance per character, a font size high above the
//! baseline), so that the tests can say that no text leaves its page and no two texts on one page overlap.
//!
//! A document stacks its pages as `<g transform="translate(0,Y)">` groups of page-local coordinates; each page is
//! checked on its own.

#![allow(dead_code)]

/// One text of a page and the box it is estimated to take up.
#[derive(Clone, Debug)]
pub struct TextBox {
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

/// The pages of a document, each as the SVG text of its group.
pub fn pages(svg: &str) -> Vec<&str> {
    svg.split("<g transform=\"translate(0,").skip(1).collect()
}

/// `page` without the nested thumbnails (`<svg class="thumbnail" ...>...</svg>`): their texts are in their own
/// coordinates and are checked on their own by [`thumbnails`].
pub fn without_thumbnails(page: &str) -> String {
    let mut out = String::new();
    let mut rest = page;
    while let Some(at) = rest.find("<svg class=\"thumbnail\"") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        rest = rest.split_once("</svg>").map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}

/// The nested thumbnails of `svg`: each one's width (its viewBox) and its content.
pub fn thumbnails(svg: &str) -> Vec<(f64, String)> {
    svg.split("<svg class=\"thumbnail\"")
        .skip(1)
        .map(|part| {
            let (tag, rest) = part.split_once('>').expect("a thumbnail tag");
            let view = tag.split("viewBox=\"0 0 ").nth(1).expect("a viewBox").split('"').next().expect("a value");
            let width = view.split(' ').next().expect("a width").parse().expect("a number");
            (width, rest.split("</svg>").next().unwrap_or("").to_owned())
        })
        .collect()
}

/// Every `<text>` element of `page`.
pub fn text_boxes(page: &str) -> Vec<TextBox> {
    let mut found = Vec::new();
    for part in page.split("<text ").skip(1) {
        let (tag, rest) = part.split_once('>').expect("a text tag");
        let tag = format!(" {tag}");
        let tag = tag.as_str();
        let body = unescape(rest.split("</text>").next().expect("a text body"));
        let number = |name: &str| -> f64 { attribute(tag, name).expect(name).parse().expect("a number") };
        let (x, y, size) = (number("x"), number("y"), number("font-size"));
        let width = body.chars().count() as f64 * size * 0.56;
        let (left, right) = match attribute(tag, "text-anchor") {
            Some("middle") => (x - width / 2.0, x + width / 2.0),
            Some("end") => (x - width, x),
            _ => (x, x + width),
        };
        found.push(TextBox { left, right, top: y - size, bottom: y, body });
    }
    found
}

/// No text of any page of `svg` ends right of `width` or starts left of 0.
pub fn assert_inside(svg: &str, width: f64) {
    for page in pages(svg) {
        for text in text_boxes(&without_thumbnails(page)) {
            assert!(
                text.left >= -0.01 && text.right <= width + 0.01,
                "{:?} spans {:.1} to {:.1} on a page {width} wide",
                text.body,
                text.left,
                text.right
            );
        }
    }
}

/// No two texts of one page of `svg` overlap by more than half a pixel in each direction.
pub fn assert_no_overlap(svg: &str) {
    for page in pages(svg) {
        let boxes = text_boxes(&without_thumbnails(page));
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let overlap = a.left < b.right - 0.5
                    && b.left < a.right - 0.5
                    && a.top < b.bottom - 0.5
                    && b.top < a.bottom - 0.5;
                assert!(!overlap, "{:?} overlaps {:?}", a, b);
            }
        }
    }
}

/// The rules of the simple figures, for the page of a document and for each thumbnail in it: nothing outside its page
/// (a thumbnail: its own width), no two texts overlapping, no text of more than 120 characters (the footer line apart),
/// at most four legend entries on a figure, at most `max_texts` `<text>` elements in the whole of `svg`.
pub fn assert_simple(svg: &str, width: f64, max_texts: usize) {
    assert_inside(svg, width);
    assert_no_overlap(svg);
    for (thumb_width, content) in thumbnails(svg) {
        assert_inside_one(&content, thumb_width);
        let boxes = text_boxes(&content);
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let overlap = a.left < b.right - 0.5
                    && b.left < a.right - 0.5
                    && a.top < b.bottom - 0.5
                    && b.top < a.bottom - 0.5;
                assert!(!overlap, "in a thumbnail: {:?} overlaps {:?}", a, b);
            }
        }
    }
    for text in text_boxes(svg) {
        assert!(
            text.body.chars().count() <= 120 || text.body.starts_with("Screening "),
            "{:?} is longer than 120 characters",
            text.body
        );
    }
    for page in pages(svg) {
        assert!(page.matches("class=\"legend-entry\"").count() <= 4, "more than four legend entries");
    }
    let count = text_count(svg);
    assert!(count <= max_texts, "{count} texts, at most {max_texts}");
}

/// No text of `content` ends right of `width` or starts left of 0.
fn assert_inside_one(content: &str, width: f64) {
    for text in text_boxes(content) {
        assert!(
            text.left >= -0.01 && text.right <= width + 0.01,
            "{:?} spans {:.1} to {:.1} on a page {width} wide",
            text.body,
            text.left,
            text.right
        );
    }
}

/// The text of every `<text>` element of `svg`, joined by a space.
pub fn plain(svg: &str) -> String {
    text_boxes(svg).into_iter().map(|t| t.body).collect::<Vec<_>>().join(" ")
}

/// The number of `<text>` elements.
pub fn text_count(svg: &str) -> usize {
    svg.matches("<text ").count()
}

/// The width of the document, px.
pub fn width(svg: &str) -> f64 {
    svg.split("width=\"").nth(1).expect("a width").split('"').next().expect("a value").parse().expect("a number")
}

/// The height of the document, px.
pub fn height(svg: &str) -> f64 {
    svg.split("height=\"").nth(1).expect("a height").split('"').next().expect("a value").parse().expect("a number")
}

/// The words "limit" and "margin" occur only in the footer's denial ("no limit applies"), and no limit line or margin
/// value is drawn.
pub fn assert_limit_and_margin_only_in_the_footer(svg: &str) {
    let text = plain(svg);
    for word in ["limit", "margin"] {
        for (at, _) in text.match_indices(word) {
            let around = &text[at.saturating_sub(3)..(at + 13).min(text.len())];
            assert_eq!(around, "no limit applies", "{word} outside the footer's denial: ...{around}");
        }
    }
    assert!(!svg.contains("class=\"limit"), "no limit line");
    assert!(!svg.contains("margin_db"), "no margin value");
}
