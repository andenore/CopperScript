//! Fresh read-only admission of declared ordinary bypass contributions
//! (`pcbir.bypass_reservations`).
//!
//! This recognizes existing source-associated geometry. It never generates
//! copper, accepts routing flags as proof, or grants ownership of an immutable
//! prefix. Python's object-identity checks on edges compare values: every
//! declared edge must occur exactly once on the board, so a value names one
//! occurrence.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PhysicalBoard, RouteKind, TrackSegment, Via};

use crate::Result;
use crate::decoupling_paths::{DecouplingCrossLayerWitness, PathEdge};

/// The bypass-only `FanoutResult` a reservation is: the frame plus its
/// generated copper, all of it declared bypass copper.
#[derive(Clone, Debug)]
pub struct BypassReservation {
    pub board: PhysicalBoard,
    pub created_tracks: Vec<TrackSegment>,
    pub created_vias: Vec<Via>,
    pub added_track_count: i64,
    pub added_via_count: i64,
    pub decoupling_tracks: Vec<TrackSegment>,
    pub decoupling_vias: Vec<Via>,
    pub decoupling_witnesses: Vec<DecouplingCrossLayerWitness>,
}

impl BypassReservation {
    /// `FanoutResult(frame + copper, ..., decoupling_*=copper, decoupling_witnesses=witnesses)`.
    pub fn new(
        frame: &PhysicalBoard,
        tracks: &[TrackSegment],
        vias: &[Via],
        witnesses: &[DecouplingCrossLayerWitness],
    ) -> Self {
        let mut board = frame.clone();
        board.tracks.extend(tracks.iter().cloned());
        board.vias.extend(vias.iter().cloned());
        BypassReservation {
            board,
            created_tracks: tracks.to_vec(),
            created_vias: vias.to_vec(),
            added_track_count: tracks.len() as i64,
            added_via_count: vias.len() as i64,
            decoupling_tracks: tracks.to_vec(),
            decoupling_vias: vias.to_vec(),
            decoupling_witnesses: witnesses.to_vec(),
        }
    }
}

fn counts<T: Eq + std::hash::Hash>(items: &[T]) -> IndexMap<&T, i64> {
    let mut result = IndexMap::new();
    for item in items {
        *result.entry(item).or_insert(0) += 1;
    }
    result
}

fn within<T: Eq + std::hash::Hash>(small: &[T], large: &[T]) -> bool {
    let large = counts(large);
    counts(small).iter().all(|(item, count)| large.get(item).copied().unwrap_or(0) >= *count)
}

/// `restore_bypass_reservation_edges`: every contribution edge and witness
/// step must name exactly one occurrence on the board.
///
/// By default only the bypass-only prefix is supported; critical replay
/// (`allow_ordinary`) may restore a mixed ordinary prefix, whose full
/// ordinary ownership, source and physical checks still run afterwards.
pub fn restore_bypass_reservation_edges(
    reserved: &BypassReservation,
    allow_ordinary: bool,
) -> std::result::Result<(), String> {
    if !within(&reserved.decoupling_tracks, &reserved.created_tracks)
        || !within(&reserved.decoupling_vias, &reserved.created_vias)
        || !allow_ordinary
            && (counts(&reserved.created_tracks) != counts(&reserved.decoupling_tracks)
                || counts(&reserved.created_vias) != counts(&reserved.decoupling_vias))
    {
        return Err("critical replay requires an exact bypass-only reservation".to_owned());
    }
    let tracks = counts(&reserved.board.tracks);
    let vias = counts(&reserved.board.vias);
    let unique = |edge: &PathEdge| match edge {
        PathEdge::Track(track) => tracks.get(track) == Some(&1),
        PathEdge::Via(via) => vias.get(via) == Some(&1),
    };
    let ambiguous = "bypass replay has a missing or ambiguous original edge occurrence".to_owned();
    for witness in &reserved.decoupling_witnesses {
        for path in [&witness.supply_path, &witness.return_path] {
            if !path.steps.iter().all(|step| unique(&step.edge)) {
                return Err(ambiguous);
            }
        }
    }
    let all_tracks = reserved.created_tracks.iter().chain(reserved.decoupling_tracks.iter());
    let all_vias = reserved.created_vias.iter().chain(reserved.decoupling_vias.iter());
    if all_tracks.into_iter().any(|t| tracks.get(t) != Some(&1))
        || all_vias.into_iter().any(|v| vias.get(v) != Some(&1))
    {
        return Err(ambiguous);
    }
    Ok(())
}

/// `verified_bypass_reservations(source, reserved)`: the exact witnessed
/// generated feeds, or no exemptions and a reason.
pub fn verified_bypass_reservations(
    source: &PhysicalBoard,
    reserved: &BypassReservation,
) -> Result<(Vec<TrackSegment>, Vec<Via>, Option<String>)> {
    let reject = |reason: &str| Ok((Vec::new(), Vec::new(), Some(reason.to_owned())));
    let (created_tracks, created_vias) = (&reserved.created_tracks, &reserved.created_vias);
    let (declared_tracks, declared_vias) = (&reserved.decoupling_tracks, &reserved.decoupling_vias);
    if reserved.added_track_count != created_tracks.len() as i64
        || reserved.added_via_count != created_vias.len() as i64
    {
        return reject("bypass reservation counters disagree with generated ownership");
    }
    let mut intent = reserved.board.clone();
    intent.tracks = source.tracks.clone();
    intent.vias = source.vias.clone();
    let prefix_tracks: Vec<TrackSegment> = source.tracks.iter().chain(created_tracks.iter()).cloned().collect();
    let prefix_vias: Vec<Via> = source.vias.iter().chain(created_vias.iter()).cloned().collect();
    if intent != *source || reserved.board.tracks != prefix_tracks || reserved.board.vias != prefix_vias {
        return reject("bypass reservations changed source intent or exact ordered copper prefix");
    }
    let rules: IndexMap<&str, _> = source.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let critical_nets: IndexSet<&str> =
        source.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.as_str()).collect();
    if created_tracks
        .iter()
        .map(|t| t.net.as_str())
        .chain(created_vias.iter().map(|v| v.net.as_str()))
        .any(|net| critical_nets.contains(net))
    {
        return reject("ordinary bypass reservations contain critical-kind generated copper");
    }
    let (generated, source_counts) = (counts(created_tracks), counts(&source.tracks));
    if counts(declared_tracks).len() != declared_tracks.len()
        || declared_tracks.iter().any(|t| generated.get(t) != Some(&1) || source_counts.contains_key(t))
    {
        return reject("declared bypass tracks have missing, immutable or ambiguous ownership");
    }
    let (generated, source_counts) = (counts(created_vias), counts(&source.vias));
    if counts(declared_vias).len() != declared_vias.len()
        || declared_vias.iter().any(|v| generated.get(v) != Some(&1) || source_counts.contains_key(v))
    {
        return reject("declared bypass vias have missing, immutable or ambiguous ownership");
    }
    let nets: IndexSet<String> = source.decoupling_links.iter().map(|link| link.net.clone()).collect();
    let original = crate::bypass_links::verified_bypass_links(source, &nets, None, None, &[])?;
    let (mut paths, mut via_paths) = (IndexMap::new(), IndexMap::new());
    let current = crate::bypass_links::verified_bypass_links(
        &reserved.board,
        &nets,
        Some(&mut paths),
        Some(&mut via_paths),
        &[],
    )?;
    if !original.iter().all(|link| current.contains(link)) {
        return reject("ordinary reservations obstruct a verified source bypass association");
    }
    let ordinary: IndexSet<_> = current
        .iter()
        .filter(|link| rules.get(link.net.as_str()).is_none_or(|rule| rule.kind == RouteKind::General))
        .cloned()
        .collect();
    let (admitted, reason) = crate::decoupling_paths::refresh_crosslayer_witnesses(
        &reserved.board,
        &reserved.decoupling_witnesses,
        None,
        &IndexSet::new(),
    )?;
    if let Some(reason) = reason {
        return reject(&format!("invalid original admitted bypass witness: {reason}"));
    }
    if admitted.iter().any(|w| !ordinary.contains(&w.link)) {
        return reject("original admitted bypass witness has no current complete ordinary association");
    }
    let mut witnessed_tracks: IndexSet<TrackSegment> = IndexSet::new();
    let mut witnessed_vias: IndexSet<Via> = IndexSet::new();
    for (link, path) in &paths {
        if ordinary.contains(link) {
            witnessed_tracks.extend(path.iter().filter(|t| t.net == link.net || t.net == link.return_net).cloned());
        }
    }
    for (link, path) in &via_paths {
        if ordinary.contains(link) {
            witnessed_vias.extend(path.iter().filter(|v| v.net == link.net || v.net == link.return_net).cloned());
        }
    }
    for witness in &admitted {
        for path in [&witness.supply_path, &witness.return_path] {
            for step in &path.steps {
                match &step.edge {
                    PathEdge::Track(track) => {
                        witnessed_tracks.insert(track.clone());
                    }
                    PathEdge::Via(via) => {
                        witnessed_vias.insert(via.clone());
                    }
                }
            }
        }
    }
    if declared_tracks.iter().any(|t| !witnessed_tracks.contains(t))
        || declared_vias.iter().any(|v| !witnessed_vias.contains(v))
    {
        return reject("declared generated bypass copper is absent from a fresh source witness");
    }
    Ok((declared_tracks.clone(), declared_vias.clone(), None))
}

/// `flow._bypass_copper_reservations`: fresh admission of the exact planned
/// bypass contribution on the macro frame of `source`, without routing.
pub fn bypass_copper_reservations(
    source: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    witnesses: &[DecouplingCrossLayerWitness],
) -> Result<(Option<BypassReservation>, Option<String>)> {
    let frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(source.clone())?)?;
    let reserved = BypassReservation::new(&frame, tracks, vias, witnesses);
    if let Err(reason) = restore_bypass_reservation_edges(&reserved, false) {
        return Ok((None, Some(reason)));
    }
    let (_, _, reason) = verified_bypass_reservations(&frame, &reserved)?;
    Ok(match reason {
        None => (Some(reserved), None),
        Some(reason) => (None, Some(reason)),
    })
}

/// `rebind_bypass_keepout_removal`: re-prove unchanged bypass copper after an
/// explicit keepout-only removal.
///
/// The original reservation must already be admissible. No source rule,
/// pose, copper occurrence or ownership changes are allowed. Paths are
/// derived again from the new board; an old context digest is never
/// overwritten or waived. Failure returns no replacement and leaves the
/// original value untouched.
pub fn rebind_bypass_keepout_removal(
    source: &PhysicalBoard,
    reserved: &BypassReservation,
    updated_source: &PhysicalBoard,
) -> Result<(Option<BypassReservation>, Option<String>)> {
    let reject = |reason: String| Ok((None, Some(reason)));
    let mut intent = updated_source.clone();
    intent.copper_keepouts = source.copper_keepouts.clone();
    if intent != *source {
        return reject("keepout rebind changed non-keepout source intent".to_owned());
    }
    let (before, after) = (&source.copper_keepouts, &updated_source.copper_keepouts);
    // Keepouts are not hashable; Python's set checks, by equality.
    let unique =
        |items: &[copper_physical::CopperKeepout]| items.iter().enumerate().all(|(i, item)| !items[..i].contains(item));
    let ordered: Vec<_> = before.iter().filter(|k| after.contains(k)).collect();
    if !unique(before)
        || !unique(after)
        || !after.iter().all(|k| before.contains(k))
        || ordered != after.iter().collect::<Vec<_>>()
    {
        return reject("keepout rebind is not an ordered removal".to_owned());
    }
    let materialize = |board: &PhysicalBoard| {
        copper_pcb::hard_macros::macro_source(board.clone()).and_then(copper_drc::macros::materialize_hard_macros)
    };
    let (frame, updated) = match (materialize(source), materialize(updated_source)) {
        (Ok(frame), Ok(updated)) => (frame, updated),
        (Err(crate::Error::Value(reason)), _) | (_, Err(crate::Error::Value(reason))) => return reject(reason),
        (Err(error), _) | (_, Err(error)) => return Err(error),
    };
    if let Err(reason) = restore_bypass_reservation_edges(reserved, false) {
        return reject(reason);
    }
    let (_, _, reason) = verified_bypass_reservations(&frame, reserved)?;
    if let Some(reason) = reason {
        return reject(format!("keepout rebind original rejected: {reason}"));
    }
    let mut candidate = reserved.clone();
    candidate.board = updated.clone();
    candidate.board.tracks.extend(reserved.created_tracks.iter().cloned());
    candidate.board.vias.extend(reserved.created_vias.iter().cloned());
    // Rematerialization changes immutable edge identities, never their values.
    if let Err(reason) = restore_bypass_reservation_edges(&candidate, false) {
        return reject(reason);
    }
    let index = crate::RoutingClearanceIndex::new(&candidate.board)?;
    let mut witnesses = Vec::new();
    for old in &candidate.decoupling_witnesses {
        let measured = crate::decoupling_paths::verify_crosslayer_decoupling(
            &candidate.board,
            &old.link,
            Some(&old.return_target),
            10_000,
            Some(&index),
        )?;
        match measured.witness {
            Some(witness) => witnesses.push(witness),
            None => return reject(format!("keepout rebind path rejected: {}", measured.reason)),
        }
    }
    drop(index);
    candidate.decoupling_witnesses = witnesses;
    let (_, _, reason) = verified_bypass_reservations(&updated, &candidate)?;
    Ok(match reason {
        None => (Some(candidate), None),
        Some(reason) => (None, Some(reason)),
    })
}

// ---------------------------------------------------------------------------
// Saved reservations

/// A saved `fanout.FanoutResult` as the bypass reservation critical replay
/// restores (`restore_bypass_reservation_edges(..., allow_ordinary=True)`):
/// decoding errors, then (inner) the `ValueError` of that later restoration.
/// Fanout analysis fields a reservation never carries are not decoded.
pub fn saved_reservation(value: &copper_state::StateValue) -> Result<std::result::Result<BypassReservation, String>> {
    use copper_pcb::state_codec::{Fields, integer, pad_reference, point, set_items, track, via};
    use copper_state::StateValue;
    const NAME: &str = "pcbir.fanout:FanoutResult";
    let fields = Fields::of(value, NAME)?;
    fields.allow(&[
        "board",
        "accesses",
        "pending_pads",
        "added_track_count",
        "added_via_count",
        "created_vias",
        "pin_analysis",
        "created_tracks",
        "assignment",
        "boundary_accesses",
        "surface_accesses",
        "decoupling_pending",
        "decoupling_tracks",
        "decoupling_vias",
        "decoupling_witnesses",
        "decoupling_pending_links",
        "late_bypass",
    ])?;
    let missing: Vec<&str> = ["board", "accesses", "pending_pads", "added_track_count", "added_via_count"]
        .into_iter()
        .filter(|name| fields.get(name).is_none())
        .collect();
    if !missing.is_empty() {
        return copper_pcb::not_implemented("a saved FanoutResult without its required fields (Python crashes)");
    }
    for name in ["pin_analysis", "decoupling_pending", "decoupling_pending_links"] {
        if fields.get(name).is_some_and(|items| !matches!(items, StateValue::Tuple(items) if items.is_empty())) {
            return copper_pcb::not_implemented(&format!("a saved FanoutResult with {name}"));
        }
    }
    for name in ["assignment", "late_bypass"] {
        if fields.get(name).is_some_and(|item| !matches!(item, StateValue::Null)) {
            return copper_pcb::not_implemented(&format!("a saved FanoutResult with {name}"));
        }
    }
    let board = copper_pcb::state_codec::board(fields.required("board")?)?;
    // An access mapping (`$map`), only checked for emptiness after decoding.
    let accesses = |name: &str, read_value: &dyn Fn(&StateValue) -> Result<()>| -> Result<usize> {
        match fields.get(name) {
            None | Some(StateValue::Null) => Ok(0),
            Some(StateValue::Map { entries, .. }) => {
                for (key, item) in entries {
                    pad_reference(key)?;
                    read_value(item)?;
                }
                Ok(entries.len())
            }
            Some(_) => copper_pcb::not_implemented(&format!("a saved FanoutResult whose {name} is not a mapping")),
        }
    };
    let routed =
        |_: &StateValue| -> Result<()> { copper_pcb::not_implemented("a saved FanoutResult with routing accesses") };
    let mut access_count = accesses("accesses", &|item| point(item).map(|_| ()))?;
    access_count += accesses("surface_accesses", &routed)?;
    access_count += accesses("boundary_accesses", &routed)?;
    let pending: Vec<PadReference> =
        set_items(fields.required("pending_pads")?)?.iter().map(pad_reference).collect::<Result<_>>()?;
    let untyped = || Ok(Err("critical replay has untyped bypass contribution edges".to_owned()));
    // An immutable tuple of edges, else Python's "untyped" refusal.
    let edges = |name: &str| -> Result<Option<&[StateValue]>> {
        match fields.get(name) {
            None => Ok(Some(&[])),
            Some(StateValue::Tuple(items)) => Ok(Some(items.as_slice())),
            Some(_) => Ok(None),
        }
    };
    let mut decoded_tracks = Vec::new();
    for name in ["created_tracks", "decoupling_tracks"] {
        let Some(items) = edges(name)? else { return untyped() };
        let mut tracks = Vec::new();
        for item in items {
            match copper_pcb::state_codec::type_name(item) {
                Some("pcbir.physical:TrackSegment") => tracks.push(track(item)?),
                _ => return untyped(),
            }
        }
        decoded_tracks.push(tracks);
    }
    let mut decoded_vias = Vec::new();
    for name in ["created_vias", "decoupling_vias"] {
        let Some(items) = edges(name)? else { return untyped() };
        let mut vias = Vec::new();
        for item in items {
            match copper_pcb::state_codec::type_name(item) {
                Some("pcbir.physical:Via") => vias.push(via(item)?),
                _ => return untyped(),
            }
        }
        decoded_vias.push(vias);
    }
    let untyped_witness = || Ok(Err("critical replay has an untyped bypass witness".to_owned()));
    let witnesses = match fields.get("decoupling_witnesses") {
        None => Vec::new(),
        Some(StateValue::Tuple(items)) => {
            let mut witnesses = Vec::new();
            for item in items {
                match saved_witness(item)? {
                    Some(witness) => witnesses.push(witness),
                    None => return untyped_witness(),
                }
            }
            witnesses
        }
        Some(_) => return untyped_witness(),
    };
    let decoupling_vias = decoded_vias.pop().expect("two sequences");
    let created_vias = decoded_vias.pop().expect("two sequences");
    let decoupling_tracks = decoded_tracks.pop().expect("two sequences");
    let created_tracks = decoded_tracks.pop().expect("two sequences");
    let reserved = BypassReservation {
        board,
        created_tracks,
        created_vias,
        added_track_count: integer(fields.required("added_track_count")?)?,
        added_via_count: integer(fields.required("added_via_count")?)?,
        decoupling_tracks,
        decoupling_vias,
        decoupling_witnesses: witnesses,
    };
    if access_count != 0 || !pending.is_empty() {
        return Ok(Err("critical replay requires an exact bypass-only reservation".to_owned()));
    }
    Ok(restore_bypass_reservation_edges(&reserved, true).map(|()| reserved))
}

use copper_pcb::board::PadReference;

/// A saved `DecouplingCrossLayerWitness`, or `None` when it is not one (or
/// one of its paths or steps is untyped).
fn saved_witness(value: &copper_state::StateValue) -> Result<Option<DecouplingCrossLayerWitness>> {
    use copper_pcb::state_codec::{Fields, pad_reference, text, type_name};
    if type_name(value) != Some("pcbir.decoupling_paths:DecouplingCrossLayerWitness") {
        return Ok(None);
    }
    let fields = Fields::of(value, "pcbir.decoupling_paths:DecouplingCrossLayerWitness")?;
    fields.allow(&[
        "link",
        "supply_path",
        "return_target",
        "return_path",
        "context_digest",
        "board_digest",
        "qualification",
    ])?;
    let link = saved_link(fields.required("link")?)?;
    let (Some(supply_path), Some(return_path)) =
        (saved_path(fields.required("supply_path")?)?, saved_path(fields.required("return_path")?)?)
    else {
        return Ok(None);
    };
    Ok(Some(DecouplingCrossLayerWitness {
        link,
        supply_path,
        return_target: pad_reference(fields.required("return_target")?)?,
        return_path,
        context_digest: text(fields.required("context_digest")?)?,
        board_digest: text(fields.required("board_digest")?)?,
        qualification: match fields.get("qualification") {
            Some(value) => text(value)?,
            None => crate::decoupling_paths::QUALIFICATION.to_owned(),
        },
    }))
}

fn saved_link(value: &copper_state::StateValue) -> Result<copper_pcb::board::DecouplingLink> {
    use copper_pcb::state_codec::{Fields, pad_reference, text};
    let fields = Fields::of(value, "pcbir.physical:DecouplingLink")?;
    fields.allow(&["capacitor", "target", "return_pad", "net", "return_net"])?;
    Ok(copper_pcb::board::DecouplingLink {
        capacitor: pad_reference(fields.required("capacitor")?)?,
        target: pad_reference(fields.required("target")?)?,
        return_pad: pad_reference(fields.required("return_pad")?)?,
        net: text(fields.required("net")?)?,
        return_net: text(fields.required("return_net")?)?,
    })
}

/// A saved `DecouplingCopperPath` whose steps are typed, else `None`.
fn saved_path(value: &copper_state::StateValue) -> Result<Option<crate::decoupling_paths::DecouplingCopperPath>> {
    use crate::decoupling_paths::{DecouplingCopperPath, DecouplingPathNode, DecouplingPathStep};
    use copper_pcb::state_codec::{Fields, layer, pad_reference, point, text, track, type_name, via};
    use copper_state::StateValue;
    if type_name(value) != Some("pcbir.decoupling_paths:DecouplingCopperPath") {
        return Ok(None);
    }
    let fields = Fields::of(value, "pcbir.decoupling_paths:DecouplingCopperPath")?;
    fields.allow(&["start", "end", "net", "steps"])?;
    let StateValue::Tuple(items) = fields.required("steps")? else { return Ok(None) };
    let node = |value: &StateValue| -> Result<DecouplingPathNode> {
        let fields = Fields::of(value, "pcbir.decoupling_paths:DecouplingPathNode")?;
        fields.allow(&["position", "layer"])?;
        Ok(DecouplingPathNode {
            position: point(fields.required("position")?)?,
            layer: layer(fields.required("layer")?)?,
        })
    };
    let mut steps = Vec::new();
    for item in items {
        if type_name(item) != Some("pcbir.decoupling_paths:DecouplingPathStep") {
            return Ok(None);
        }
        let fields = Fields::of(item, "pcbir.decoupling_paths:DecouplingPathStep")?;
        fields.allow(&["edge", "start", "end"])?;
        let edge = fields.required("edge")?;
        let edge = match type_name(edge) {
            Some("pcbir.physical:TrackSegment") => PathEdge::Track(track(edge)?),
            Some("pcbir.physical:Via") => PathEdge::Via(via(edge)?),
            _ => return Ok(None),
        };
        steps.push(DecouplingPathStep {
            edge,
            start: node(fields.required("start")?)?,
            end: node(fields.required("end")?)?,
        });
    }
    Ok(Some(DecouplingCopperPath {
        start: pad_reference(fields.required("start")?)?,
        end: pad_reference(fields.required("end")?)?,
        net: text(fields.required("net")?)?,
        steps,
    }))
}
