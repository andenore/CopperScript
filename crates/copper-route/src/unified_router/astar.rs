//! The grid A* search kernel (`pcbir/native/astar.c`, which the Python
//! router runs): same costs, same heap and the same tie-breaking.
//!
//! The C kernel's scratch buffers have caps and report an overflow, after
//! which Python grows them (or falls back to its own search of the same
//! semantics); the vectors here grow as needed, which is the same result.

pub const FLAG_NECK: u8 = 1;
pub const FLAG_NECK_MID: u8 = 2;
pub const FLAG_REGION: u8 = 4;
pub const FLAG_TARGET: u8 = 8;

const DX: [i64; 8] = [1, -1, 0, 0, 1, 1, -1, -1];
const DY: [i64; 8] = [0, 0, 1, -1, 1, -1, 1, -1];
use std::f64::consts::SQRT_2;

const STEP: [f64; 8] = [1.0, 1.0, 1.0, 1.0, SQRT_2, SQRT_2, SQRT_2, SQRT_2];
const INF: f64 = 1e30;

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: f64,
    g: f64,
    /// Negative: a completed target marker.
    node: i32,
}

fn entry_less(a: &Entry, b: &Entry) -> bool {
    if a.key != b.key {
        return a.key < b.key;
    }
    if a.g != b.g {
        return a.g < b.g;
    }
    a.node < b.node
}

fn heap_push(heap: &mut Vec<Entry>, value: Entry) {
    let mut i = heap.len();
    heap.push(value);
    while i > 0 {
        let parent = (i - 1) >> 1;
        if !entry_less(&value, &heap[parent]) {
            break;
        }
        heap[i] = heap[parent];
        i = parent;
    }
    heap[i] = value;
}

fn heap_pop(heap: &mut Vec<Entry>) -> Entry {
    let top = heap[0];
    let last = heap.pop().expect("non-empty heap");
    let n = heap.len();
    let mut i = 0;
    loop {
        let mut child = 2 * i + 1;
        if child >= n {
            break;
        }
        if child + 1 < n && entry_less(&heap[child + 1], &heap[child]) {
            child += 1;
        }
        if !entry_less(&heap[child], &last) {
            break;
        }
        heap[i] = heap[child];
        i = child;
    }
    if n > 0 {
        heap[i] = last;
    }
    top
}

fn octile(x: i64, y: i64, (tx0, ty0, tx1, ty1): (i64, i64, i64, i64)) -> f64 {
    let hx = if x < tx0 {
        tx0 - x
    } else if x > tx1 {
        x - tx1
    } else {
        0
    };
    let hy = if y < ty0 {
        ty0 - y
    } else if y > ty1 {
        y - ty1
    } else {
        0
    };
    if hx > hy { hx as f64 + 0.41421356 * hy as f64 } else { hy as f64 + 0.41421356 * hx as f64 }
}

/// Everything one search reads (the kernel's arguments on a uniform lattice).
pub struct SearchInput<'a> {
    pub nx: i64,
    pub ny: i64,
    pub nl: usize,
    pub owner: &'a [i32],
    pub mid_owner: &'a [i32],
    pub via_owner: &'a [i32],
    pub hist: &'a [f64],
    pub occ: &'a [i32],
    pub via_hist: &'a [f64],
    pub via_occ: &'a [i32],
    pub flags: &'a [u8],
    pub nid: i32,
    /// Layer indexes; bit 8 set: entered only on target nodes.
    pub layers: &'a [i32],
    /// `[layer][move]` planar move costs.
    pub move_base: &'a [f64],
    pub present: f64,
    pub via_cost: f64,
    pub bbox: (i64, i64, i64, i64),
    pub tbox: (i64, i64, i64, i64),
    pub sources: &'a [(i64, f64)],
    pub target_extra: Option<&'a [f64]>,
    pub turn: Option<&'a [f64]>,
    pub guide: Option<&'a [f64]>,
    /// `nl * nl` span masks (`None`: no technology for that transition);
    /// `None` altogether: through vias only.
    pub via_transitions: Option<&'a [Option<&'a [i32]>]>,
    pub max_expansions: i64,
}

/// Reusable search state (`best`, `parent`).
pub struct Scratch {
    pub best: Vec<f64>,
    pub parent: Vec<i32>,
    heap: Vec<Entry>,
    touched: Vec<usize>,
}

impl Scratch {
    pub fn new(size: usize) -> Scratch {
        Scratch { best: vec![INF; size], parent: vec![-1; size], heap: Vec::new(), touched: Vec::new() }
    }
}

/// `pcbir_astar` on a uniform lattice: the path (source first), or `None`
/// when no target was reached.
pub fn search(input: &SearchInput<'_>, scratch: &mut Scratch) -> Option<Vec<i64>> {
    let path = run(input, scratch);
    for &node in &scratch.touched {
        scratch.best[node] = INF;
    }
    scratch.touched.clear();
    scratch.heap.clear();
    path
}

fn run(input: &SearchInput<'_>, scratch: &mut Scratch) -> Option<Vec<i64>> {
    let nx = input.nx;
    let plane = nx * input.ny;
    let (bx0, by0, bx1, by1) = input.bbox;
    let Scratch { best, parent, heap, touched } = scratch;
    let mut allowed = [0u8; 64];
    for &layer in input.layers {
        allowed[(layer & 0xff) as usize] = if layer & 0x100 != 0 { 2 } else { 1 };
    }
    for &(node, source_cost) in input.sources {
        let (layer, rest) = (node / plane, node % plane);
        let index = node as usize;
        if allowed[layer as usize] == 0 || source_cost >= best[index] {
            continue;
        }
        if best[index] >= INF {
            touched.push(index);
        }
        best[index] = source_cost;
        parent[index] = -1;
        let key = source_cost + octile(rest % nx, rest / nx, input.tbox);
        heap_push(heap, Entry { key, g: source_cost, node: node as i32 });
    }
    let mut expansions = 0i64;
    let mut found: i64 = -1;
    while !heap.is_empty() {
        let entry = heap_pop(heap);
        let node = entry.node as i64;
        let cost = entry.g;
        if node < 0 {
            found = -node - 1;
            break;
        }
        let index = node as usize;
        if cost > best[index] {
            continue;
        }
        if input.flags[index] & FLAG_TARGET != 0 {
            let Some(extra) = input.target_extra else {
                found = node;
                break;
            };
            let total = cost + extra[index];
            heap_push(heap, Entry { key: total, g: total, node: (-node - 1) as i32 });
        }
        expansions += 1;
        if expansions > input.max_expansions {
            break;
        }
        let (layer, rest) = (node / plane, node % plane);
        let (y, x) = (rest / nx, rest % nx);
        let target_only = allowed[layer as usize] == 2;
        let here_in_region = input.flags[index] & FLAG_REGION != 0;
        let here_neck_only = here_in_region && input.flags[index] & FLAG_NECK != 0;
        let base = &input.move_base[layer as usize * 8..layer as usize * 8 + 8];
        let mut turn_row: Option<&[f64]> = None;
        if let Some(turn) = input.turn {
            let p = parent[index] as i64;
            if p >= 0 && p / plane == layer {
                let d = node - p;
                for m in 0..8 {
                    if d == DY[m] * nx + DX[m] {
                        turn_row = Some(&turn[8 * m..8 * m + 8]);
                        break;
                    }
                }
            }
        }
        for m in 0..8 {
            let (x2, y2) = (x + DX[m], y + DY[m]);
            if x2 < bx0 || x2 > bx1 || y2 < by0 || y2 > by1 {
                continue;
            }
            let dn = DY[m] * nx + DX[m];
            let n2 = (node + dn) as usize;
            if target_only && input.flags[n2] & FLAG_TARGET == 0 {
                continue;
            }
            if here_neck_only && input.flags[n2] & FLAG_REGION == 0 {
                continue;
            }
            let o = input.owner[n2];
            if o != -1 && o != input.nid && !(here_in_region && input.flags[n2] & FLAG_NECK != 0) {
                continue;
            }
            if DX[m] != 0 && DY[m] != 0 {
                let cell = (node + (if DY[m] > 0 { 0 } else { -1 }) * nx + if DX[m] > 0 { 0 } else { -1 }) as usize;
                let o = input.mid_owner[cell];
                if o != -1
                    && o != input.nid
                    && !(here_in_region && input.flags[n2] & FLAG_REGION != 0 && input.flags[cell] & FLAG_NECK_MID != 0)
                {
                    continue;
                }
            }
            let mut c = cost + base[m] * (1.0 + input.hist[n2]) * (1.0 + input.present * input.occ[n2] as f64);
            if let Some(row) = turn_row {
                c += row[m];
            }
            if let Some(guide) = input.guide {
                c += STEP[m] * guide[n2];
            }
            if c < best[n2] {
                if best[n2] >= INF {
                    touched.push(n2);
                }
                best[n2] = c;
                parent[n2] = node as i32;
                let key = c + octile(x2, y2, input.tbox);
                heap_push(heap, Entry { key, g: c, node: n2 as i32 });
            }
        }
        let h = octile(x, y, input.tbox);
        let vo = input.via_owner[rest as usize];
        if input.via_transitions.is_none() && vo != -1 && vo != input.nid {
            continue;
        }
        let vpen = input.via_cost
            * (1.0 + input.via_hist[rest as usize])
            * (1.0 + input.present * input.via_occ[rest as usize] as f64);
        for &entry in input.layers {
            let layer2 = (entry & 0xff) as i64;
            if layer2 == layer {
                continue;
            }
            if let Some(transitions) = input.via_transitions {
                match transitions[layer as usize * input.nl + layer2 as usize] {
                    Some(mask) if mask[rest as usize] == -1 || mask[rest as usize] == input.nid => {}
                    _ => continue,
                }
            }
            let n2 = (layer2 * plane + rest) as usize;
            if entry & 0x100 != 0 && input.flags[n2] & FLAG_TARGET == 0 {
                continue;
            }
            let o = input.owner[n2];
            if o != -1 && o != input.nid {
                continue;
            }
            let c = cost + vpen * (1.0 + input.hist[n2]) * (1.0 + input.present * input.occ[n2] as f64);
            if c < best[n2] {
                if best[n2] >= INF {
                    touched.push(n2);
                }
                best[n2] = c;
                parent[n2] = node as i32;
                heap_push(heap, Entry { key: c + h, g: c, node: n2 as i32 });
            }
        }
    }
    if found < 0 {
        return None;
    }
    let mut path = Vec::new();
    let mut node = found;
    while node != -1 {
        path.push(node);
        node = parent[node as usize] as i64;
    }
    path.reverse();
    Some(path)
}
