//! Python's `difflib.unified_diff` over `str.splitlines(keepends=True)`
//! lines: `SequenceMatcher` with no junk function and its automatic
//! popularity heuristic, grouped with three lines of context.

use indexmap::IndexMap as HashMap;

/// `str.splitlines(keepends=True)`: Python's universal line boundaries.
pub fn splitlines_keepends(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let end = match ch {
            '\r' => match chars.peek() {
                Some((_, '\n')) => {
                    chars.next();
                    index + 2
                }
                _ => index + 1,
            },
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                index + ch.len_utf8()
            }
            _ => continue,
        };
        lines.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

type Opcode = (&'static str, usize, usize, usize, usize);

struct Matcher<'a> {
    a: &'a [&'a str],
    b: &'a [&'a str],
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    fn new(a: &'a [&'a str], b: &'a [&'a str]) -> Self {
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, line) in b.iter().enumerate() {
            b2j.entry(line).or_default().push(i);
        }
        // `autojunk`: drop elements more popular than 1% of a long `b`.
        if b.len() >= 200 {
            let ntest = b.len() / 100 + 1;
            b2j.retain(|_, indices| indices.len() <= ntest);
        }
        Matcher { a, b, b2j }
    }

    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, line) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut next: HashMap<usize, usize> = HashMap::new();
            if let Some(indices) = self.b2j.get(line) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    next.insert(j, k);
                    if k > bestsize {
                        (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                    }
                }
            }
            j2len = next;
        }
        // No element is junk, so popular elements extend a match on both sides.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut merged = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    merged.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            merged.push((i1, j1, k1));
        }
        merged.push((la, lb, 0));
        merged
    }

    fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut answer = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(("equal", 0, 1, 0, 1));
        }
        if let Some(first) = codes.first_mut()
            && first.0 == "equal"
        {
            let (tag, i1, i2, j1, j2) = *first;
            *first = (tag, i1.max(i2.saturating_sub(n)), i2, j1.max(j2.saturating_sub(n)), j2);
        }
        if let Some(last) = codes.last_mut()
            && last.0 == "equal"
        {
            let (tag, i1, i2, j1, j2) = *last;
            *last = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > n + n {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                (i1, j1) = (i1.max(i2.saturating_sub(n)), j1.max(j2.saturating_sub(n)));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }
}

fn range(start: usize, stop: usize) -> String {
    let length = stop - start;
    match length {
        1 => format!("{}", start + 1),
        0 => format!("{start},0"),
        _ => format!("{},{length}", start + 1),
    }
}

/// `"".join(unified_diff(a, b, fromfile, tofile))` with `n=3`.
pub fn unified_diff(a: &[&str], b: &[&str], fromfile: &str, tofile: &str) -> String {
    let matcher = Matcher::new(a, b);
    let mut out = String::new();
    for (index, group) in matcher.grouped_opcodes(3).iter().enumerate() {
        if index == 0 {
            out.push_str(&format!("--- {fromfile}\n+++ {tofile}\n"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push_str(&format!("@@ -{} +{} @@\n", range(first.1, last.2), range(first.3, last.4)));
        for &(tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                for line in &a[i1..i2] {
                    out.push(' ');
                    out.push_str(line);
                }
                continue;
            }
            if tag == "replace" || tag == "delete" {
                for line in &a[i1..i2] {
                    out.push('-');
                    out.push_str(line);
                }
            }
            if tag == "replace" || tag == "insert" {
                for line in &b[j1..j2] {
                    out.push('+');
                    out.push_str(line);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python() {
        // difflib.unified_diff(a, b, "current.copper", "proposed.copper")
        let a = splitlines_keepends("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        let b = splitlines_keepends("a\nb\nX\nd\ne\nf\ng\nh\ni\nY\nj");
        assert_eq!(
            unified_diff(&a, &b, "current.copper", "proposed.copper"),
            "--- current.copper\n+++ proposed.copper\n@@ -1,10 +1,11 @@\n a\n b\n-c\n+X\n d\n e\n f\n g\n h\n i\n-j\n+Y\n+j"
        );
        let a = splitlines_keepends("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n");
        let b = splitlines_keepends("0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n");
        assert_eq!(
            unified_diff(&a, &b, "x", "y"),
            "--- x\n+++ y\n@@ -1,3 +1,4 @@\n+0\n 1\n 2\n 3\n@@ -9,4 +10,3 @@\n 9\n 10\n 11\n-12\n"
        );
        assert_eq!(splitlines_keepends("a\r\nb\rc\u{2028}d"), vec!["a\r\n", "b\r", "c\u{2028}", "d"]);
    }
}
