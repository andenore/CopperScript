//! "Did you mean" suggestions: Python's `difflib.get_close_matches(word,
//! candidates, n=1, cutoff=0.6)`, ported from CPython's `SequenceMatcher`.

use std::collections::BTreeMap;

/// The closest candidate scoring at least 0.6, as Python picks it: highest
/// ratio, ties broken by the larger candidate string.
pub fn close_match(word: &str, candidates: &[&str]) -> Option<String> {
    close_matches(word, candidates, 1, 0.6).into_iter().next()
}

pub fn close_matches(word: &str, candidates: &[&str], count: usize, cutoff: f64) -> Vec<String> {
    let b: Vec<char> = word.chars().collect();
    let matcher = Matcher::new(&b);
    let mut scored: Vec<(f64, String)> = Vec::new();
    for candidate in candidates {
        let a: Vec<char> = candidate.chars().collect();
        if matcher.real_quick_ratio(&a) >= cutoff && matcher.quick_ratio(&a) >= cutoff {
            let ratio = matcher.ratio(&a);
            if ratio >= cutoff {
                scored.push((ratio, (*candidate).to_owned()));
            }
        }
    }
    // heapq.nlargest(n, (score, x)): descending by score, then by string.
    scored.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    scored.into_iter().take(count).map(|(_, candidate)| candidate).collect()
}

fn calculate_ratio(matches: usize, length: usize) -> f64 {
    if length == 0 { 1.0 } else { 2.0 * matches as f64 / length as f64 }
}

/// `SequenceMatcher` with a fixed second sequence `b` and no junk.
struct Matcher<'a> {
    b: &'a [char],
    /// Positions of each character in `b`, ascending; popular characters
    /// are dropped when `b` has 200 or more items (`autojunk`).
    b2j: BTreeMap<char, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    fn new(b: &'a [char]) -> Self {
        let mut b2j: BTreeMap<char, Vec<usize>> = BTreeMap::new();
        for (index, character) in b.iter().enumerate() {
            b2j.entry(*character).or_default().push(index);
        }
        if b.len() >= 200 {
            let limit = b.len() / 100 + 1;
            b2j.retain(|_, indices| indices.len() <= limit);
        }
        Matcher { b, b2j }
    }

    fn real_quick_ratio(&self, a: &[char]) -> f64 {
        calculate_ratio(a.len().min(self.b.len()), a.len() + self.b.len())
    }

    fn quick_ratio(&self, a: &[char]) -> f64 {
        let mut available: BTreeMap<char, isize> = BTreeMap::new();
        for character in self.b {
            *available.entry(*character).or_default() += 1;
        }
        let mut matches = 0;
        for character in a {
            let count = available.entry(*character).or_default();
            if *count > 0 {
                matches += 1;
            }
            *count -= 1;
        }
        calculate_ratio(matches, a.len() + self.b.len())
    }

    fn ratio(&self, a: &[char]) -> f64 {
        let matches: usize = self.matching_blocks(a).iter().map(|(_, _, size)| size).sum();
        calculate_ratio(matches, a.len() + self.b.len())
    }

    fn matching_blocks(&self, a: &[char]) -> Vec<(usize, usize, usize)> {
        let mut queue = vec![(0, a.len(), 0, self.b.len())];
        let mut blocks = Vec::new();
        while let Some((a_low, a_high, b_low, b_high)) = queue.pop() {
            let (i, j, size) = self.longest_match(a, a_low, a_high, b_low, b_high);
            if size > 0 {
                blocks.push((i, j, size));
                if a_low < i && b_low < j {
                    queue.push((a_low, i, b_low, j));
                }
                if i + size < a_high && j + size < b_high {
                    queue.push((i + size, a_high, j + size, b_high));
                }
            }
        }
        blocks.sort();
        blocks
    }

    /// `find_longest_match`: the earliest longest block, extended over equal
    /// popular characters as CPython does (there is no junk).
    fn longest_match(
        &self,
        a: &[char],
        a_low: usize,
        a_high: usize,
        b_low: usize,
        b_high: usize,
    ) -> (usize, usize, usize) {
        let b = self.b;
        let (mut best_i, mut best_j, mut best_size) = (a_low, b_low, 0usize);
        let mut lengths: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, character) in a.iter().enumerate().take(a_high).skip(a_low) {
            let mut next: BTreeMap<usize, usize> = BTreeMap::new();
            if let Some(indices) = self.b2j.get(character) {
                for &j in indices {
                    if j < b_low {
                        continue;
                    }
                    if j >= b_high {
                        break;
                    }
                    let length = j.checked_sub(1).and_then(|previous| lengths.get(&previous)).copied().unwrap_or(0) + 1;
                    next.insert(j, length);
                    if length > best_size {
                        best_i = i + 1 - length;
                        best_j = j + 1 - length;
                        best_size = length;
                    }
                }
            }
            lengths = next;
        }
        // There is no junk, so only popular characters (absent from b2j) can
        // extend the block; CPython's junk-extension loops never run.
        while best_i > a_low && best_j > b_low && a[best_i - 1] == b[best_j - 1] {
            best_i -= 1;
            best_j -= 1;
            best_size += 1;
        }
        while best_i + best_size < a_high
            && best_j + best_size < b_high
            && a[best_i + best_size] == b[best_j + best_size]
        {
            best_size += 1;
        }
        (best_i, best_j, best_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_difflib() {
        let keywords = ["component", "port", "module", "net", "no_connect", "supply", "interface", "constraint"];
        assert_eq!(close_match("componnet", &keywords).as_deref(), Some("component"));
        assert_eq!(close_match("nett", &keywords).as_deref(), Some("net"));
        assert_eq!(close_match("zzz", &keywords), None);
        // Equal ratios: Python keeps the larger string.
        assert_eq!(close_matches("ab", &["ac", "ad"], 1, 0.5), ["ad"]);
    }
}
