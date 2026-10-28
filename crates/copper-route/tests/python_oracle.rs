//! Search-lattice queries against fixtures recorded from Python by
//! `scripts/generate_rust_fixtures.py` (`lattice_fixture`).

use std::path::Path;

use copper_json::{Dumps, Value};
use copper_pcb::board::PhysicalBoard;
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_physical::Point;
use copper_route::fine_regions::{FinePatch, FineRegionPlan};
use copper_route::routing_lattice::{Lattice, NestedLattice, UniformLattice};
use num_traits::ToPrimitive;

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn ints(values: &[i64]) -> Value {
    Value::Array(values.iter().map(|v| int(*v)).collect())
}

fn float(value: f64) -> Value {
    Value::Float(value)
}

fn list(values: Vec<Value>) -> Value {
    Value::Array(values)
}

fn option(value: Option<i64>) -> Value {
    value.map_or(Value::Null, int)
}

fn pairs(values: &[(i64, f64)]) -> Value {
    list(values.iter().map(|(n, s)| list(vec![int(*n), float(*s)])).collect())
}

fn sample(values: &[Value]) -> Value {
    let mut object = copper_json::Object::new();
    object.insert("length".to_owned(), int(values.len() as i64));
    object.insert("every_97th".to_owned(), list(values.iter().step_by(97).cloned().collect()));
    Value::Object(object)
}

fn get_i64(value: &Value, key: &str) -> i64 {
    match value.get(key) {
        Some(Value::Int(number)) => number.to_i64().expect("i64"),
        other => panic!("{key}: {other:?}"),
    }
}

fn get_f64(value: &Value, key: &str) -> f64 {
    match value.get(key) {
        Some(Value::Float(number)) => *number,
        Some(Value::Int(number)) => number.to_f64().expect("f64"),
        other => panic!("{key}: {other:?}"),
    }
}

fn board() -> PhysicalBoard {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let design = copper_lang::compiler::load_design(&root.join("examples/valid_board/board.copper"), false, false)
        .expect("board");
    let options = PhysicalOptions::new(100.0, 80.0, 4, "generic").expect("options");
    physicalize(&design, Footprints::Proxy, &options, false, false).expect("physical board")
}

fn patch(x0: i64, y0: i64, x1: i64, y1: i64, factor: i64, reference: &str) -> FinePatch {
    FinePatch {
        x0_nm: x0,
        y0_nm: y0,
        x1_nm: x1,
        y1_nm: y1,
        factor,
        references: vec![reference.to_owned()],
        reason: "test".to_owned(),
        layers: None,
        waste_nodes: 0,
    }
}

/// The lattice-specific queries shared by both lattice kinds.
trait Queries: Lattice {
    fn snap(&self, layer: i64, point: Point) -> Option<i64>;
    fn decode(&self, node: i64) -> (i64, i64, i64, i64);
    fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64>;
    fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64>;
    fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)>;
    fn delta(&self, a: i64, b: i64) -> (i64, i64);
    fn diagonal_cell(&self, a: i64, b: i64) -> i64;
    fn box_of(&self, nodes: &[i64]) -> (i64, i64, i64, i64);
    fn grow_box(&self, index_box: (i64, i64, i64, i64), margin: i64) -> (i64, i64, i64, i64);
    fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)>;
}

macro_rules! queries {
    ($lattice:ty) => {
        impl Queries for $lattice {
            fn snap(&self, layer: i64, point: Point) -> Option<i64> {
                <$lattice>::snap(self, layer, point)
            }
            fn decode(&self, node: i64) -> (i64, i64, i64, i64) {
                <$lattice>::decode(self, node)
            }
            fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
                <$lattice>::offset_nodes(self, node, offsets)
            }
            fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64> {
                <$lattice>::columns_around(self, point, offsets)
            }
            fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)> {
                <$lattice>::cover(self, min_x, min_y, max_x, max_y)
            }
            fn delta(&self, a: i64, b: i64) -> (i64, i64) {
                <$lattice>::delta(self, a, b)
            }
            fn diagonal_cell(&self, a: i64, b: i64) -> i64 {
                <$lattice>::diagonal_cell(self, a, b)
            }
            fn box_of(&self, nodes: &[i64]) -> (i64, i64, i64, i64) {
                <$lattice>::box_of(self, nodes).expect("nodes")
            }
            fn grow_box(&self, index_box: (i64, i64, i64, i64), margin: i64) -> (i64, i64, i64, i64) {
                <$lattice>::grow_box(self, index_box, margin)
            }
            fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)> {
                <$lattice>::segment_nodes(self, layer, start, end, radius_nm)
            }
        }
    };
}

queries!(UniformLattice);
queries!(NestedLattice);

fn tuple4(value: (i64, i64, i64, i64)) -> Value {
    ints(&[value.0, value.1, value.2, value.3])
}

/// Every recorded field of one query, recomputed and compared.
/// `previous` is the preceding point query's (layer, point, radius): a pair query's segment starts there.
fn replay(
    lattice: &dyn Queries,
    query: &Value,
    previous: Option<(i64, Point, f64)>,
    failures: &mut Vec<String>,
    case: &str,
) {
    let offsets = [(0, 0), (1, 0), (-2, 1), (3, -3), (0, 5)];
    let mut fields: Vec<(&str, Value)> = Vec::new();
    match query.get("op").and_then(Value::as_str) {
        Some("point") => {
            let (layer, x, y) = (get_i64(query, "layer"), get_i64(query, "x"), get_i64(query, "y"));
            let radius = get_f64(query, "radius");
            let point = Point { x_nm: x, y_nm: y };
            let near = lattice.nearest(layer, point);
            let near_point = lattice.point(near);
            let decoded = lattice.decode(near);
            let (xf, yf) = (x as f64, y as f64);
            fields.extend([
                ("node_at", option(lattice.node_at(layer, point))),
                ("snap", option(lattice.snap(layer, point))),
                ("nearest", int(near)),
                ("nearest_point", ints(&[near_point.x_nm, near_point.y_nm])),
                ("step", int(lattice.step_nm(near))),
                ("decode", tuple4(decoded)),
                ("neighbours", pairs(&lattice.neighbours(near, true))),
                ("orthogonal", pairs(&lattice.neighbours(near, false))),
                ("offset_nodes", ints(&lattice.offset_nodes(near, &offsets))),
                ("columns_around", ints(&lattice.columns_around(point, &offsets))),
                ("nodes_in", ints(&lattice.nodes_in(layer, xf - radius, yf - radius, xf + radius, yf + radius))),
                (
                    "cover",
                    list(
                        lattice
                            .cover(xf - radius, yf - 2.0 * radius, xf + radius, yf)
                            .into_iter()
                            .map(|(c, x, y)| ints(&[c, x, y]))
                            .collect(),
                    ),
                ),
            ]);
        }
        Some("pair") => {
            let (a, b) = (get_i64(query, "a"), get_i64(query, "b"));
            let delta = lattice.delta(a, b);
            let index_box = lattice.box_of(&[a, b]);
            fields.extend([
                ("delta", ints(&[delta.0, delta.1])),
                ("diagonal_cell", int(lattice.diagonal_cell(a, b))),
                ("box_of", tuple4(index_box)),
                ("grow_box", tuple4(lattice.grow_box(index_box, 3))),
            ]);
            if let Some((layer, start, radius)) = previous {
                fields.push(("segment", pairs(&lattice.segment_nodes(layer, start, lattice.point(b), radius + 1.5))));
            }
        }
        _ => return,
    }
    for (key, actual) in fields {
        let expected = query.get(key).unwrap_or_else(|| panic!("{key} in {case}"));
        let (expected, actual) =
            (Dumps::new().sort_keys(true).dumps(expected), Dumps::new().sort_keys(true).dumps(&actual));
        if expected != actual {
            failures.push(format!("{case} {key}: Python {expected}\n  Rust {actual}"));
        }
    }
}

#[test]
fn lattice_queries_match_python() {
    let document = copper_json::parse(include_str!("fixtures/lattice.json")).expect("fixture is JSON");
    let board = board();
    let pitch = 250_000;
    let plan = FineRegionPlan {
        base_pitch_nm: pitch,
        patches: vec![
            patch(10_000_000, 10_000_000, 12_000_000, 13_000_000, 2, "U1"),
            patch(30_000_000, 20_000_000, 31_000_000, 20_750_000, 5, "U2"),
        ],
        base_nodes: 0,
        fine_nodes: 0,
        uniform_nodes_at_finest: 0,
        layer_count: 0,
        skipped: Vec::new(),
    };
    let uniform = UniformLattice::new(&board, pitch, None).expect("uniform");
    let window =
        UniformLattice::new(&board, 333_333, Some((5_000_000, 7_000_000, 41_000_000, 33_000_000))).expect("window");
    let nested = NestedLattice::new(&board, pitch, &plan, None).expect("nested");
    let mut failures = Vec::new();
    for case in document.get("cases").and_then(Value::as_array).expect("cases") {
        let name = case.get("name").and_then(Value::as_str).expect("name");
        let lattice: &dyn Queries = match name {
            "uniform" => &uniform,
            "window" => &window,
            _ => &nested,
        };
        let summary = case.get("summary").expect("summary");
        let mut expected: Vec<(&str, Value)> =
            vec![("size", int(lattice.size())), ("plane", int(lattice.plane())), ("pitch_nm", int(lattice.pitch_nm()))];
        let moves = |moves: &[(i64, i64, i64, i64, f64)]| {
            list(moves.iter().map(|m| list(vec![int(m.0), int(m.1), int(m.2), int(m.3), float(m.4)])).collect())
        };
        match name {
            "nested" => {
                let (base, patches) = nested.signature();
                let mut signature = vec![int(base.0), int(base.1), int(base.2), int(base.3), int(base.4), int(base.5)];
                signature.push(list(patches.iter().map(|p| ints(&[p.0, p.1, p.2, p.3, p.4])).collect()));
                let values = |items: &[i64]| items.iter().map(|v| int(*v)).collect::<Vec<_>>();
                let tables = nested.search_tables();
                expected.extend([
                    ("signature", list(signature)),
                    ("full_box", tuple4(nested.full_box())),
                    ("moves", moves(nested.move_deltas())),
                    ("node_count", int(nested.node_count())),
                    ("shadowed", sample(&values(&nested.shadowed))),
                    ("shadowed_cells", sample(&values(&nested.shadowed_cells))),
                    ("portal_pairs", list(vec![ints(&nested.portal_pairs.0), ints(&nested.portal_pairs.1)])),
                    ("block_moves", list((0..nested.blocks.len()).map(|i| moves(nested.block_moves(i))).collect())),
                    (
                        "search_tables",
                        list(vec![
                            sample(&tables.0.iter().map(|v| float(*v)).collect::<Vec<_>>()),
                            sample(&tables.1.iter().map(|v| float(*v)).collect::<Vec<_>>()),
                            sample(&tables.2.iter().map(|v| int(*v as i64)).collect::<Vec<_>>()),
                            sample(&values(&tables.3)),
                        ]),
                    ),
                ]);
            }
            _ => {
                let uniform = if name == "uniform" { &uniform } else { &window };
                let s = uniform.signature();
                expected.extend([
                    ("signature", ints(&[s.0, s.1, s.2, s.3, s.4, s.5])),
                    ("full_box", tuple4(uniform.full_box())),
                    ("moves", moves(uniform.move_deltas())),
                ]);
            }
        }
        for (key, actual) in expected {
            let (python, rust) = (
                Dumps::new().sort_keys(true).dumps(summary.get(key).expect(key)),
                Dumps::new().sort_keys(true).dumps(&actual),
            );
            if python != rust {
                failures.push(format!("{name} summary {key}: Python {python}\n  Rust {rust}"));
            }
        }
        let mut previous = None;
        for query in case.get("queries").and_then(Value::as_array).expect("queries") {
            match query.get("op").and_then(Value::as_str) {
                Some("point") => {
                    replay(lattice, query, None, &mut failures, name);
                    let point = Point { x_nm: get_i64(query, "x"), y_nm: get_i64(query, "y") };
                    previous = Some((get_i64(query, "layer"), point, get_f64(query, "radius")));
                }
                Some("pair") => replay(lattice, query, previous, &mut failures, name),
                Some("nested") => {
                    let (x, y) = (get_i64(query, "x"), get_i64(query, "y"));
                    let point = Point { x_nm: x, y_nm: y };
                    let end = Point { x_nm: x + 1_234_567, y_nm: y - 765_432 };
                    let local = nested.segment_nodes_local(1, point, end, 0.5, 10.0);
                    let fields = [
                        ("within", ints(&nested.columns_within(point, 700_000.0))),
                        (
                            "local",
                            list(local.iter().map(|(n, d, w)| list(vec![int(*n), float(*d), float(*w)])).collect()),
                        ),
                    ];
                    for (key, actual) in fields {
                        let python = Dumps::new().sort_keys(true).dumps(query.get(key).expect(key));
                        let rust = Dumps::new().sort_keys(true).dumps(&actual);
                        if python != rust {
                            failures.push(format!("nested {key}: Python {python}\n  Rust {rust}"));
                        }
                    }
                }
                other => panic!("unknown query {other:?}"),
            }
        }
    }
    let shown: Vec<&String> = failures.iter().take(12).collect();
    assert!(failures.is_empty(), "{} lattice queries differ:\n{shown:#?}", failures.len());
}
