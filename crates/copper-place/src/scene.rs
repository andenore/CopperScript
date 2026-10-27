//! The planner's view of one immutable physical board: placement rules,
//! courtyard geometry, pad positions and the relative-rule measures
//! (`placement._placement_polygon`, `transformed_pad_position`,
//! `_relative_penalty`, `relative_placement_violations` and friends).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use copper_drc::legality::{Checker, Policy};
use copper_pcb::board::{
    AlignmentAxis, ComponentPlacementRule, PhysicalBoard, Placement, PlacementTarget, RelativePlacementKind,
    RelativePlacementRule,
};
use copper_pcb::transform::{transform_local, transformed_local_point};
use copper_pcb::{Error, Result};
use copper_physical::pyfloat::{hypot, round_int};
use copper_physical::{BoardSide, Point};
use copper_units::Decimal;

/// Poses by reference, in Python `dict` insertion order.
pub type Poses = IndexMap<String, Placement>;

/// A borrowed list of relative rules (`rules=` arguments).
pub type Rules<'r> = [&'r RelativePlacementRule];

/// `relative_placement_violations`: the fields the planner reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    pub references: Vec<String>,
    pub message: String,
}

/// One board's static placement context.
pub struct Scene<'a> {
    pub board: &'a PhysicalBoard,
    pub checker: Checker<'a>,
    rules: IndexMap<&'a str, &'a ComponentPlacementRule>,
    /// `distribution_zone_nets(board)`.
    pub plane_nets: IndexSet<String>,
    /// Every relative rule of the board, in order.
    pub relative: Vec<&'a RelativePlacementRule>,
    courtyards: RefCell<IndexMap<(usize, BoardSide), Vec<(Decimal, Rc<Vec<Point>>)>>>,
}

impl<'a> Scene<'a> {
    pub fn new(board: &'a PhysicalBoard, policy: Policy) -> Self {
        Scene {
            board,
            checker: Checker::new(board, policy),
            rules: board.placement_rules.iter().map(|r| (r.reference.as_str(), r)).collect(),
            plane_nets: copper_pcb::zones::distribution_zone_nets(board),
            relative: board.relative_rules.iter().collect(),
            courtyards: RefCell::new(IndexMap::new()),
        }
    }

    /// `_rules(board).get(reference)`.
    pub fn rule(&self, reference: &str) -> Option<&'a ComponentPlacementRule> {
        self.rules.get(reference).copied()
    }

    /// `_allowed_orientations`.
    pub fn allowed_orientations(&self, reference: &str) -> Vec<Decimal> {
        match self.rule(reference) {
            Some(rule) => match &rule.fixed_rotation_degrees {
                Some(rotation) => vec![rotation.clone()],
                None => rule.allowed_orientations.clone(),
            },
            None => [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
        }
    }

    /// `_rotated_courtyard`: the courtyard (or body rectangle) at the pose's angle and side.
    pub fn rotated_courtyard(&self, pose: &Placement) -> Rc<Vec<Point>> {
        let index = self.board.footprints.get_index_of(&pose.footprint).expect("placed footprint");
        let mut cache = self.courtyards.borrow_mut();
        let entries = cache.entry((index, pose.side)).or_default();
        if let Some((_, polygon)) = entries.iter().find(|(rotation, _)| *rotation == pose.rotation_degrees) {
            return polygon.clone();
        }
        let footprint = &self.board.footprints[index];
        let courtyard = if footprint.courtyard.is_empty() {
            let (w, h) =
                ((footprint.body_size.width_nm + 1).div_euclid(2), (footprint.body_size.height_nm + 1).div_euclid(2));
            vec![Point::new(-w, -h), Point::new(w, -h), Point::new(w, h), Point::new(-w, h)]
        } else {
            footprint.courtyard.clone()
        };
        let polygon: Rc<Vec<Point>> =
            Rc::new(courtyard.iter().map(|point| transform_local(*point, &pose.rotation_degrees, pose.side)).collect());
        entries.push((pose.rotation_degrees.clone(), polygon.clone()));
        polygon
    }

    /// `_placement_polygon`: the placed courtyard, or the conservative body rectangle.
    pub fn polygon(&self, pose: &Placement) -> Vec<Point> {
        let (x, y) = (pose.position.x_nm, pose.position.y_nm);
        self.rotated_courtyard(pose).iter().map(|p| Point::new(p.x_nm + x, p.y_nm + y)).collect()
    }

    /// `_half_extents`: the larger absolute extent of the rotated courtyard on each axis.
    pub fn half_extents(&self, pose: &Placement) -> (i64, i64) {
        let (min_x, min_y, max_x, max_y) = point_bounds(&self.rotated_courtyard(pose));
        (min_x.abs().max(max_x.abs()), min_y.abs().max(max_y.abs()))
    }

    /// `_half_extents_mm`.
    pub fn half_extents_mm(&self, pose: &Placement) -> (f64, f64) {
        let (width, height) = self.half_extents(pose);
        (width as f64 / 1_000_000.0, height as f64 / 1_000_000.0)
    }

    /// `_footprint_area`: the body area.
    pub fn footprint_area(&self, pose: &Placement) -> i64 {
        let footprint = &self.board.footprints[&pose.footprint];
        footprint.body_size.width_nm * footprint.body_size.height_nm
    }

    /// `transformed_pad_position`: the first pad with this number.
    pub fn pad_position(&self, pose: &Placement, pad: &str) -> Result<Point> {
        let footprint = &self.board.footprints[&pose.footprint];
        match footprint.pads.iter().find(|item| item.number == pad) {
            Some(item) => Ok(transformed_local_point(pose, item.position)),
            None => Err(Error::Value(format!(
                "component {} has no pad {}",
                copper_json::python_str_repr(&pose.reference),
                copper_json::python_str_repr(pad)
            ))),
        }
    }

    /// `_pad_offset_mm`: the pad's rotated offset from the component origin.
    pub fn pad_offset_mm(&self, pose: &Placement, pad: &str) -> (f64, f64) {
        let footprint = &self.board.footprints[&pose.footprint];
        let item = footprint.pads.iter().find(|item| item.number == pad).expect("netted pad");
        let offset = transform_local(item.position, &pose.rotation_degrees, pose.side);
        (offset.x_nm as f64 / 1_000_000.0, offset.y_nm as f64 / 1_000_000.0)
    }

    /// `_target_point`: a pad position, or the component origin.
    pub fn target_point(&self, placements: &Poses, target: &PlacementTarget) -> Result<Option<Point>> {
        let Some(pose) = placements.get(&target.reference) else { return Ok(None) };
        match &target.pad {
            Some(pad) => Ok(Some(self.pad_position(pose, pad)?)),
            None => Ok(Some(pose.position)),
        }
    }

    /// Every target point of a rule; `None` when a target is not placed.
    fn rule_points(&self, placements: &Poses, rule: &RelativePlacementRule) -> Result<Option<Vec<Point>>> {
        let mut points = Vec::new();
        for target in &rule.targets {
            points.push(self.target_point(placements, target)?);
        }
        Ok(points.into_iter().collect())
    }

    /// `_relative_penalty(board, placements, rules=rules)`.
    pub fn relative_penalty(&self, placements: &Poses, rules: &Rules) -> Result<i64> {
        let mut total = 0;
        for rule in rules {
            let Some(points) = self.rule_points(placements, rule)? else { continue };
            match rule.kind {
                RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance => {
                    let limit = rule.distance_nm.unwrap_or(0);
                    for pair in points.windows(2) {
                        let distance = rounded_distance(pair[0], pair[1]);
                        let excess = if rule.kind == RelativePlacementKind::MaxDistance {
                            distance - limit
                        } else {
                            limit - distance
                        };
                        total += excess.max(0) * rule.weight;
                    }
                }
                RelativePlacementKind::Align => {
                    let (low, high) = axis_span(&points, rule.axis);
                    total += (high - low - rule.tolerance_nm).max(0) * rule.weight;
                }
            }
        }
        Ok(total)
    }

    /// `_relative_penalty(board, placements)` over every board rule.
    pub fn penalty(&self, placements: &Poses) -> Result<i64> {
        self.relative_penalty(placements, &self.relative)
    }

    /// `_local_connection_length`: bounded preference inside pin-to-pin maximum distances.
    pub fn local_connection_length(&self, placements: &Poses, rules: &Rules) -> Result<i64> {
        let mut total = 0;
        for rule in rules {
            if rule.kind != RelativePlacementKind::MaxDistance {
                continue;
            }
            for pair in rule.targets.windows(2) {
                let (left, right) = (&pair[0], &pair[1]);
                if left.pad.is_none() || right.pad.is_none() {
                    continue;
                }
                let Some(a) = self.target_point(placements, left)? else { continue };
                let Some(b) = self.target_point(placements, right)? else { continue };
                total += rule.distance_nm.unwrap_or(0).min(rounded_distance(a, b)) * rule.weight;
            }
        }
        Ok(total)
    }

    /// `_relative_margin`: the smallest remaining slack of any relative rule.
    pub fn relative_margin(&self, placements: &Poses) -> Result<i64> {
        let mut margins: Vec<i64> = Vec::new();
        for rule in &self.relative {
            let Some(points) = self.rule_points(placements, rule)? else { continue };
            match rule.kind {
                RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance => {
                    for pair in points.windows(2) {
                        let distance = rounded_distance(pair[0], pair[1]);
                        margins.push(if rule.kind == RelativePlacementKind::MaxDistance {
                            rule.distance_nm.unwrap_or(0) - distance
                        } else {
                            distance - rule.distance_nm.unwrap_or(0)
                        });
                    }
                }
                RelativePlacementKind::Align => {
                    let (low, high) = axis_span(&points, rule.axis);
                    margins.push(rule.tolerance_nm - (high - low));
                }
            }
        }
        Ok(margins.into_iter().min().unwrap_or(0))
    }

    /// `relative_placement_violations`: the references and message of each violated rule.
    pub fn violations(&self, placements: &Poses) -> Result<Vec<Violation>> {
        let mut violations = Vec::new();
        for rule in &self.relative {
            let Some(points) = self.rule_points(placements, rule)? else { continue };
            match rule.kind {
                RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance => {
                    let limit = rule.distance_nm.unwrap_or(0);
                    let maximum = rule.kind == RelativePlacementKind::MaxDistance;
                    for (index, pair) in points.windows(2).enumerate() {
                        let actual = rounded_distance(pair[0], pair[1]);
                        let amount = if maximum { actual - limit } else { limit - actual }.max(0);
                        if amount == 0 {
                            continue;
                        }
                        let (left, right) = (&rule.targets[index], &rule.targets[index + 1]);
                        violations.push(Violation {
                            references: vec![left.reference.clone(), right.reference.clone()],
                            message: format!(
                                "{} to {}: {} distance {}, actual {}",
                                left.reference,
                                right.reference,
                                if maximum { "maximum" } else { "minimum" },
                                mm_text(limit),
                                mm_text(actual)
                            ),
                        });
                    }
                }
                RelativePlacementKind::Align => {
                    let (low, high) = axis_span(&points, rule.axis);
                    let actual = high - low;
                    if (actual - rule.tolerance_nm).max(0) != 0 {
                        let references: Vec<String> = rule.targets.iter().map(|t| t.reference.clone()).collect();
                        let message = format!(
                            "{}: {} alignment tolerance {}, actual span {}",
                            references.join(", "),
                            rule.axis.map_or("axis", AlignmentAxis::value),
                            mm_text(rule.tolerance_nm),
                            mm_text(actual)
                        );
                        violations.push(Violation { references, message });
                    }
                }
            }
        }
        Ok(violations)
    }

    /// `_fixed_placements`: source- and session-fixed poses, by sorted reference.
    pub fn fixed_placements(&self, source: &Poses, fixed_references: &IndexSet<String>) -> Poses {
        let mut references: Vec<&String> = source.keys().collect();
        references.sort();
        let mut result = Poses::new();
        for reference in references {
            let rule = self.rule(reference);
            let fixed_by_rule = rule.is_some_and(|r| r.fixed_position.is_some());
            if !fixed_references.contains(reference) && !fixed_by_rule {
                continue;
            }
            let mut pose = source[reference].clone();
            if let Some(rule) = rule.filter(|_| fixed_by_rule) {
                pose.position = rule.fixed_position.expect("fixed position");
                if let Some(rotation) = &rule.fixed_rotation_degrees {
                    pose.rotation_degrees = rotation.clone();
                }
                if let Some(side) = rule.side {
                    pose.side = side;
                }
            }
            result.insert(reference.clone(), pose);
        }
        result
    }

    /// `component_pair_courtyard_gap_nm` with the planner's fallback.
    pub fn courtyard_gap(&self, first: &str, second: &str, fallback: i64) -> Result<i64> {
        self.checker.courtyard_gap(first, second, fallback)
    }

    /// `_legal(candidate, placed, board, options)`.
    pub fn legal(&self, candidate: &Placement, placed: &Poses) -> Result<bool> {
        self.checker.legal(candidate, placed, None)
    }
}

/// `_point_bounds`: `(min_x, min_y, max_x, max_y)`.
pub fn point_bounds(points: &[Point]) -> (i64, i64, i64, i64) {
    let mut bounds = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for p in points {
        bounds = (bounds.0.min(p.x_nm), bounds.1.min(p.y_nm), bounds.2.max(p.x_nm), bounds.3.max(p.y_nm));
    }
    bounds
}

/// `round(hypot(dx, dy))`.
pub fn rounded_distance(a: Point, b: Point) -> i64 {
    round_int(hypot((a.x_nm - b.x_nm) as f64, (a.y_nm - b.y_nm) as f64))
}

fn axis_span(points: &[Point], axis: Option<AlignmentAxis>) -> (i64, i64) {
    let values = points.iter().map(|p| if axis == Some(AlignmentAxis::X) { p.x_nm } else { p.y_nm });
    (values.clone().min().expect("targets"), values.max().expect("targets"))
}

/// `_mm_text`: millimetres through the shared quantity formatter.
pub fn mm_text(value_nm: i64) -> String {
    let millimetres = Decimal::from_i64(value_nm).div(&Decimal::from_i64(1_000_000)).expect("finite length");
    copper_units::Quantity::of(copper_units::Dimension::Length, &millimetres, "mm").expect("a length in mm").to_string()
}

/// A pose with a new position (`replace(pose, position=...)`).
pub fn moved(pose: &Placement, position: Point) -> Placement {
    Placement { position, ..pose.clone() }
}

/// A pose with a new position and rotation.
pub fn posed(pose: &Placement, position: Point, rotation: &Decimal) -> Placement {
    Placement { position, rotation_degrees: rotation.clone(), ..pose.clone() }
}

/// `{**placements, reference: pose}`.
pub fn with(placements: &Poses, pose: &Placement) -> Poses {
    let mut result = placements.clone();
    result.insert(pose.reference.clone(), pose.clone());
    result
}

/// `{ref: pose for ref, pose in placements.items() if ref != reference}`.
pub fn without(placements: &Poses, reference: &str) -> Poses {
    placements.iter().filter(|(r, _)| *r != reference).map(|(r, p)| (r.clone(), p.clone())).collect()
}
