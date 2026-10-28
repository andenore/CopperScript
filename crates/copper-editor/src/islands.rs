//! `pcbir.editor.islands`: the exact deterministic island MST, by spatial
//! Boruvka nearest-cut edges.
//!
//! Bounding boxes and uniform-component subtree labels prune distance tests,
//! not candidate connectivity; the unique edge order reproduces the exhaustive
//! Kruskal result even for coincident terminals and tied lengths.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use copper_physical::Point;

use crate::{Result, value};

/// One terminal: (explicit root, identity, reference, pad, position).
#[derive(Clone, Debug)]
pub struct Land {
    pub root: String,
    pub identity: String,
    pub reference: String,
    pub pad: String,
    pub position: Point,
}

/// `edge`: the squared length and both lands, ordered by (root, identity).
#[derive(Clone, Debug)]
pub struct Edge {
    pub distance: i128,
    pub a: Land,
    pub b: Land,
}

impl Edge {
    /// The Python tuple prefix `edge[:5]`.
    fn key(&self) -> (i128, &str, &str, &str, &str) {
        (self.distance, &self.a.root, &self.b.root, &self.a.identity, &self.b.identity)
    }
}

fn edge(first: &Land, second: &Land) -> Edge {
    let (a, b) = if (&second.root, &second.identity) < (&first.root, &first.identity) {
        (second, first)
    } else {
        (first, second)
    };
    let dx = i128::from(a.position.x_nm - b.position.x_nm);
    let dy = i128::from(a.position.y_nm - b.position.y_nm);
    Edge { distance: dx * dx + dy * dy, a: a.clone(), b: b.clone() }
}

struct Node {
    bounds: (i64, i64, i64, i64),
    lands: Vec<usize>,
    children: Vec<Node>,
    component: Option<String>,
}

fn tree(lands: &[Land], indices: Vec<usize>) -> Node {
    let xs = indices.iter().map(|&i| lands[i].position.x_nm);
    let ys = indices.iter().map(|&i| lands[i].position.y_nm);
    let bounds = (
        xs.clone().min().expect("lands"),
        ys.clone().min().expect("lands"),
        xs.max().expect("lands"),
        ys.max().expect("lands"),
    );
    if indices.len() <= 8 {
        return Node { bounds, lands: indices, children: Vec::new(), component: None };
    }
    let axis_x = bounds.2 - bounds.0 >= bounds.3 - bounds.1;
    let mut ordered = indices;
    ordered.sort_by(|&a, &b| {
        let (la, lb) = (&lands[a], &lands[b]);
        let (ka, kb) = if axis_x { (la.position.x_nm, lb.position.x_nm) } else { (la.position.y_nm, lb.position.y_nm) };
        (ka, &la.root, &la.identity).cmp(&(kb, &lb.root, &lb.identity))
    });
    let upper = ordered.split_off(ordered.len() / 2);
    Node { bounds, lands: Vec::new(), children: vec![tree(lands, ordered), tree(lands, upper)], component: None }
}

struct Forest {
    parent: BTreeMap<String, String>,
}

impl Forest {
    fn find(&mut self, root: &str) -> String {
        let mut current = root.to_owned();
        loop {
            let parent = self.parent[&current].clone();
            if parent == current {
                return current;
            }
            let grandparent = self.parent[&parent].clone();
            self.parent.insert(current, grandparent.clone());
            current = grandparent;
        }
    }
}

fn label(node: &mut Node, lands: &[Land], forest: &mut Forest) -> Option<String> {
    let groups: Vec<Option<String>> = if node.children.is_empty() {
        node.lands.iter().map(|&i| Some(forest.find(&lands[i].root))).collect()
    } else {
        node.children.iter_mut().map(|child| label(child, lands, forest)).collect()
    };
    let first = &groups[0];
    node.component = if first.is_some() && groups.iter().all(|g| g == first) { first.clone() } else { None };
    node.component.clone()
}

fn lower_bound(node: &Node, point: Point) -> i128 {
    let (x0, y0, x1, y1) = node.bounds;
    let dx = i128::from((x0 - point.x_nm).max(0).max(point.x_nm - x1));
    let dy = i128::from((y0 - point.y_nm).max(0).max(point.y_nm - y1));
    dx * dx + dy * dy
}

fn visit(node: &Node, land: &Land, group: &str, lands: &[Land], forest: &mut Forest, best: &mut Option<Edge>) {
    if node.component.as_deref() == Some(group)
        || best.as_ref().is_some_and(|b| lower_bound(node, land.position) > b.distance)
    {
        return;
    }
    if node.children.is_empty() {
        for &index in &node.lands {
            let other = &lands[index];
            if forest.find(&other.root) == group {
                continue;
            }
            let candidate = edge(land, other);
            if best.as_ref().is_none_or(|b| candidate.key() < b.key()) {
                *best = Some(candidate);
            }
        }
    } else {
        let mut children: Vec<&Node> = node.children.iter().collect();
        children.sort_by_key(|child| lower_bound(child, land.position));
        for child in children {
            visit(child, land, group, lands, forest, best);
        }
    }
}

/// `nearest_island_tree`: the exhaustive complete-island-graph MST in stable edge order.
pub fn nearest_island_tree(lands: &[Land]) -> Result<Vec<Edge>> {
    let mut forest = Forest { parent: lands.iter().map(|l| (l.root.clone(), l.root.clone())).collect() };
    if forest.parent.len() <= 1 {
        return Ok(Vec::new());
    }
    let mut root = tree(lands, (0..lands.len()).collect());
    let mut result: Vec<Edge> = Vec::new();
    loop {
        let roots: Vec<String> = forest.parent.keys().cloned().collect();
        let mut groups: Vec<String> = roots.iter().map(|r| forest.find(r)).collect();
        groups.sort();
        groups.dedup();
        if groups.len() <= 1 {
            break;
        }
        label(&mut root, lands, &mut forest);
        let mut choices: Vec<(String, Edge)> = Vec::new();
        for land in lands {
            let group = forest.find(&land.root);
            let mut best = None;
            visit(&root, land, &group, lands, &mut forest, &mut best);
            let Some(candidate) = best else { continue };
            match choices.iter_mut().find(|(g, _)| *g == group) {
                None => choices.push((group, candidate)),
                Some((_, chosen)) => {
                    if candidate.key() < chosen.key() {
                        *chosen = candidate;
                    }
                }
            }
        }
        let mut candidates: Vec<Edge> = choices.into_iter().map(|(_, e)| e).collect();
        candidates.sort_by(|x, y| x.key().cmp(&y.key()));
        let mut added = 0;
        for candidate in candidates {
            let (a, b) = (forest.find(&candidate.a.root), forest.find(&candidate.b.root));
            if a != b {
                let (low, high) = match a.cmp(&b) {
                    Ordering::Less => (a, b),
                    _ => (b, a),
                };
                forest.parent.insert(high, low);
                result.push(candidate);
                added += 1;
            }
        }
        if added == 0 {
            return value("island MST could not connect the complete terminal graph");
        }
    }
    result.sort_by(|x, y| x.key().cmp(&y.key()));
    Ok(result)
}
