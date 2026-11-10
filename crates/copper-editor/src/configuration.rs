//! The editor uses the physical commands' source-driven construction policy.

use copper_lang::compiler::Design;
use copper_pcb::physicalize::PhysicalOptions;

use crate::{Error, Result};

/// Editor dependencies are prepared before building the scene, so this stage
/// uses the locked, offline cache.
pub fn physical_options(design: &Design, layers: Option<i64>, fab_profile: &str) -> Result<PhysicalOptions> {
    copper_pcb::configuration::physical_options(design, (100.0, 80.0), layers, fab_profile, true, true)
        .map_err(Error::from)
}
