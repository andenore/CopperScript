//! The panel's frame: rails, tabs that bridge each board's gap to whatever
//! lies across it (a rail or another board), mouse-bite holes along the tab
//! ends on boards, fiducials and tooling holes on the rails, and the panel
//! outline (the union of boards, tabs and rails) on `Edge.Cuts`.

use copper_cam::poly;
use copper_lang::panel::{PanelSpec, PanelTab, Side, TabEdge};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::compose::{Point, Pose, mm};
use crate::sexpr::{Node, quote};

/// How far a tab may reach across its gap before it is an error.
pub const MAX_TAB_LENGTH: i64 = 20_000_000;

/// A fiducial's mask opening radius: 1 mm copper with a 0.5 mm mask margin.
pub const FIDUCIAL_OPENING_RADIUS: i64 = 1_000_000;

/// Mouse-bite holes along one end of a tab `width` wide.
pub fn holes_per_tab(width: i64, mouse_bite: &copper_lang::panel::MouseBite) -> i64 {
    ((width - mouse_bite.drill) / mouse_bite.pitch + 1).max(1)
}

/// Fiducial centres' distance from the panel's outer edge on the top and
/// bottom rails: `fiducial_offset`, or the rail's centre.
pub fn fiducial_offsets(spec: &PanelSpec) -> [i64; 2] {
    [spec.fiducial_offset.unwrap_or(spec.rails.top / 2), spec.fiducial_offset.unwrap_or(spec.rails.bottom / 2)]
}

/// A tab as built.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltTab {
    pub name: String,
    pub board: String,
    pub side: Side,
    /// Distance along the side from its start, and the tab width.
    pub at: i64,
    pub width: i64,
    /// The board it bridges to, or `None` for a rail.
    pub across: Option<String>,
    /// The gap it spans.
    pub length: i64,
    pub mouse_bites: usize,
    /// The panel position of every mouse-bite hole centre.
    pub bites: Vec<Point>,
    /// The tab's board-edge segment on the panel.
    pub edge: (Point, Point),
}

pub struct Frame {
    /// `Edge.Cuts` polygons and the frame footprints (mouse bites, fiducials, tooling holes).
    pub items: Vec<Node>,
    pub tabs: Vec<BuiltTab>,
    /// Panel outline bounds `(x0, y0, x1, y1)`.
    pub bounds: (i64, i64, i64, i64),
    /// The rail rectangles.
    pub rails: Vec<[i64; 4]>,
}

type Rect = [i64; 4];

fn rect_shapes(r: Rect) -> poly::Shapes {
    poly::polygon(&[
        (r[0] as f64, r[1] as f64),
        (r[2] as f64, r[1] as f64),
        (r[2] as f64, r[3] as f64),
        (r[0] as f64, r[3] as f64),
    ])
}

fn ring_shapes(ring: &[Point]) -> poly::Shapes {
    poly::polygon(&ring.iter().map(|(x, y)| (*x as f64, *y as f64)).collect::<Vec<_>>())
}

fn bounds_of<'a>(points: impl Iterator<Item = &'a Point>) -> Rect {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for (x, y) in points {
        b = [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)];
    }
    b
}

fn uuid(key: &str) -> String {
    let digest = Sha256::digest(format!("copper-panel-frame:{key}").as_bytes());
    let h: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn property(key: &str, value: &str, uuid_key: &str) -> Node {
    crate::sexpr::parse(
        &format!(
            "(property {} {} (at 0 0 0) (layer \"F.Fab\") (hide yes) (uuid {}) (effects (font (size 1 1) (thickness 0.15))))",
            quote(key),
            quote(value),
            quote(&uuid(uuid_key))
        ),
        "frame",
    )
    .expect("a valid property")
}

/// A board-only footprint of frame features at `at`, its pads given in local coordinates.
fn footprint(library_id: &str, reference: &str, value: &str, at: Point, attributes: &str, pads: Vec<String>) -> Node {
    let mut text = format!(
        "(footprint {} (layer \"F.Cu\") (uuid {}) (at {} {} 0) (attr {attributes})",
        quote(library_id),
        quote(&uuid(reference)),
        mm(at.0),
        mm(at.1)
    );
    for pad in pads {
        text.push(' ');
        text.push_str(&pad);
    }
    text.push(')');
    let mut node = crate::sexpr::parse(&text, "frame").expect("a valid frame footprint");
    if let Some(items) = node.items_mut() {
        items.insert(4, property("Reference", reference, &format!("{reference}:ref")));
        items.insert(5, property("Value", value, &format!("{reference}:value")));
    }
    node
}

fn npth(at: Point, drill: i64) -> String {
    format!(
        "(pad \"\" np_thru_hole circle (at {} {}) (size {} {}) (drill {}) (layers \"*.Cu\" \"*.Mask\"))",
        mm(at.0),
        mm(at.1),
        mm(drill),
        mm(drill),
        mm(drill)
    )
}

/// The side of a rectangular board outline in the board's own frame:
/// its start, unit direction along it, outward normal and length.
fn side_geometry(local: Rect, side: Side) -> (Point, Point, Point, i64) {
    let [x0, y0, x1, y1] = local;
    match side {
        Side::Top => ((x0, y0), (1, 0), (0, -1), x1 - x0),
        Side::Bottom => ((x0, y1), (1, 0), (0, 1), x1 - x0),
        Side::Left => ((x0, y0), (0, 1), (-1, 0), y1 - y0),
        Side::Right => ((x1, y0), (0, 1), (1, 0), y1 - y0),
    }
}

/// Build the frame. `local` holds each board's outline rings in its own
/// coordinates, `outlines` the same rings on the panel.
pub fn build(
    spec: &PanelSpec,
    local: &IndexMap<String, Vec<Vec<Point>>>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    poses: &IndexMap<String, Pose>,
) -> Result<Frame, String> {
    // Rails around the boards' overall bounds, `spacing` away.
    let all = bounds_of(outlines.values().flatten().flatten());
    let s = spec.spacing;
    let reach = |rail: i64| if rail > 0 { s + rail } else { 0 };
    let outer = [
        all[0] - reach(spec.rails.left),
        all[1] - reach(spec.rails.top),
        all[2] + reach(spec.rails.right),
        all[3] + reach(spec.rails.bottom),
    ];
    let mut rails: Vec<Rect> = Vec::new();
    if spec.rails.top > 0 {
        rails.push([outer[0], outer[1], outer[2], all[1] - s]);
    }
    if spec.rails.bottom > 0 {
        rails.push([outer[0], all[3] + s, outer[2], outer[3]]);
    }
    if spec.rails.left > 0 {
        rails.push([outer[0], outer[1], all[0] - s, outer[3]]);
    }
    if spec.rails.right > 0 {
        rails.push([all[2] + s, outer[1], outer[2], outer[3]]);
    }
    let mut shapes: poly::Shapes = Vec::new();
    for rings in outlines.values() {
        for ring in rings {
            shapes.extend(ring_shapes(ring));
        }
    }
    for rail in &rails {
        shapes.extend(rect_shapes(*rail));
    }
    let mut items: Vec<Node> = Vec::new();
    let mut tabs: Vec<BuiltTab> = Vec::new();
    let mut footprints: Vec<(String, Rect)> = Vec::new();
    for tab in &spec.tabs {
        let built = build_tab(spec, tab, local, outlines, poses, &rails)?;
        // One hole as wide as the tab would cut it through.
        if built.width <= spec.mouse_bite.drill {
            return Err(format!(
                "tab {} is {} mm wide, no wider than its {} mm mouse-bite holes: they would cut it through",
                built.name,
                mm(built.width),
                mm(spec.mouse_bite.drill)
            ));
        }
        // Tabs may not share material: the same bridge declared from both
        // boards, or two tabs on one side, would drill the bites twice.
        let area = tab_rect(&built, poses[&built.board]);
        for (other, rect) in &footprints {
            if area[0] < rect[2] && rect[0] < area[2] && area[1] < rect[3] && rect[1] < area[3] {
                return Err(format!(
                    "tabs {other} and {} overlap (board {}'s {} side); declare each bridge once",
                    built.name,
                    built.board,
                    built.side.as_str()
                ));
            }
            // Apart, tabs leave the router the panel spacing between them:
            // a narrower slot cannot be milled.
            let dx = (rect[0] - area[2]).max(area[0] - rect[2]).max(0);
            let dy = (rect[1] - area[3]).max(area[1] - rect[3]).max(0);
            let gap = (dx as f64).hypot(dy as f64);
            if gap < spec.spacing as f64 {
                return Err(format!(
                    "tabs {other} and {} are {} mm apart, closer than the panel spacing ({} mm): the router cannot \
                     mill between them; make them one tab or move one",
                    built.name,
                    mm(gap.round() as i64),
                    mm(spec.spacing)
                ));
            }
        }
        footprints.push((built.name.clone(), area));
        // A tab may meet only its own board and what it bridges to: any other
        // board keeps the milled gap from it, or the outline would fuse them.
        let ring = vec![(area[0], area[1]), (area[2], area[1]), (area[2], area[3]), (area[0], area[3])];
        for (name, rings) in outlines {
            if *name == built.board || built.across.as_deref() == Some(name.as_str()) {
                continue;
            }
            let gap = crate::intent::ring_distance(std::slice::from_ref(&ring), rings);
            if gap < spec.spacing {
                return Err(format!(
                    "tab {} passes board {name} {} mm away, closer than the panel spacing ({} mm)",
                    built.name,
                    mm(gap),
                    mm(spec.spacing)
                ));
            }
        }
        let (a, b) = built.edge;
        let pose = poses[&built.board];
        let normal = pose.rotate(side_geometry([0, 0, 0, 0], built.side).2);
        // The tab rectangle, a micron into both sides so the union joins them.
        let far = built.length + 1_000;
        let corners = [
            (a.0 - normal.0 * 1_000, a.1 - normal.1 * 1_000),
            (b.0 - normal.0 * 1_000, b.1 - normal.1 * 1_000),
            (b.0 + normal.0 * far, b.1 + normal.1 * far),
            (a.0 + normal.0 * far, a.1 + normal.1 * far),
        ];
        shapes.extend(ring_shapes(&corners));
        // Mouse bites `inset` inside each board the tab touches.
        let width = built.width;
        let count = holes_per_tab(width, &spec.mouse_bite);
        let span = (count - 1) * spec.mouse_bite.pitch;
        let direction = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let centre = ((a.0 + b.0) / 2, (a.1 + b.1) / 2);
        let mut pads = Vec::new();
        let mut bites = Vec::new();
        let mut ends = vec![(-spec.mouse_bite.inset, built.board.as_str())];
        if let Some(across) = &built.across {
            ends.push((built.length + spec.mouse_bite.inset, across.as_str()));
        }
        let tab_material = ring_shapes(&corners);
        for (depth, owner) in ends {
            // Every hole perforates its board: it lies on the board and the
            // tab, not in a slot or past a thin flange of the board.
            let mut material: poly::Shapes = outlines[owner].iter().flat_map(|ring| ring_shapes(ring)).collect();
            material.extend(tab_material.iter().cloned());
            let material = poly::union(&material);
            for k in 0..count {
                let along = -span / 2 + k * spec.mouse_bite.pitch;
                let p = (direction.0 * along + normal.0 * depth, direction.1 * along + normal.1 * depth);
                let hole = poly::circle(
                    copper_cam::geometry::Point::new(centre.0 + p.0, centre.1 + p.1),
                    spec.mouse_bite.drill as f64 / 2.0,
                    250.0,
                );
                let astray: f64 = poly::difference(&hole, &material).iter().map(|s| poly::shape_area(s)).sum();
                if astray > 1e6 {
                    return Err(format!(
                        "tab {}: its mouse bites {} mm inside board {owner} fall off the board (a slot or a flange \
                         narrower than the inset); reduce mouse_bite.inset or move the tab",
                        built.name,
                        mm(spec.mouse_bite.inset)
                    ));
                }
                pads.push(npth(p, spec.mouse_bite.drill));
                bites.push((centre.0 + p.0, centre.1 + p.1));
            }
        }
        items.push(footprint(
            "copper-panel:MouseBites",
            &format!("TAB-{}", built.name),
            "mouse bites",
            centre,
            "board_only exclude_from_pos_files exclude_from_bom",
            pads,
        ));
        tabs.push(BuiltTab { mouse_bites: bites.len(), bites, ..built });
    }
    // Fiducials and tooling holes on the top and bottom rails.
    let rail_centre = |top: bool| if top { (outer[1] + all[1] - s) / 2 } else { (all[3] + s + outer[3]) / 2 };
    if (spec.fiducials > 0 || spec.tooling_hole.is_some())
        && (spec.rails.top < 3_000_000 || spec.rails.bottom < 3_000_000)
    {
        return Err("fiducials and tooling holes need top and bottom rails of at least 3 mm".to_owned());
    }
    let [top_offset, bottom_offset] = fiducial_offsets(spec);
    if spec.fiducials > 0
        && let Some(offset) = spec.fiducial_offset
        && (offset < FIDUCIAL_OPENING_RADIUS
            || offset + FIDUCIAL_OPENING_RADIUS > spec.rails.top.min(spec.rails.bottom))
    {
        return Err(format!(
            "fiducial_offset {} mm puts the fiducials' {} mm mask openings off the {} mm and {} mm rails",
            mm(offset),
            mm(2 * FIDUCIAL_OPENING_RADIUS),
            mm(spec.rails.top),
            mm(spec.rails.bottom)
        ));
    }
    let fiducial_spots = [
        (outer[0] + 8_000_000, outer[1] + top_offset),
        (outer[2] - 8_000_000, outer[1] + top_offset),
        (outer[0] + 8_000_000, outer[3] - bottom_offset),
        (outer[2] - 8_000_000, outer[3] - bottom_offset),
    ];
    let tooling_spots = [
        (outer[0] + 3_000_000, rail_centre(true)),
        (outer[2] - 3_000_000, rail_centre(true)),
        (outer[0] + 3_000_000, rail_centre(false)),
        (outer[2] - 3_000_000, rail_centre(false)),
    ];
    // Every fiducial (by its mask opening) and tooling hole lies on the panel
    // and keeps 0.2 mm from the others: a narrow panel cannot hold them.
    let mut features: Vec<(String, Point, i64)> = fiducial_spots
        .iter()
        .take(spec.fiducials as usize)
        .enumerate()
        .map(|(k, at)| (format!("FID{}", k + 1), *at, FIDUCIAL_OPENING_RADIUS))
        .collect();
    if let Some(drill) = spec.tooling_hole {
        features.extend(tooling_spots.iter().enumerate().map(|(k, at)| (format!("TH{}", k + 1), *at, drill / 2)));
    }
    // A tooling hole keeps 0.2 mm of rail on both sides, or it notches the rail open.
    if let Some(drill) = spec.tooling_hole {
        let narrowest = spec.rails.top.min(spec.rails.bottom);
        if drill / 2 + 200_000 > narrowest / 2 {
            return Err(format!(
                "tooling holes of {} mm need rails of at least {} mm (0.2 mm of rail either side); the rails are {} mm",
                mm(drill),
                mm(drill + 400_000),
                mm(narrowest)
            ));
        }
    }
    for (k, (name, at, r)) in features.iter().enumerate() {
        if at.0 - r < outer[0] || at.0 + r > outer[2] {
            return Err(format!(
                "the panel is {} mm wide: {name} would leave it; widen the panel or drop fiducials and tooling holes",
                mm(outer[2] - outer[0])
            ));
        }
        for (other, there, q) in &features[k + 1..] {
            let distance = ((at.0 - there.0) as f64).hypot((at.1 - there.1) as f64);
            if distance < (r + q + 200_000) as f64 {
                return Err(format!(
                    "the panel is {} mm wide: {name} and {other} would overlap; widen the panel or drop fiducials \
                     and tooling holes",
                    mm(outer[2] - outer[0])
                ));
            }
        }
    }
    for (k, at) in fiducial_spots.iter().take(spec.fiducials as usize).enumerate() {
        let pad = "(pad \"1\" smd circle (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Mask\") (solder_mask_margin 0.5))"
            .to_owned();
        items.push(footprint(
            "copper-panel:Fiducial",
            &format!("FID{}", k + 1),
            "fiducial",
            *at,
            "smd exclude_from_bom",
            vec![pad],
        ));
    }
    if let Some(drill) = spec.tooling_hole {
        for (k, at) in tooling_spots.iter().enumerate() {
            items.push(footprint(
                "copper-panel:ToolingHole",
                &format!("TH{}", k + 1),
                "tooling hole",
                *at,
                "board_only exclude_from_pos_files exclude_from_bom",
                vec![npth((0, 0), drill)],
            ));
        }
    }
    // The panel must hold together: boards, tabs and rails one piece.
    let united = poly::union(&shapes);
    if united.len() > 1 {
        let mut pieces: Vec<String> = Vec::new();
        for piece in &united {
            let piece = vec![piece.clone()];
            let mut members: Vec<String> = Vec::new();
            for (name, rings) in outlines {
                let board: poly::Shapes = rings.iter().flat_map(|ring| ring_shapes(ring)).collect();
                if !poly::intersection(&piece, &board).is_empty() {
                    members.push(format!("board {name}"));
                }
            }
            if rails.iter().any(|rail| !poly::intersection(&piece, &rect_shapes(*rail)).is_empty()) {
                members.push("rails".to_owned());
            }
            pieces.push(format!("[{}]", members.join(", ")));
        }
        return Err(format!(
            "the panel falls apart into {} pieces ({}); tabs must join every board to a rail or a joined board",
            united.len(),
            pieces.join(" ")
        ));
    }
    // The outline: every contour of the union on Edge.Cuts.
    for (k, shape) in united.iter().enumerate() {
        for (j, contour) in shape.iter().enumerate() {
            let points: Vec<String> = contour.iter().map(|p| format!("(xy {} {})", mm(p.x), mm(p.y))).collect();
            let text = format!(
                "(gr_poly (pts {}) (stroke (width 0.05) (type default)) (fill none) (layer \"Edge.Cuts\") (uuid {}))",
                points.join(" "),
                quote(&uuid(&format!("outline:{k}:{j}")))
            );
            items.push(crate::sexpr::parse(&text, "frame").expect("a valid outline"));
        }
    }
    Ok(Frame { items, tabs, bounds: (outer[0], outer[1], outer[2], outer[3]), rails })
}

fn build_tab(
    spec: &PanelSpec,
    tab: &PanelTab,
    local: &IndexMap<String, Vec<Vec<Point>>>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    poses: &IndexMap<String, Pose>,
    rails: &[Rect],
) -> Result<BuiltTab, String> {
    let side = match &tab.edge {
        TabEdge::Side(side) => *side,
        TabEdge::Named(name) => {
            return Err(format!(
                "tab {}: named edges ({name}) need the board's source intent; use side for now",
                tab.name
            ));
        }
    };
    let rings = &local[&tab.board];
    let rect = match rings.as_slice() {
        [ring] if ring.len() == 4 && is_axis_aligned(ring) => bounds_of(ring.iter()),
        _ => {
            return Err(format!(
                "tab {}: board {} is not a plain rectangle, so it has no {} side",
                tab.name,
                tab.board,
                side.as_str()
            ));
        }
    };
    let (start, along, normal, length) = side_geometry(rect, side);
    let half = tab.width / 2;
    if tab.at - half < 0 || tab.at + half > length {
        return Err(format!(
            "tab {} ({} mm wide at {} mm) runs off board {}'s {} side ({} mm long)",
            tab.name,
            mm(tab.width),
            mm(tab.at),
            tab.board,
            side.as_str(),
            mm(length)
        ));
    }
    let pose = poses[&tab.board];
    let a = pose.apply((start.0 + along.0 * (tab.at - half), start.1 + along.1 * (tab.at - half)));
    let b = pose.apply((start.0 + along.0 * (tab.at + half), start.1 + along.1 * (tab.at + half)));
    let n = pose.rotate(normal);
    // Look across the gap for the nearest rail or other board.
    let probe = [
        a,
        b,
        (b.0 + n.0 * MAX_TAB_LENGTH, b.1 + n.1 * MAX_TAB_LENGTH),
        (a.0 + n.0 * MAX_TAB_LENGTH, a.1 + n.1 * MAX_TAB_LENGTH),
    ];
    let probe = ring_shapes(&probe);
    let mut nearest: Option<(i64, Option<String>)> = None;
    let mut consider = |shapes: poly::Shapes, name: Option<&str>| {
        for shape in poly::intersection(&probe, &shapes) {
            for contour in shape {
                for p in contour {
                    let d = (p.x - a.0) * n.0 + (p.y - a.1) * n.1;
                    if nearest.as_ref().is_none_or(|(best, _)| d < *best) {
                        nearest = Some((d, name.map(str::to_owned)));
                    }
                }
            }
        }
    };
    for (name, rings) in outlines {
        if *name != tab.board {
            for ring in rings {
                consider(ring_shapes(ring), Some(name));
            }
        }
    }
    for rail in rails {
        consider(rect_shapes(*rail), None);
    }
    let Some((distance, across)) = nearest else {
        return Err(format!(
            "tab {}: nothing lies across board {}'s {} side within {} mm (a tab must bridge to a rail or a board)",
            tab.name,
            tab.board,
            side.as_str(),
            mm(MAX_TAB_LENGTH)
        ));
    };
    if distance <= 0 {
        return Err(format!(
            "tab {}: board {} touches or overlaps {} across its {} side",
            tab.name,
            tab.board,
            across.as_deref().map_or_else(|| "a rail".to_owned(), |b| format!("board {b}")),
            side.as_str()
        ));
    }
    // Its whole far end lands on what it bridges to: a sliver of a board or
    // rail inside its width would hold the joint by a corner.
    let far = |p: Point| (p.0 + n.0 * distance, p.1 + n.1 * distance);
    let deep = |p: Point| (p.0 + n.0 * (distance + 1_000), p.1 + n.1 * (distance + 1_000));
    let face = ring_shapes(&[far(a), far(b), deep(b), deep(a)]);
    let target: poly::Shapes = match &across {
        Some(name) => outlines[name].iter().flat_map(|ring| ring_shapes(ring)).collect(),
        None => rails.iter().flat_map(|rail| rect_shapes(*rail)).collect(),
    };
    let landed: f64 = poly::intersection(&face, &target).iter().map(|s| poly::shape_area(s)).sum();
    let whole: f64 = face.iter().map(|s| poly::shape_area(s)).sum();
    if landed < whole * 0.999 {
        return Err(format!(
            "tab {}: only part of its far end lands on {}; move it so its whole width meets a straight side",
            tab.name,
            across.as_deref().map_or_else(|| "a rail".to_owned(), |b| format!("board {b}"))
        ));
    }
    let _ = spec;
    Ok(BuiltTab {
        name: tab.name.clone(),
        board: tab.board.clone(),
        side,
        at: tab.at,
        width: tab.width,
        across,
        length: distance,
        mouse_bites: 0,
        bites: Vec::new(),
        edge: (a, b),
    })
}

/// Keep-outs inside each board along every tab landing, `depth` deep on
/// the copper `layers` and reaching `extend` past both tab ends, clipped to
/// the board. Where a tab joins a board its edge leaves Edge.Cuts (the
/// outline is the union), and after breaking the mouse-bite walls become the
/// board edge, out to almost the tab's ends: fills and tracks there would
/// lose the copper-to-edge clearance they keep on the board alone, so the
/// keep-out holds them back from every bite wall as the edge did.
///
/// The mouse-bite holes themselves (radius `bite_radius`) are cut out of the
/// keep-outs: they are the panel's own, and a keep-out that forbids pads would
/// otherwise contain them (KiCad reports each as an item not allowed). Board
/// pads stay forbidden there: after breaking, the bite walls are their edge.
pub fn tab_edge_keepouts(
    tabs: &[BuiltTab],
    poses: &IndexMap<String, Pose>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    depth: i64,
    extend: i64,
    bite_radius: i64,
    layers: &[String],
) -> Vec<Node> {
    if depth <= 0 || layers.is_empty() {
        return Vec::new();
    }
    let layer_list: Vec<String> = layers.iter().map(|l| quote(l)).collect();
    // Every hole, 10 um wider than drilled so no polygon edge grazes it.
    let holes: poly::Shapes = tabs
        .iter()
        .flat_map(|tab| tab.bites.iter())
        .flat_map(|p| {
            poly::circle(copper_cam::geometry::Point::new(p.0, p.1), (bite_radius + BITE_CUT_MARGIN) as f64, 250.0)
        })
        .collect();
    let mut zones = Vec::new();
    for tab in tabs {
        let n = poses[&tab.board].rotate(side_geometry([0, 0, 0, 0], tab.side).2);
        let (a, b) = tab.edge;
        let dir = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let (a, b) = ((a.0 - dir.0 * extend, a.1 - dir.1 * extend), (b.0 + dir.0 * extend, b.1 + dir.1 * extend));
        let shift = |p: Point, d: i64| (p.0 + n.0 * d, p.1 + n.1 * d);
        // Into the tab's own board, and into the board it bridges to.
        let mut strips = vec![("near", tab.board.as_str(), [a, b, shift(b, -depth), shift(a, -depth)])];
        if let Some(across) = &tab.across {
            let (fa, fb) = (shift(a, tab.length), shift(b, tab.length));
            strips.push(("far", across.as_str(), [fa, fb, shift(fb, depth), shift(fa, depth)]));
        }
        for (end, board, corners) in strips {
            let board_shape: poly::Shapes = outlines[board].iter().flat_map(|ring| ring_shapes(ring)).collect();
            let strip = poly::intersection(&ring_shapes(&corners), &board_shape);
            let strip = if holes.is_empty() { strip } else { poly::difference(&strip, &holes) };
            for (k, piece) in strip.iter().enumerate() {
                // A hole wholly inside the strip is joined to its outline by a
                // zero-width bridge, as KiCad stores zone outlines.
                let pts: Vec<String> =
                    poly::fracture(piece).iter().map(|p| format!("(xy {} {})", mm(p.x), mm(p.y))).collect();
                let key = if k == 0 { format!("{}:{end}", tab.name) } else { format!("{}:{end}:{k}", tab.name) };
                let text = format!(
                    "(zone (layers {}) (uuid {}) (name {}) (hatch edge 0.5) \
                     (keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed) (copperpour not_allowed) (footprints allowed)) \
                     (polygon (pts {})))",
                    layer_list.join(" "),
                    quote(&uuid(&format!("tab-edge:{key}"))),
                    quote(&format!("panel/tab-edge/{}/{end}", tab.name)),
                    pts.join(" ")
                );
                zones.push(crate::sexpr::parse(&text, "frame").expect("a valid keep-out"));
            }
        }
    }
    zones
}

/// How much wider than the drill a mouse bite is cut out of a keep-out.
pub const BITE_CUT_MARGIN: i64 = 10_000;

/// The panel rectangle a built tab occupies across its gap.
pub fn tab_rect(tab: &BuiltTab, pose: Pose) -> Rect {
    let normal = pose.rotate(side_geometry([0, 0, 0, 0], tab.side).2);
    let (a, b) = tab.edge;
    let far = |p: Point| (p.0 + normal.0 * tab.length, p.1 + normal.1 * tab.length);
    bounds_of([a, b, far(a), far(b)].iter())
}

fn is_axis_aligned(ring: &[Point]) -> bool {
    (0..ring.len()).all(|k| {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        p.0 == q.0 || p.1 == q.1
    })
}
