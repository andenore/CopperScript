//! Two-element L-section matching networks for a simulated feed impedance.
//!
//! The closed-form L-section design (Pozar, *Microwave Engineering*, §5.1)
//! gives up to four networks that match `Z_L` to `Z0` exactly at the design
//! frequency; each is evaluated across every band with ideal lossless parts
//! whose reactance scales with frequency, and the best worst-case one wins.
//! Component tolerance, parasitics, part Q and E-series snapping are not
//! modelled: the values are a starting point for bench tuning.

use ems::prelude::Complex64;

/// One ideal lumped element, in henry or farad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element {
    SeriesL(f64),
    SeriesC(f64),
    ShuntL(f64),
    ShuntC(f64),
}

impl Element {
    /// `"series 2.7 nH"`, `"shunt 1.2 pF"`.
    pub fn describe(&self) -> String {
        match self {
            Element::SeriesL(h) => format!("series {:.2} nH", h * 1e9),
            Element::SeriesC(f) => format!("series {:.2} pF", f * 1e12),
            Element::ShuntL(h) => format!("shunt {:.2} nH", h * 1e9),
            Element::ShuntC(f) => format!("shunt {:.2} pF", f * 1e12),
        }
    }
}

/// Elements in order from the antenna toward the radio.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub elements: Vec<Element>,
}

impl Network {
    /// The impedance the radio sees through the network at `f`.
    pub fn apply(&self, load: Complex64, f: f64) -> Complex64 {
        let w = 2.0 * std::f64::consts::PI * f;
        let mut z = load;
        for element in &self.elements {
            z = match *element {
                Element::SeriesL(h) => z + Complex64::new(0.0, w * h),
                Element::SeriesC(c) => z + Complex64::new(0.0, -1.0 / (w * c)),
                Element::ShuntL(h) => (Complex64::new(1.0, 0.0) / z + Complex64::new(0.0, -1.0 / (w * h))).inv(),
                Element::ShuntC(c) => (Complex64::new(1.0, 0.0) / z + Complex64::new(0.0, w * c)).inv(),
            };
        }
        z
    }

    pub fn describe(&self) -> String {
        if self.elements.is_empty() {
            return "none".to_owned();
        }
        let parts: Vec<String> = self.elements.iter().map(Element::describe).collect();
        format!("{} (antenna side first)", parts.join(", then "))
    }
}

/// `20 log10 |Γ|` of `z` against `z0`.
pub fn return_loss_db(z: Complex64, z0: f64) -> f64 {
    let gamma = (z - z0) / (z + z0);
    20.0 * gamma.norm().max(1e-12).log10()
}

fn series(x: f64, w: f64) -> Option<Element> {
    if x.abs() < 1e-9 {
        None
    } else if x > 0.0 {
        Some(Element::SeriesL(x / w))
    } else {
        Some(Element::SeriesC(-1.0 / (w * x)))
    }
}

fn shunt(b: f64, w: f64) -> Option<Element> {
    if b.abs() < 1e-12 {
        None
    } else if b > 0.0 {
        Some(Element::ShuntC(b / w))
    } else {
        Some(Element::ShuntL(-1.0 / (w * b)))
    }
}

/// Every L-section matching `load` to `z0` exactly at `f0`.
pub fn l_sections(load: Complex64, f0: f64, z0: f64) -> Vec<Network> {
    let (r, x) = (load.re, load.im);
    let w = 2.0 * std::f64::consts::PI * f0;
    let mut networks = Vec::new();
    if r <= 0.0 || !r.is_finite() || !x.is_finite() {
        return networks;
    }
    if return_loss_db(load, z0) < -40.0 {
        return vec![Network { elements: Vec::new() }];
    }
    // Shunt element next to the load, series toward the source.
    let disc = r * r + x * x - z0 * r;
    if disc >= 0.0 {
        for sign in [1.0, -1.0] {
            let b = (x + sign * (r / z0).sqrt() * disc.sqrt()) / (r * r + x * x);
            if b.abs() < 1e-15 {
                continue;
            }
            let xs = 1.0 / b + x * z0 / r - z0 / (b * r);
            let elements: Vec<Element> = [shunt(b, w), series(xs, w)].into_iter().flatten().collect();
            networks.push(Network { elements });
        }
    }
    // Series element next to the load, shunt toward the source.
    if r < z0 {
        for sign in [1.0, -1.0] {
            let xs = sign * (r * (z0 - r)).sqrt() - x;
            let b = sign * ((z0 - r) / r).sqrt() / z0;
            let elements: Vec<Element> = [series(xs, w), shunt(b, w)].into_iter().flatten().collect();
            networks.push(Network { elements });
        }
    }
    networks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_l_section_matches_at_the_design_frequency() {
        let f0 = 2.44e9;
        for load in [Complex64::new(12.0, -30.0), Complex64::new(120.0, 45.0), Complex64::new(30.0, 25.0)] {
            let networks = l_sections(load, f0, 50.0);
            assert!(!networks.is_empty(), "{load:?}");
            for network in networks {
                let rl = return_loss_db(network.apply(load, f0), 50.0);
                assert!(rl < -60.0, "{load:?} {network:?} {rl}");
            }
        }
    }

    #[test]
    fn a_matched_load_needs_no_elements() {
        let networks = l_sections(Complex64::new(50.0, 0.0), 2.44e9, 50.0);
        assert!(networks.iter().any(|n| n.elements.is_empty()));
    }
}
