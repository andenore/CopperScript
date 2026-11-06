//! Original local fanout promises, not inferred terminals or a planner.
//! Every acceptance is freshly checked; saved anchors/status are observations.
//! A verified pad-to-site launch is not a complete escape-to-boundary proof.

use copper_pcb::board::{PadReference, PhysicalBoard, Via};
use copper_pcb::{Error, Result};
use copper_physical::{BoardSide, CopperLayer};

use crate::fanout::FanoutResult;
use crate::fixed_prefix::{AdmittedFixedPrefix, FixedPrefixRole};
use crate::pin_escape::{Anchor, verified_fanout_path, verified_routing_access};
use crate::routing_clearance::RoutingClearanceIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorProvenance {
    FanoutViaAccess,
    FanoutSurfaceAccess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapePromise {
    pub pad: PadReference,
    pub net: String,
    pub original_anchor: Anchor,
    pub provenance: AnchorProvenance,
    /// This observation cannot replace validation in a later candidate frame.
    pub original_verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromiseState {
    VerifiedOriginal,
    VerifiedResitedVia,
    PendingReported,
    UnsupportedMultiplePhysicalLands,
    OriginalAnchorUnverified,
    NoOwnedOrdinaryVia,
    ViaRuleOrReturnUnresolved,
    OriginalPadAccessUnresolved,
    SurfaceAdaptationUnsupported,
}

#[derive(Clone, Debug)]
pub struct PromiseCheck {
    pub original: EscapePromise,
    pub state: PromiseState,
    pub verified_anchor: Option<Anchor>,
    pub owned_candidates_checked: usize,
}

#[derive(Clone, Debug)]
pub struct EscapeChecks {
    pub promises: Vec<PromiseCheck>,
    pub pending: Vec<PadReference>,
}

/// Captured from FanoutResult fields only; no track-endpoint reconstruction.
#[derive(Clone, Debug)]
pub struct EscapePromiseLedger {
    source: PhysicalBoard,
    original_fanout: FanoutResult,
    declared_temporary_keepouts: Vec<copper_physical::CopperKeepout>,
    promises: Vec<EscapePromise>,
    pending: Vec<PadReference>,
}

fn pad_binding(board: &PhysicalBoard, pad: &PadReference) -> Result<(String, usize)> {
    let placement = board
        .placements
        .iter()
        .find(|placement| placement.reference == pad.component)
        .ok_or_else(|| Error::Value("escape promise names an unplaced original component".to_owned()))?;
    let footprint = board
        .footprints
        .get(&placement.footprint)
        .ok_or_else(|| Error::Value("escape promise has no original footprint".to_owned()))?;
    let lands = footprint.pads.iter().filter(|land| land.number == pad.pad).count();
    if lands == 0 {
        return Err(Error::Value("escape promise names a missing original pad".to_owned()));
    }
    let nets: Vec<_> = board.nets.iter().filter(|net| net.pads.contains(pad)).collect();
    if nets.len() != 1 {
        return Err(Error::Value("escape promise requires one exact original electrical pad binding".to_owned()));
    }
    Ok((nets[0].name.clone(), lands))
}

fn unique_via<'a>(board: &'a PhysicalBoard, net: &str, point: copper_physical::Point) -> Option<&'a Via> {
    let mut vias = board.vias.iter().filter(|via| via.net == net && via.position == point);
    let first = vias.next()?;
    vias.next().is_none().then_some(first)
}

fn via_rule_permits(board: &PhysicalBoard, net: &str, via: &Via) -> bool {
    let rule = board.net_routing_rules.iter().find(|rule| rule.net == net);
    if rule.is_some_and(|rule| rule.max_vias == Some(0)) {
        return false;
    }
    let layers = &board.stackup.copper_layers;
    let (Some(a), Some(b)) = (
        layers.iter().position(|layer| *layer == via.from_layer),
        layers.iter().position(|layer| *layer == via.to_layer),
    ) else {
        return false;
    };
    copper_pcb::zones::routing_layers(board, net, rule).iter().any(|layer| layers[a.min(b)..=a.max(b)].contains(layer))
        && crate::return_paths::existing_returns_permit(board, rule, via)
}

fn anchor_verified(
    board: &PhysicalBoard,
    pad: &PadReference,
    net: &str,
    anchor: &Anchor,
    clearance: &RoutingClearanceIndex<'_>,
    retained: Option<&PhysicalBoard>,
) -> Result<bool> {
    // Reachability is established on retained fixed copper, while collision
    // and return checks use the complete candidate context. New mutable
    // same-net copper cannot invent the promised pad's retained launch path.
    let retained = retained.unwrap_or(board);
    match anchor {
        Anchor::Point(point) => {
            let Some(via) = unique_via(board, net, *point) else {
                return Ok(false);
            };
            Ok(via_rule_permits(board, net, via)
                && verified_fanout_path(retained, pad, net, *point, clearance)?.is_some())
        }
        Anchor::Access(access) => {
            let placement = board
                .placements
                .iter()
                .find(|placement| placement.reference == pad.component)
                .ok_or_else(|| Error::Value("original escape pad is no longer placed".to_owned()))?;
            let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
            if access.layer != side {
                let Some(via) = unique_via(board, net, access.launch_position) else {
                    return Ok(false);
                };
                if !via_rule_permits(board, net, via) {
                    return Ok(false);
                }
            }
            Ok(verified_routing_access(retained, pad, net, access, clearance)?.is_some())
        }
    }
}

impl EscapePromiseLedger {
    pub fn capture(source: &PhysicalBoard, fanout: &FanoutResult) -> Result<Self> {
        Self::capture_in_context(source, fanout, &[])
    }

    /// Admit an explicitly derived producer context while proving original
    /// pad anchors against the untouched RAW fanout board. Removing a declared
    /// temporary halo from admission cannot manufacture original access proof.
    /// Unknown masks and source-owned non-mask contract changes stay rejected.
    pub fn capture_in_context(
        source: &PhysicalBoard,
        raw_fanout: &FanoutResult,
        declared_temporary_keepouts: &[copper_physical::CopperKeepout],
    ) -> Result<Self> {
        let baseline = AdmittedFixedPrefix::admit(source, &[])?;
        let derived = crate::fixed_prefix::strip_declared_temporary_keepouts(
            baseline.board(),
            &raw_fanout.board,
            declared_temporary_keepouts,
        )?;
        crate::global_reserved_access::admit_fixed_prefix(baseline.board(), &derived)?;
        let fanout = raw_fanout;
        let clearance = RoutingClearanceIndex::new(&fanout.board)?;
        let mut promises = Vec::new();
        for (pad, anchor, provenance) in
            fanout
                .accesses
                .iter()
                .map(|(pad, point)| (pad, Anchor::Point(*point), AnchorProvenance::FanoutViaAccess))
                .chain(
                    fanout.surface_accesses.iter().flat_map(|accesses| accesses.iter()).map(|(pad, access)| {
                        (pad, Anchor::Access(access.clone()), AnchorProvenance::FanoutSurfaceAccess)
                    }),
                )
        {
            if promises.iter().any(|promise: &EscapePromise| promise.pad == *pad) {
                return Err(Error::Value(
                    "original fanout has conflicting via/surface provenance for one pad".to_owned(),
                ));
            }
            let (net, lands) = pad_binding(source, pad)?;
            let original_verified = lands == 1 && anchor_verified(&fanout.board, pad, &net, &anchor, &clearance, None)?;
            promises.push(EscapePromise {
                pad: pad.clone(),
                net,
                original_anchor: anchor,
                provenance,
                original_verified,
            });
        }
        Ok(Self {
            source: source.clone(),
            original_fanout: fanout.clone(),
            declared_temporary_keepouts: declared_temporary_keepouts.to_vec(),
            promises,
            pending: fanout.pending_pads.clone(),
        })
    }

    pub fn promises(&self) -> &[EscapePromise] {
        &self.promises
    }
    pub fn pending(&self) -> &[PadReference] {
        &self.pending
    }
    /// Original producer input for typed replay. Restore only through fresh
    /// `capture_in_context(source, original_fanout, declared_temporary_keepouts)`,
    /// never from saved verified booleans or a cleaned producer snapshot.
    pub fn original_fanout(&self) -> &FanoutResult {
        &self.original_fanout
    }
    pub fn source(&self) -> &PhysicalBoard {
        &self.source
    }
    pub fn declared_temporary_keepouts(&self) -> &[copper_physical::CopperKeepout] {
        &self.declared_temporary_keepouts
    }

    /// Finite verification of current explicit retained occurrences. The exact
    /// original barrel may remain owned by ViaLattice; replacement anchors
    /// must come from OrdinaryEscape. No coordinate-only ownership, mutable
    /// same-net path, new site or routing search can certify a local launch.
    pub fn validate(&self, current: &AdmittedFixedPrefix, complete: &PhysicalBoard) -> Result<EscapeChecks> {
        current.revalidate_for(&self.source)?;
        crate::global_reserved_access::admit_fixed_prefix(current.board(), complete)?;
        let clearance = RoutingClearanceIndex::new(complete)?;
        let mut owned_tracks = Vec::new();
        let mut owned_vias = Vec::new();
        let mut owned_lattice_vias = Vec::new();
        for range in current.ranges() {
            match range.role {
                FixedPrefixRole::OrdinaryEscape => {
                    owned_tracks.extend(current.board().tracks[range.tracks.clone()].iter());
                    owned_vias.extend(current.board().vias[range.vias.clone()].iter());
                }
                FixedPrefixRole::ViaLattice => {
                    owned_lattice_vias.extend(current.board().vias[range.vias.clone()].iter())
                }
            }
        }
        let mut checks = Vec::new();
        for promise in &self.promises {
            let mut check = PromiseCheck {
                original: promise.clone(),
                state: PromiseState::OriginalPadAccessUnresolved,
                verified_anchor: None,
                owned_candidates_checked: 0,
            };
            let (_, lands) = pad_binding(&self.source, &promise.pad)?;
            if self.pending.contains(&promise.pad) {
                check.state = PromiseState::PendingReported;
            } else if lands != 1 {
                check.state = PromiseState::UnsupportedMultiplePhysicalLands;
            } else if !promise.original_verified {
                check.state = PromiseState::OriginalAnchorUnverified;
            } else {
                match &promise.original_anchor {
                    Anchor::Point(original) => {
                        check.state = PromiseState::NoOwnedOrdinaryVia;
                        let mut candidates = Vec::new();
                        if let Some(original_via) = unique_via(&self.original_fanout.board, &promise.net, *original) {
                            // Retain only the exact original physical barrel,
                            // not another shape at matching coordinates. Its
                            // current occurrence must be explicitly lattice-owned.
                            candidates.extend(owned_lattice_vias.iter().copied().filter(|via| *via == original_via));
                        }
                        let ordinary: Vec<_> =
                            owned_vias.iter().copied().filter(|via| via.net == promise.net).collect();
                        candidates.extend(ordinary.iter().copied().filter(|via| via.position == *original));
                        candidates.extend(ordinary.iter().copied().filter(|via| via.position != *original));
                        // Preserve current declaration order, preferring an actual
                        // retained original point when it is still explicitly owned.
                        for via in candidates {
                            check.owned_candidates_checked += 1;
                            if unique_via(complete, &promise.net, via.position) != Some(via)
                                || !via_rule_permits(complete, &promise.net, via)
                            {
                                check.state = PromiseState::ViaRuleOrReturnUnresolved;
                                continue;
                            }
                            let anchor = Anchor::Point(via.position);
                            if anchor_verified(
                                complete,
                                &promise.pad,
                                &promise.net,
                                &anchor,
                                &clearance,
                                Some(current.board()),
                            )? {
                                check.state = if via.position == *original {
                                    PromiseState::VerifiedOriginal
                                } else {
                                    PromiseState::VerifiedResitedVia
                                };
                                check.verified_anchor = Some(anchor);
                                break;
                            }
                            check.state = PromiseState::OriginalPadAccessUnresolved;
                        }
                    }
                    Anchor::Access(access) => {
                        // This prototype checks the original owned path only.
                        // Changed surface paths require an explicit new producer
                        // record; do not guess a port from same-net endpoints.
                        let mut remaining = owned_tracks.clone();
                        let owned = access.path.iter().all(|track| {
                            remaining
                                .iter()
                                .position(|other| **other == *track)
                                .map(|index| remaining.remove(index))
                                .is_some()
                        });
                        if !owned
                            || (access.layer != pad_side(&self.source, &promise.pad)?
                                && !owned_vias
                                    .iter()
                                    .any(|via| via.net == promise.net && via.position == access.launch_position))
                        {
                            check.state = PromiseState::SurfaceAdaptationUnsupported;
                        } else if anchor_verified(
                            complete,
                            &promise.pad,
                            &promise.net,
                            &promise.original_anchor,
                            &clearance,
                            Some(current.board()),
                        )? {
                            check.state = PromiseState::VerifiedOriginal;
                            check.verified_anchor = Some(promise.original_anchor.clone());
                        }
                    }
                }
            }
            checks.push(check);
        }
        Ok(EscapeChecks { promises: checks, pending: self.pending.clone() })
    }
}

fn pad_side(board: &PhysicalBoard, pad: &PadReference) -> Result<CopperLayer> {
    let placement = board
        .placements
        .iter()
        .find(|placement| placement.reference == pad.component)
        .ok_or_else(|| Error::Value("original escape pad is no longer placed".to_owned()))?;
    Ok(if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back })
}

#[cfg(test)]
#[path = "escape_promise_tests.rs"]
mod tests;
