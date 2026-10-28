//! Polygon-aware coarse keepout geometry, without changing exact copper gates
//! (`pcbir.global_keepouts`).

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::ToPrimitive;

use copper_pcb::Result;
use copper_physical::geometry::{Bounds, bounds, point_in_polygon, polygon_triangles};
use copper_physical::{CopperKeepout, CopperLayer, Point};

/// `GlobalTrackKeepout`: a track keepout's outer ring as triangles for lane counting.
#[derive(Clone, Debug)]
pub struct GlobalTrackKeepout {
    pub layers: Vec<CopperLayer>,
    pub vertices: Vec<Point>,
    pub area: Bounds,
    pub triangles: Vec<[Point; 3]>,
}

fn rational(value: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(value))
}

fn floor(value: &BigRational) -> i64 {
    value.numer().div_floor(value.denom()).to_i64().expect("coordinate")
}

fn ceil(value: &BigRational) -> i64 {
    value.numer().div_ceil(value.denom()).to_i64().expect("coordinate")
}

impl GlobalTrackKeepout {
    /// `from_keepout`: holes retain the conservative filled outer ring.
    pub fn from_keepout(keepout: &CopperKeepout) -> Result<Self> {
        let vertices = keepout.vertices.clone();
        Ok(GlobalTrackKeepout {
            layers: keepout.layers.clone(),
            area: bounds(&vertices),
            triangles: polygon_triangles(&vertices)?,
            vertices,
        })
    }

    pub fn blocks_node(&self, point: Point, layer: CopperLayer) -> bool {
        self.layers.contains(&layer)
            && self.area.min_x <= point.x_nm
            && point.x_nm <= self.area.max_x
            && self.area.min_y <= point.y_nm
            && point.y_nm <= self.area.max_y
            && point_in_polygon(point, &self.vertices)
    }

    /// `blocked_intervals`: conservative transverse projection over the whole coarse edge.
    ///
    /// Clipping each convex triangle to the longitudinal strip preserves a
    /// concave region's empty bays. Rational cuts round outward to integer nm.
    pub fn blocked_intervals(
        &self,
        first: Point,
        second: Point,
        half_width_nm: i64,
        clearance_nm: i64,
        layer: CopperLayer,
    ) -> Vec<(i64, i64)> {
        if !self.layers.contains(&layer) {
            return Vec::new();
        }
        let horizontal = first.y_nm == second.y_nm;
        let along = |p: Point| if horizontal { p.x_nm } else { p.y_nm };
        let across = |p: Point| if horizontal { p.y_nm } else { p.x_nm };
        let (mut low, mut high) = (along(first).min(along(second)), along(first).max(along(second)));
        low -= clearance_nm;
        high += clearance_nm;
        let center = across(first);
        let (box_low, box_high) =
            if horizontal { (self.area.min_x, self.area.max_x) } else { (self.area.min_y, self.area.max_y) };
        let (side_low, side_high) =
            if horizontal { (self.area.min_y, self.area.max_y) } else { (self.area.min_x, self.area.max_x) };
        if box_high < low
            || box_low > high
            || side_high + clearance_nm < center - half_width_nm
            || side_low - clearance_nm > center + half_width_nm
        {
            return Vec::new();
        }
        let mut result = Vec::new();
        for triangle in &self.triangles {
            let mut values: Vec<BigRational> = triangle
                .iter()
                .filter(|p| low <= along(**p) && along(**p) <= high)
                .map(|p| rational(across(*p)))
                .collect();
            for index in 0..3 {
                let (a, b) = (triangle[index], triangle[(index + 1) % 3]);
                let (av, bv) = (along(a), along(b));
                if av == bv {
                    continue;
                }
                for boundary in [low, high] {
                    if av.min(bv) <= boundary && boundary <= av.max(bv) {
                        let t = BigRational::new(BigInt::from(boundary - av), BigInt::from(bv - av));
                        values.push(rational(across(a)) + t * rational(across(b) - across(a)));
                    }
                }
            }
            if !values.is_empty() {
                let minimum = values.iter().min().expect("values");
                let maximum = values.iter().max().expect("values");
                result.push((floor(minimum) - clearance_nm, ceil(maximum) + clearance_nm));
            }
        }
        result
    }
}
