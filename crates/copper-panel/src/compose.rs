//! Compose routed KiCad boards into one panel board: every placed item is
//! moved by its board's pose, nets and footprint references get the board's
//! instance prefix (so boards stay electrically separate and references stay
//! unique), UUIDs are derived per instance, zone fills are dropped (the panel
//! is refilled), and each board's `Edge.Cuts` outline is taken out for the
//! frame to replace. Anything the composer does not know how to place fails.

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::sexpr::{Node, quote, unquote};

pub type Point = (i64, i64);

/// Millimetre text -> nanometres.
pub fn nm(text: &str) -> Result<i64, String> {
    copper_cam::board::parse_nm(text).ok_or_else(|| format!("not a coordinate: {text}"))
}

/// Nanometres -> KiCad millimetre text.
pub fn mm(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let abs = value.unsigned_abs();
    let whole = abs / 1_000_000;
    let frac = abs % 1_000_000;
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        let digits = format!("{frac:06}");
        format!("{sign}{whole}.{}", digits.trim_end_matches('0'))
    }
}

/// A board's placement on the panel: counter-clockwise quarter turns (as
/// seen in KiCad, y down) about the board origin, then a translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pose {
    pub quarter_turns: u8,
    pub offset: Point,
}

impl Pose {
    pub fn rotate(&self, (x, y): Point) -> Point {
        let mut p = (x, y);
        for _ in 0..self.quarter_turns % 4 {
            p = (p.1, -p.0);
        }
        p
    }

    pub fn apply(&self, p: Point) -> Point {
        let (x, y) = self.rotate(p);
        (x + self.offset.0, y + self.offset.1)
    }

    /// The board point that [`Pose::apply`] maps to `q`.
    pub fn invert(&self, q: Point) -> Point {
        let back = Pose { quarter_turns: (4 - self.quarter_turns % 4) % 4, offset: (0, 0) };
        back.rotate((q.0 - self.offset.0, q.1 - self.offset.1))
    }

    pub fn degrees(&self) -> f64 {
        90.0 * f64::from(self.quarter_turns % 4)
    }
}

fn angle_text(degrees: f64) -> String {
    let mut a = degrees % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    if (a - a.round()).abs() < 1e-9 {
        format!("{}", a.round() as i64)
    } else {
        let text = format!("{a:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// A routed board read for composition.
pub struct BoardInput {
    pub instance: String,
    pub tree: Node,
}

/// What composition produced, before the frame is added.
pub struct Composition {
    /// `(kicad_pcb ...)` with the header, the net table and every board's items.
    pub tree: Node,
    /// Per board, its transformed outline rings (from `Edge.Cuts`).
    pub outlines: IndexMap<String, Vec<Vec<Point>>>,
    /// Per board, its pose.
    pub poses: IndexMap<String, Pose>,
}

const HEADER: [&str; 6] = ["version", "generator", "generator_version", "general", "paper", "layers"];
const PLACED: [&str; 10] =
    ["footprint", "segment", "arc", "via", "zone", "gr_line", "gr_arc", "gr_circle", "gr_rect", "gr_poly"];

fn derived_uuid(instance: &str, old: &str) -> String {
    let digest = Sha256::digest(format!("copper-panel:{instance}:{old}").as_bytes());
    let h: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The board's outline rings from its `Edge.Cuts` graphics (lines chained
/// into rings, rectangles and polygons), in its own coordinates.
pub fn outline_rings(tree: &Node, source: &str) -> Result<Vec<Vec<Point>>, String> {
    let mut segments: Vec<(Point, Point)> = Vec::new();
    let mut rings: Vec<Vec<Point>> = Vec::new();
    for item in tree.items() {
        if item.child("layer").and_then(|l| l.string_value(1)).as_deref() != Some("Edge.Cuts") {
            continue;
        }
        let point = |tag: &str| -> Result<Point, String> {
            let n = item.child(tag).ok_or_else(|| format!("{source}: outline item without {tag}"))?;
            Ok((nm(n.value(1).unwrap_or(""))?, nm(n.value(2).unwrap_or(""))?))
        };
        match item.tag() {
            Some("gr_line") => segments.push((point("start")?, point("end")?)),
            Some("gr_rect") => {
                let (a, b) = (point("start")?, point("end")?);
                rings.push(vec![a, (b.0, a.1), b, (a.0, b.1)]);
            }
            Some("gr_poly") => {
                let pts = item.child("pts").ok_or_else(|| format!("{source}: outline polygon without points"))?;
                let ring = pts
                    .children("xy")
                    .map(|xy| Ok((nm(xy.value(1).unwrap_or(""))?, nm(xy.value(2).unwrap_or(""))?)))
                    .collect::<Result<Vec<_>, String>>()?;
                rings.push(ring);
            }
            Some(other) => {
                return Err(format!(
                    "{source}: outline {other} is not supported yet (lines, rectangles and polygons are)"
                ));
            }
            None => {}
        }
    }
    // Chain line segments into closed rings.
    while let Some((start, mut end)) = segments.pop() {
        let mut ring = vec![start];
        while end != start {
            ring.push(end);
            let next = segments.iter().position(|(a, b)| *a == end || *b == end).ok_or_else(|| {
                format!("{source}: the board outline is not closed at ({}, {})", mm(end.0), mm(end.1))
            })?;
            let (a, b) = segments.swap_remove(next);
            end = if a == end { b } else { a };
        }
        rings.push(ring);
    }
    if rings.is_empty() {
        return Err(format!("{source}: no board outline on Edge.Cuts"));
    }
    Ok(rings)
}

/// The pose that rotates a board and puts its outline's bounding-box
/// minimum at `at`.
pub fn pose_for(rings: &[Vec<Point>], quarter_turns: u8, at: Point) -> Pose {
    let rotation = Pose { quarter_turns, offset: (0, 0) };
    let (mut min_x, mut min_y) = (i64::MAX, i64::MAX);
    for p in rings.iter().flatten() {
        let (x, y) = rotation.rotate(*p);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
    }
    Pose { quarter_turns, offset: (at.0 - min_x, at.1 - min_y) }
}

/// Rewrites one board's items into panel coordinates and names.
struct Placer<'a> {
    instance: &'a str,
    pose: Pose,
    nets: &'a IndexMap<String, usize>,
    old_nets: &'a IndexMap<String, String>,
    source: &'a str,
}

impl Placer<'_> {
    fn net_name(&self, name: &str) -> String {
        if name.is_empty() { String::new() } else { format!("{}/{name}", self.instance) }
    }

    fn move_point(&self, node: &mut Node) -> Result<(), String> {
        let items = node.items_mut().ok_or("not a list")?;
        if items.len() < 3 {
            return Err(format!("{}: point without coordinates", self.source));
        }
        let x = nm(items[1].atom_text().unwrap_or(""))?;
        let y = nm(items[2].atom_text().unwrap_or(""))?;
        let (x, y) = self.pose.apply((x, y));
        items[1] = Node::atom(mm(x));
        items[2] = Node::atom(mm(y));
        Ok(())
    }

    /// Add the pose's rotation to an `(at x y [angle])` that holds an
    /// absolute angle (pads and texts inside footprints, footprint poses).
    fn turn(&self, at: &mut Node) {
        if self.pose.quarter_turns.is_multiple_of(4) {
            return;
        }
        let Some(items) = at.items_mut() else { return };
        let angle = items.get(3).and_then(Node::atom_text).and_then(|t| t.parse::<f64>().ok()).unwrap_or(0.0);
        let text = angle_text(angle + self.pose.degrees());
        if items.len() > 3 {
            items[3] = Node::atom(text);
        } else {
            items.push(Node::atom(text));
        }
    }

    /// `(net N ["name"])` -> the panel's number and prefixed name.
    fn renet(&self, node: &mut Node) -> Result<(), String> {
        let items = node.items_mut().ok_or("not a list")?;
        let old = items.get(1).and_then(Node::atom_text).unwrap_or("0").to_owned();
        let name = if let Some(name) = items.get(2).and_then(Node::atom_text) {
            unquote(name)
        } else {
            self.old_nets.get(&old).cloned().ok_or_else(|| format!("{}: unknown net number {old}", self.source))?
        };
        let panel_name = self.net_name(&name);
        let number = self.nets.get(&panel_name).copied().unwrap_or(0);
        items.truncate(1);
        items.push(Node::atom(number.to_string()));
        Ok(())
    }

    fn uuids(&self, node: &mut Node) {
        let instance = self.instance.to_owned();
        node.walk_mut(&mut |n| {
            if matches!(n.tag(), Some("uuid" | "tstamp"))
                && let Some(items) = n.items_mut()
                && let Some(old) = items.get(1).and_then(Node::atom_text)
            {
                let fresh = derived_uuid(&instance, &unquote(old));
                items[1] = Node::atom(quote(&fresh));
            }
        });
    }

    fn footprint(&self, node: &mut Node) -> Result<(), String> {
        let items = node.items_mut().ok_or("not a list")?;
        for item in items.iter_mut() {
            match item.tag() {
                Some("at") => {
                    self.move_point(item)?;
                    self.turn(item);
                }
                Some("property") => {
                    let key = item.string_value(1).unwrap_or_default();
                    if key == "Reference" || key == "CopperScriptPath" {
                        let value = item.string_value(2).unwrap_or_default();
                        if let Some(list) = item.items_mut() {
                            list[2] = Node::string(&format!("{}-{value}", self.instance));
                        }
                    }
                    if let Some(at) = item.child_mut("at") {
                        self.turn(at);
                    }
                }
                Some("fp_text") => {
                    if item.value(1) == Some("reference") {
                        let value = item.string_value(2).unwrap_or_default();
                        if let Some(list) = item.items_mut() {
                            list[2] = Node::string(&format!("{}-{value}", self.instance));
                        }
                    }
                    if let Some(at) = item.child_mut("at") {
                        self.turn(at);
                    }
                }
                Some("pad") => {
                    if let Some(at) = item.child_mut("at") {
                        self.turn(at);
                    }
                    if let Some(net) = item.child_mut("net") {
                        let name = net.string_value(2).unwrap_or_default();
                        let panel = self.net_name(&name);
                        let number = self.nets.get(&panel).copied().unwrap_or(0);
                        *net =
                            Node::list(vec![Node::atom("net"), Node::atom(number.to_string()), Node::string(&panel)]);
                    }
                }
                Some("zone") => {
                    return Err(format!("{}: zones inside footprints are not supported in panels yet", self.source));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn geometry(&self, node: &mut Node) -> Result<(), String> {
        let mut result = Ok(());
        node.walk_mut(&mut |n| {
            if result.is_err() {
                return;
            }
            match n.tag() {
                Some("start" | "end" | "mid" | "center" | "xy" | "at") if n.items().len() >= 3 => {
                    result = self.move_point(n);
                }
                _ => {}
            }
        });
        result
    }

    fn place(&self, mut item: Node) -> Result<Option<Node>, String> {
        let tag = item.tag().unwrap_or("").to_owned();
        match tag.as_str() {
            "footprint" => self.footprint(&mut item)?,
            "zone" => {
                if let Some(list) = item.items_mut() {
                    list.retain(|n| !matches!(n.tag(), Some("filled_polygon" | "fill_segments")));
                }
                let net_name = item.child("net_name").and_then(|n| n.string_value(1)).unwrap_or_default();
                let panel = self.net_name(&net_name);
                let number = self.nets.get(&panel).copied().unwrap_or(0);
                if let Some(net) = item.child_mut("net") {
                    *net = Node::tagged("net", &[&number.to_string()]);
                }
                if let Some(name) = item.child_mut("net_name") {
                    *name = Node::list(vec![Node::atom("net_name"), Node::string(&panel)]);
                }
                // Every polygon (a zone may have several, holes included).
                self.geometry(&mut item)?;
            }
            "segment" | "arc" | "via" => {
                self.geometry(&mut item)?;
                if let Some(net) = item.child_mut("net") {
                    self.renet(net)?;
                }
            }
            "gr_text" => {
                if let Some(at) = item.child_mut("at") {
                    self.move_point(at)?;
                    self.turn(at);
                }
            }
            _ if tag.starts_with("gr_") => {
                if item.child("layer").and_then(|l| l.string_value(1)).as_deref() == Some("Edge.Cuts") {
                    return Ok(None);
                }
                self.geometry(&mut item)?;
                if let Some(net) = item.child_mut("net") {
                    self.renet(net)?;
                }
            }
            _ => return Err(format!("{}: top-level {tag} cannot be placed on a panel yet", self.source)),
        }
        self.uuids(&mut item);
        Ok(Some(item))
    }
}

/// Compose `boards`, each at its pose.
pub fn compose(boards: &[BoardInput], poses: &[Pose]) -> Result<Composition, String> {
    let first = boards.first().ok_or("a panel needs at least one board")?;
    let header: Vec<&Node> = HEADER.iter().filter_map(|tag| first.tree.child(tag)).collect();
    for board in &boards[1..] {
        for tag in ["layers", "setup"] {
            if board.tree.child(tag) != first.tree.child(tag) {
                return Err(format!(
                    "board {} has a different {tag} from board {}; a panel needs one stack-up and layer set",
                    board.instance, first.instance
                ));
            }
        }
    }
    // The panel's net table: every board's nets, prefixed, in board order.
    let mut nets: IndexMap<String, usize> = IndexMap::new();
    nets.insert(String::new(), 0);
    let mut old_tables: Vec<IndexMap<String, String>> = Vec::new();
    for board in boards {
        let mut table = IndexMap::new();
        for net in board.tree.children("net") {
            let number = net.value(1).unwrap_or("0").to_owned();
            let name = net.string_value(2).unwrap_or_default();
            if !name.is_empty() {
                let panel = format!("{}/{name}", board.instance);
                let next = nets.len();
                nets.entry(panel).or_insert(next);
            }
            table.insert(number, name);
        }
        old_tables.push(table);
    }
    let mut items: Vec<Node> = vec![Node::atom("kicad_pcb")];
    items.extend(header.into_iter().cloned());
    if let Some(setup) = first.tree.child("setup") {
        items.push(setup.clone());
    }
    for (name, number) in &nets {
        items.push(Node::list(vec![Node::atom("net"), Node::atom(number.to_string()), Node::string(name)]));
    }
    let mut outlines = IndexMap::new();
    let mut pose_map = IndexMap::new();
    for ((board, pose), old_nets) in boards.iter().zip(poses).zip(&old_tables) {
        let source = format!("board {}", board.instance);
        let rings = outline_rings(&board.tree, &source)?;
        outlines
            .insert(board.instance.clone(), rings.iter().map(|r| r.iter().map(|p| pose.apply(*p)).collect()).collect());
        pose_map.insert(board.instance.clone(), *pose);
        let placer = Placer { instance: &board.instance, pose: *pose, nets: &nets, old_nets, source: &source };
        for item in board.tree.items().iter().skip(1) {
            let tag = item.tag().unwrap_or("");
            if HEADER.contains(&tag) || tag == "setup" || tag == "net" || tag == "title_block" {
                continue;
            }
            if !PLACED.contains(&tag) && tag != "gr_text" && !tag.starts_with("gr_") {
                return Err(format!("{source}: top-level {tag} cannot be placed on a panel yet"));
            }
            if let Some(placed) = placer.place(item.clone())? {
                items.push(placed);
            }
        }
    }
    Ok(Composition { tree: Node::List(items), outlines, poses: pose_map })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::{parse, print};

    pub(crate) const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "copperscript")
  (general (thickness 1.6)) (paper "A4")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (setup (pad_to_mask_clearance 0))
  (net 0 "") (net 1 "SIG") (net 2 "GND")
  (footprint "lib:R" (layer "F.Cu") (uuid "11111111-1111-1111-1111-111111111111") (at 5 3 0)
    (property "Reference" "R1" (at 0 -1 0) (layer "F.Fab"))
    (pad "1" smd rect (at -0.5 0) (size 0.5 0.5) (layers "F.Cu") (net 1 "SIG"))
    (pad "2" smd rect (at 0.5 0) (size 0.5 0.5) (layers "F.Cu") (net 2 "GND")))
  (segment (start 1 1) (end 4.5 3) (width 0.25) (layer "F.Cu") (net 1) (uuid "22222222-2222-2222-2222-222222222222"))
  (via (at 8 4) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "33333333-3333-3333-3333-333333333333"))
  (zone (net 2) (net_name "GND") (layers "F.Cu") (uuid "44444444-4444-4444-4444-444444444444")
    (polygon (pts (xy 0 0) (xy 10 0) (xy 10 6) (xy 0 6)))
    (filled_polygon (layer "F.Cu") (pts (xy 0 0) (xy 1 0) (xy 1 1))))
  (gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "a"))
  (gr_line (start 10 0) (end 10 6) (layer "Edge.Cuts") (uuid "b"))
  (gr_line (start 10 6) (end 0 6) (layer "Edge.Cuts") (uuid "c"))
  (gr_line (start 0 6) (end 0 0) (layer "Edge.Cuts") (uuid "d")))"#;

    fn input(instance: &str) -> BoardInput {
        BoardInput { instance: instance.to_owned(), tree: parse(BOARD, instance).expect("parsed") }
    }

    #[test]
    fn coordinates_round_trip_through_millimetre_text() {
        for v in [0, 1, -1, 1_500_000, -2_250_000, 40_000_000, 6_650_000, 123_456] {
            assert_eq!(nm(&mm(v)).expect("parsed"), v);
        }
    }

    #[test]
    fn quarter_turns_rotate_counter_clockwise_on_screen() {
        let pose = Pose { quarter_turns: 1, offset: (0, 0) };
        assert_eq!(pose.rotate((1, 0)), (0, -1));
        assert_eq!(Pose { quarter_turns: 2, offset: (5, 5) }.apply((1, 2)), (4, 3));
    }

    #[test]
    fn two_boards_compose_with_separate_nets_and_unique_references() {
        let boards = [input("A"), input("B")];
        let rings = outline_rings(&boards[0].tree, "A").expect("outline");
        let poses = [pose_for(&rings, 0, (0, 0)), pose_for(&rings, 1, (12_000_000, 0))];
        assert_eq!(poses[1].offset, (12_000_000, 10_000_000));
        let composed = compose(&boards, &poses).expect("composed");
        let tree = parse(&print(&composed.tree), "panel").expect("reprinted panel parses");
        let nets: Vec<String> = tree.children("net").filter_map(|n| n.string_value(2)).collect();
        assert_eq!(nets, ["", "A/SIG", "A/GND", "B/SIG", "B/GND"]);
        let refs: Vec<String> = tree
            .children("footprint")
            .filter_map(|f| f.children("property").find(|p| p.string_value(1).as_deref() == Some("Reference")))
            .filter_map(|p| p.string_value(2))
            .collect();
        assert_eq!(refs, ["A-R1", "B-R1"]);
        // Board B is turned a quarter: its footprint at (5, 3) lands at (12 + 3, 10 - 5).
        let b = tree.children("footprint").nth(1).expect("board B's footprint");
        assert_eq!(
            b.child("at").map(|a| a.items().iter().skip(1).filter_map(Node::atom_text).collect::<Vec<_>>()),
            Some(vec!["15", "5", "90"])
        );
        let pad = b.children("pad").next().expect("a pad");
        assert_eq!(pad.child("at").and_then(|a| a.value(3)), Some("90"));
        assert_eq!(pad.child("net").and_then(|n| n.string_value(2)).as_deref(), Some("B/SIG"));
        // Fills and outlines are gone; zones keep their prefixed nets.
        assert!(tree.children("zone").all(|z| z.child("filled_polygon").is_none()));
        assert!(
            tree.items()
                .iter()
                .all(|i| i.child("layer").and_then(|l| l.string_value(1)).as_deref() != Some("Edge.Cuts"))
        );
        let zone = tree.children("zone").nth(1).expect("board B's zone");
        assert_eq!(zone.child("net_name").and_then(|n| n.string_value(1)).as_deref(), Some("B/GND"));
        assert_eq!(zone.child("net").and_then(|n| n.value(1)), Some("4"));
        // UUIDs differ between the instances of one board.
        let uuids: Vec<String> =
            tree.children("segment").filter_map(|s| s.child("uuid").and_then(|u| u.string_value(1))).collect();
        assert_ne!(uuids[0], uuids[1]);
        assert_eq!(composed.outlines["B"][0].len(), 4);
        // Reading the panel with the native CAM model works.
        let board =
            copper_cam::board::parse_board(&print(&composed.tree), "panel").expect("copper-cam reads the panel");
        assert_eq!((board.footprints.len(), board.tracks.len(), board.vias.len()), (2, 2, 2));
    }

    #[test]
    fn every_polygon_of_a_zone_moves_with_its_board() {
        let two = BOARD.replacen(
            "(polygon (pts (xy 0 0) (xy 10 0) (xy 10 6) (xy 0 6)))",
            "(polygon (pts (xy 2 2) (xy 3 2) (xy 3 3) (xy 2 3))) (polygon (pts (xy 8 4) (xy 9 4) (xy 9 5) (xy 8 5)))",
            1,
        );
        let board = BoardInput { instance: "A".to_owned(), tree: parse(&two, "A").expect("parsed") };
        let rings = outline_rings(&board.tree, "A").expect("outline");
        let composed = compose(&[board], &[pose_for(&rings, 0, (30_000_000, 10_000_000))]).expect("composed");
        let zone = composed.tree.children("zone").next().expect("the zone");
        let firsts: Vec<(String, String)> = zone
            .children("polygon")
            .filter_map(|p| p.child("pts").and_then(|pts| pts.children("xy").next().cloned()))
            .map(|xy| (xy.value(1).unwrap_or("").to_owned(), xy.value(2).unwrap_or("").to_owned()))
            .collect();
        assert_eq!(firsts, [("32".to_owned(), "12".to_owned()), ("38".to_owned(), "14".to_owned())]);
    }

    #[test]
    fn different_stackups_are_refused() {
        let mut other = input("B");
        let text = BOARD.replace("(pad_to_mask_clearance 0)", "(pad_to_mask_clearance 0.05)");
        other.tree = parse(&text, "B").expect("parsed");
        let boards = [input("A"), other];
        let pose = Pose { quarter_turns: 0, offset: (0, 0) };
        let error = compose(&boards, &[pose, pose]).err().expect("refused");
        assert!(error.contains("different setup"), "{error}");
    }
}
