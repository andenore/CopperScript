//! A small deterministic pseudo-random generator for Monte Carlo runs.
//!
//! SplitMix64 (Steele, Lea and Flood, "Fast splittable pseudorandom number
//! generators", OOPSLA 2014): a 64-bit state advanced by a fixed odd
//! increment and finalised by two multiply-xorshift rounds. It passes the
//! BigCrush battery and needs no dependency. The same seed gives the same
//! sequence on every run; across platforms the integer stream is identical,
//! and the floating-point draws ([`Rng::gaussian`]) use only `ln`, `sqrt`
//! and `cos`, which agree to the last bit on the platforms CopperScript is
//! built for in practice but are not guaranteed to by Rust.
//!
//! [`Rng::stream`] derives an independent stream for run `index` of a seed,
//! so run 17 of a 200-run Monte Carlo draws the same values as run 17 of a
//! 1000-run one.

/// The SplitMix64 increment (the golden ratio as a 64-bit fraction).
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A SplitMix64 generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator seeded with `seed`.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The generator of run `index` under `seed`: independent of how many
    /// runs there are.
    pub fn stream(seed: u64, index: u64) -> Self {
        Self { state: mix(seed ^ mix(index.wrapping_add(1).wrapping_mul(GAMMA))) }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        mix(self.state)
    }

    /// Uniform in `[0, 1)`, 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in `[-1, 1)`.
    pub fn symmetric(&mut self) -> f64 {
        2.0 * self.uniform() - 1.0
    }

    /// A standard normal value (mean 0, standard deviation 1), Box-Muller.
    pub fn gaussian(&mut self) -> f64 {
        // 1 - u is in (0, 1], so the logarithm is finite.
        let u1 = 1.0 - self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// A normal value with standard deviation `sigma`, redrawn until it lies
    /// within `[-bound, bound]` (a truncated normal; `bound` must be positive).
    pub fn truncated_gaussian(&mut self, sigma: f64, bound: f64) -> f64 {
        loop {
            let x = sigma * self.gaussian();
            if x.abs() <= bound {
                return x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_values() {
        // The first outputs of SplitMix64 seeded with 0 (Vigna's reference implementation).
        let mut r = Rng::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(r.next_u64(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn streams_are_reproducible_and_distinct() {
        let a: Vec<u64> = (0..4).map(|_| Rng::stream(7, 3).next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(Rng::stream(7, 3).next_u64(), Rng::stream(7, 4).next_u64());
        assert_ne!(Rng::stream(7, 3).next_u64(), Rng::stream(8, 3).next_u64());
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let u = r.uniform();
            assert!((0.0..1.0).contains(&u));
            let t = r.truncated_gaussian(1.0 / 3.0, 1.0);
            assert!(t.abs() <= 1.0);
        }
    }
}
