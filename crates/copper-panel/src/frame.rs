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
    /// The board it bridges to, or `None` for the frame.
    pub across: Option<String>,
    /// What a frame tab lands on: `rail`, or `frame` for the frame fill.
    pub lands_on: &'static str,
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
    /// The area the router mills away, in square nanometres.
    pub cut_area: f64,
    /// Areas beyond the milled gap that are cut away rather than kept as
    /// frame (each drops out as a loose slug): bounds and area.
    pub pockets: Vec<(Rect, f64)>,
}

/// Arcs (rounded corners, the frame fill's offsets) as polylines this close.
const ARC_ERROR: f64 = 1_000.0;

/// How much smaller than the cutter's radius the rounded corners are cut.
const OPENING_SLACK: i64 = 10_000;

/// Shapes eroded and grown back by `radius`: the part of a cut region a round
/// cutter of that radius can reach. Corners of the region that the material
/// wraps come out rounded to the radius; corners of the material stay sharp.
///
/// A slot exactly as wide as the cutter erodes to nothing at the full
/// radius, so the radius is taken `OPENING_SLACK` short: such a slot stays.
fn opening(shapes: &poly::Shapes, radius: i64) -> poly::Shapes {
    let radius = radius - OPENING_SLACK;
    let eroded = poly::offset(shapes, -radius, true, ARC_ERROR);
    poly::intersection(&poly::offset(&eroded, radius, true, ARC_ERROR), shapes)
}

fn touches(a: &poly::Shapes, b: &poly::Shapes) -> bool {
    !poly::intersection(&poly::offset(a, 1_000, false, ARC_ERROR), b).is_empty()
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
/// coordinates, `outlines` the same rings on the panel, and `bodies` the
/// parts placed with the panel that reach past their board (the frame fill
/// keeps the milled gap from them). `clear` holds each board's stretches of
/// sides where no tab may go, `(side, start, end)` in its own frame: the
/// bites that run past a tab's ends stay out of them.
pub fn build(
    spec: &PanelSpec,
    local: &IndexMap<String, Vec<Vec<Point>>>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    poses: &IndexMap<String, Pose>,
    bodies: &poly::Shapes,
    clear: &IndexMap<String, Vec<(Side, i64, i64)>>,
) -> Result<Frame, String> {
    if spec.cutter <= 0 || spec.cutter > spec.spacing {
        return Err(format!(
            "a {} mm cutter cannot mill the {} mm gaps between boards; widen spacing or give a smaller cutter",
            mm(spec.cutter),
            mm(spec.spacing)
        ));
    }
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
    let mut boards: poly::Shapes = Vec::new();
    for rings in outlines.values() {
        for ring in rings {
            boards.extend(ring_shapes(ring));
        }
    }
    let boards = poly::union(&boards);
    let rail_shapes: poly::Shapes = poly::union(&rails.iter().flat_map(|rail| rect_shapes(*rail)).collect());
    // Beyond the milled gap round every board and placed body: kept as frame
    // where it joins a rail, unless a web there is too thin for the cutter.
    let mut keep_off = boards.clone();
    keep_off.extend(bodies.iter().cloned());
    let near = poly::offset(&poly::union(&keep_off), s, true, ARC_ERROR);
    let beyond = poly::difference(&poly::difference(&rect_shapes(outer), &near), &rail_shapes);
    let beyond = opening(&beyond, spec.cutter / 2);
    let mut frame_material = rail_shapes.clone();
    let mut pockets: Vec<(Rect, f64)> = Vec::new();
    for piece in &beyond {
        let piece = vec![piece.clone()];
        if spec.frame_fill && !rail_shapes.is_empty() && touches(&piece, &rail_shapes) {
            frame_material.extend(piece);
        } else {
            let (x0, y0, x1, y1) = poly::bounds(&piece[0]);
            pockets.push(([x0, y0, x1, y1], poly::shape_area(&piece[0])));
        }
    }
    let frame_material = poly::union(&frame_material);
    let mut shapes: poly::Shapes = boards.clone();
    shapes.extend(frame_material.iter().cloned());
    let mut items: Vec<Node> = Vec::new();
    let mut tabs: Vec<BuiltTab> = Vec::new();
    let mut footprints: Vec<(String, Rect)> = Vec::new();
    for tab in &spec.tabs {
        let built = build_tab(spec, tab, local, outlines, poses, &frame_material, &rail_shapes)?;
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
        // Mouse bites `inset` inside each board the tab touches: a row across
        // the tab, centred. The cutter leaves a fillet in each corner where
        // the tab meets a board, so the row goes on at its pitch up to a
        // cutter radius past both tab ends to break through them, where those
        // holes stay on the board and out of stretches kept clear of tabs.
        let width = built.width;
        let fillet = spec.cutter / 2;
        let (drill, pitch) = (spec.mouse_bite.drill, spec.mouse_bite.pitch);
        let count = holes_per_tab(width, &spec.mouse_bite);
        let span = (count - 1) * pitch;
        let mut offsets: Vec<(i64, bool)> = (0..count).map(|k| (-span / 2 + k * pitch, true)).collect();
        let mut beyond_end = span / 2 + pitch;
        while beyond_end + drill / 2 <= width / 2 + fillet {
            offsets.push((-beyond_end, false));
            offsets.push((beyond_end, false));
            beyond_end += pitch;
        }
        let direction = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let centre = ((a.0 + b.0) / 2, (a.1 + b.1) / 2);
        let mut pads = Vec::new();
        let mut bites = Vec::new();
        let mut ends = vec![(-spec.mouse_bite.inset, built.board.as_str())];
        if let Some(across) = &built.across {
            ends.push((built.length + spec.mouse_bite.inset, across.as_str()));
        }
        // Where a hole `along` from the tab's centre falls on an end's board
        // side, in that board's frame: the side and the position along it.
        let place = |owner: &str, depth: i64, along: i64| -> Option<(Side, i64)> {
            if owner == built.board {
                return Some((built.side, built.at + along));
            }
            let pose = poses[owner];
            let nb = bounds_of(local[owner].iter().flatten());
            let far = |t: i64| {
                pose.invert((
                    centre.0 + direction.0 * t + normal.0 * (depth - spec.mouse_bite.inset),
                    centre.1 + direction.1 * t + normal.1 * (depth - spec.mouse_bite.inset),
                ))
            };
            let (side, ..) = crate::intent::side_of(nb, far(-width / 2), far(width / 2))?;
            let q = far(along);
            Some((side, if matches!(side, Side::Top | Side::Bottom) { q.0 - nb[0] } else { q.1 - nb[1] }))
        };
        // The tab with its fillets, as far as the bites reach: holes there
        // drill the fillets, and the last may break out into the slot.
        let wide = |p: Point, sign: i64| (p.0 + sign * direction.0 * fillet, p.1 + sign * direction.1 * fillet);
        let filleted =
            ring_shapes(&[wide(corners[0], -1), wide(corners[1], 1), wide(corners[2], 1), wide(corners[3], -1)]);
        for (depth, owner) in ends {
            // Every hole perforates its board: it lies on the board and the
            // tab, not in a slot or past a thin flange of the board.
            let mut material: poly::Shapes = outlines[owner].iter().flat_map(|ring| ring_shapes(ring)).collect();
            material.extend(filleted.iter().cloned());
            let material = poly::union(&material);
            for &(along, required) in &offsets {
                if !required {
                    let kept_clear = place(owner, depth, along).is_none_or(|(side, at)| {
                        clear.get(owner).is_some_and(|stretches| {
                            stretches
                                .iter()
                                .any(|(s, start, end)| *s == side && at - drill / 2 < *end && at + drill / 2 > *start)
                        })
                    });
                    if kept_clear {
                        continue;
                    }
                }
                let p = (direction.0 * along + normal.0 * depth, direction.1 * along + normal.1 * depth);
                let hole = poly::circle(
                    copper_cam::geometry::Point::new(centre.0 + p.0, centre.1 + p.1),
                    spec.mouse_bite.drill as f64 / 2.0,
                    250.0,
                );
                let astray: f64 = poly::difference(&hole, &material).iter().map(|s| poly::shape_area(s)).sum();
                if astray > 1e6 && !required {
                    continue;
                }
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
            if !poly::intersection(&piece, &frame_material).is_empty() {
                members.push("frame".to_owned());
            }
            pieces.push(format!("[{}]", members.join(", ")));
        }
        return Err(format!(
            "the panel falls apart into {} pieces ({}); tabs must join every board to a rail or a joined board",
            united.len(),
            pieces.join(" ")
        ));
    }
    // Rows of bites from neighbouring tabs, each run a cutter radius long,
    // must not crowd closer than their own pitch.
    for (k, one) in tabs.iter().enumerate() {
        for other in &tabs[k + 1..] {
            let closest = one
                .bites
                .iter()
                .flat_map(|p| other.bites.iter().map(move |q| ((p.0 - q.0) as f64).hypot((p.1 - q.1) as f64)))
                .fold(f64::INFINITY, f64::min);
            if closest < spec.mouse_bite.pitch as f64 - 1_000.0 {
                return Err(format!(
                    "tabs {} and {}: their mouse bites, which run {} mm past each tab end through the cutter's \
                     fillets, come {} mm apart, closer than their {} mm pitch; move the tabs apart",
                    one.name,
                    other.name,
                    mm(spec.cutter / 2),
                    mm(closest.round() as i64),
                    mm(spec.mouse_bite.pitch)
                ));
            }
        }
    }
    // What the router mills: the rest of the outline's rectangle, as far as
    // a round cutter reaches. Every corner the material wraps comes out
    // rounded to the cutter's radius; the material left there is a fillet.
    // Past the panel's edge is open air: a slot running out to it stays open.
    let d = spec.cutter;
    let around = rect_shapes([outer[0] - d, outer[1] - d, outer[2] + d, outer[3] + d]);
    let cut = poly::intersection(&opening(&poly::difference(&around, &united), d / 2), &rect_shapes(outer));
    let milled = poly::union(&poly::difference(&rect_shapes(outer), &cut));
    // A fillet holds to a tab or the frame; one joining two boards by
    // itself would fuse them (a gap the cutter could not enter).
    let mut tab_shapes: poly::Shapes = Vec::new();
    for tab in &tabs {
        tab_shapes.extend(rect_shapes(tab_rect(tab, poses[&tab.board])));
    }
    // Slivers where polygonal arcs meet are far too small to bridge a gap.
    let board_bounds: Vec<(&String, Rect)> =
        outlines.iter().map(|(name, rings)| (name, bounds_of(rings.iter().flatten()))).collect();
    for fillet in poly::difference(&milled, &united) {
        if poly::shape_area(&fillet) < 1e9 {
            continue;
        }
        let (x0, y0, x1, y1) = poly::bounds(&fillet);
        let piece = vec![fillet];
        let joined: Vec<&String> = board_bounds
            .iter()
            .filter(|(_, b)| x0 <= b[2] + 1_000 && b[0] <= x1 + 1_000 && y0 <= b[3] + 1_000 && b[1] <= y1 + 1_000)
            .filter(|(name, _)| touches(&piece, &outlines[*name].iter().flat_map(|ring| ring_shapes(ring)).collect()))
            .map(|(name, _)| *name)
            .collect();
        if joined.len() > 1 && !touches(&piece, &tab_shapes) {
            return Err(format!(
                "boards {} and {}: the {} mm cutter cannot mill between them near ({}, {}) mm; move them apart",
                joined[0],
                joined[1],
                mm(spec.cutter),
                mm(x0),
                mm(y0)
            ));
        }
    }
    let cut_area: f64 = cut.iter().map(|shape| poly::shape_area(shape)).sum();
    // The outline: every contour of the milled panel on Edge.Cuts.
    for (k, shape) in milled.iter().enumerate() {
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
    Ok(Frame { items, tabs, bounds: (outer[0], outer[1], outer[2], outer[3]), rails, cut_area, pockets })
}

fn build_tab(
    spec: &PanelSpec,
    tab: &PanelTab,
    local: &IndexMap<String, Vec<Vec<Point>>>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    poses: &IndexMap<String, Pose>,
    frame: &poly::Shapes,
    rails: &poly::Shapes,
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
    consider(frame.clone(), None);
    let Some((distance, across)) = nearest else {
        return Err(format!(
            "tab {}: nothing lies across board {}'s {} side within {} mm (a tab must bridge to the frame or a board)",
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
            across.as_deref().map_or_else(|| "the frame".to_owned(), |b| format!("board {b}")),
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
        None => frame.clone(),
    };
    let landed: f64 = poly::intersection(&face, &target).iter().map(|s| poly::shape_area(s)).sum();
    let whole: f64 = face.iter().map(|s| poly::shape_area(s)).sum();
    if landed < whole * 0.999 {
        return Err(format!(
            "tab {}: only part of its far end lands on {}; move it so its whole width meets a straight side",
            tab.name,
            across.as_deref().map_or_else(|| "the frame".to_owned(), |b| format!("board {b}"))
        ));
    }
    let _ = spec;
    let lands_on = if across.is_none() && poly::intersection(&face, rails).is_empty() { "frame" } else { "rail" };
    Ok(BuiltTab {
        name: tab.name.clone(),
        board: tab.board.clone(),
        side,
        at: tab.at,
        width: tab.width,
        across,
        lands_on,
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
