//! Bounded transactional rerouting of a small net subset and its blocker cone
//! (`pcbir.dependency_reroute`).
//!
//! The native filled-copper feedback (`filled_feedback`) reserves a bridge,
//! then reroutes the ordinary nets it displaced with the unified router. Every
//! retry starts afresh with all unrelated area copper restored; probe paths only
//! name blockers and are never accepted as geometry.

use std::collections::BTreeSet;
use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};
use sha2::Digest;

use copper_json::Value;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};

use crate::Result;
use crate::critical::CriticalRoutingStatus;
use crate::detailed::{DetailedRoutingResult, DetailedRoutingStatus};
use crate::flow::{PhysicalFlowStatus, RoutingPipelineResult};
use crate::progress::{emit, int, text};
use crate::route_closure::close_detailed_lands;
use crate::routing_clearance::RoutingClearanceIndex;

/// Python's `Counter` of hashable copper objects, in first-seen order.
pub type Multiset<T> = IndexMap<T, i64>;

pub fn multiset<'a, T: Clone + Eq + Hash + 'a>(items: impl IntoIterator<Item = &'a T>) -> Multiset<T> {
    let mut counts = Multiset::new();
    for item in items {
        *counts.entry(item.clone()).or_insert(0) += 1;
    }
    counts
}

/// `Counter(a) - Counter(b)`: only positive counts remain.
pub fn subtract<T: Clone + Eq + Hash>(first: &Multiset<T>, second: &Multiset<T>) -> Multiset<T> {
    first
        .iter()
        .filter_map(|(item, count)| {
            let left = count - second.get(item).copied().unwrap_or(0);
            (left > 0).then(|| (item.clone(), left))
        })
        .collect()
}

/// `Counter` equality (missing counts are zero).
pub fn same_counts<T: Eq + Hash>(first: &Multiset<T>, second: &Multiset<T>) -> bool {
    first.iter().filter(|(_, c)| **c > 0).all(|(item, count)| second.get(item) == Some(count))
        && second.iter().filter(|(_, c)| **c > 0).all(|(item, count)| first.get(item) == Some(count))
}

/// `with_preserved_area`: `fixed` plus the given area copper of every net
/// outside `changed`.
pub fn with_preserved_area(
    fixed: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    changed: &BTreeSet<String>,
) -> PhysicalBoard {
    let mut board = fixed.clone();
    board.tracks.extend(tracks.iter().filter(|track| !changed.contains(&track.net)).cloned());
    board.vias.extend(vias.iter().filter(|via| !changed.contains(&via.net)).cloned());
    board
}

/// `unaffected_copper`: track and via multisets of every net outside `changed`.
pub fn unaffected_copper(board: &PhysicalBoard, changed: &BTreeSet<String>) -> (Multiset<TrackSegment>, Multiset<Via>) {
    (
        multiset(board.tracks.iter().filter(|track| !changed.contains(&track.net))),
        multiset(board.vias.iter().filter(|via| !changed.contains(&via.net))),
    )
}

fn same_unaffected(first: &PhysicalBoard, second: &PhysicalBoard, changed: &BTreeSet<String>) -> bool {
    let (a, b) = (unaffected_copper(first, changed), unaffected_copper(second, changed));
    same_counts(&a.0, &b.0) && same_counts(&a.1, &b.1)
}

/// A successful local transaction: the board, the subset reroute (None when
/// nothing was displaced), the final changed set and the expansions used.
pub type LocalReroute = (PhysicalBoard, Option<DetailedRoutingResult>, BTreeSet<String>, i64);

fn names_value(names: &BTreeSet<String>) -> Value {
    Value::Array(names.iter().map(|name| text(name)).collect())
}

/// `reroute_local_dependencies`: expand an exact blocker cone, never accepting
/// relaxed probe geometry.
#[allow(clippy::too_many_arguments)]
pub fn reroute_local_dependencies(
    initial: &RoutingPipelineResult,
    fixed: &PhysicalBoard,
    movable_tracks: &[TrackSegment],
    movable_vias: &[Via],
    changed: &BTreeSet<String>,
    eligible: &BTreeSet<String>,
    maximum_local_blockers: i64,
    maximum_dependency_expansions: i64,
    mut on_progress: Option<crate::progress::ProgressCallback<'_>>,
) -> Result<Option<LocalReroute>> {
    if !changed.is_subset(eligible) || changed.len() as i64 > maximum_local_blockers {
        return Ok(None);
    }
    if changed.is_empty() {
        return Ok(Some((with_preserved_area(fixed, movable_tracks, movable_vias, changed), None, changed.clone(), 0)));
    }
    // A repair must not overwrite the main run's replay input artifact.
    let mut settings = initial.unified_options.clone();
    settings.save_inputs_path = None;
    let required = IndexMap::new();
    let search = |source: &PhysicalBoard,
                  names: &BTreeSet<String>,
                  kind: &str,
                  expansion: i64,
                  progress: &mut Option<crate::progress::ProgressCallback<'_>>|
     -> Result<DetailedRoutingResult> {
        emit(
            progress,
            "zone_subset_search",
            "started",
            vec![
                ("kind", text(kind)),
                ("affected_nets", names_value(names)),
                ("expansion", int(expansion)),
                ("router", text("unified")),
            ],
        );
        let only: IndexSet<String> = names.iter().cloned().collect();
        let result = crate::unified_router::route::route_unified(
            source,
            &initial.placement_and_global.global_route,
            &settings,
            None,
            Some(&only),
            Some(&required),
            None,
            crate::flow::reborrow(progress),
        )
        .map_err(|error| match error {
            crate::unified_error::Error::Value(message) => copper_pcb::Error::Value(message),
            other => copper_pcb::Error::NotImplemented(other.crash_line()),
        })?;
        let (result, _, _) = close_detailed_lands(&result, copper_drc::Policy::default())?;
        emit(
            progress,
            "zone_subset_search",
            "finished",
            vec![
                ("kind", text(kind)),
                ("affected_nets", names_value(names)),
                ("expansion", int(expansion)),
                ("overflow", int(result.metrics.total_conflict_overflow)),
                (
                    "failed_nets",
                    Value::Array(
                        result.nets.iter().filter(|item| !item.connected).map(|item| text(&item.net)).collect(),
                    ),
                ),
            ],
        );
        Ok(result)
    };
    let complete = |result: &DetailedRoutingResult, names: &BTreeSet<String>| {
        result.nets.iter().map(|item| &item.net).collect::<BTreeSet<_>>() == names.iter().collect::<BTreeSet<_>>()
            && result.nets.iter().all(|item| item.connected)
            && result.metrics.total_conflict_overflow == 0
    };
    let fixed_tracks = multiset(&fixed.tracks);
    let fixed_vias = multiset(&fixed.vias);
    let mut changed = changed.clone();
    for expansion in 0..=maximum_dependency_expansions {
        let trial = with_preserved_area(fixed, movable_tracks, movable_vias, &changed);
        let actual = search(&trial, &changed, "transaction", expansion, &mut on_progress)?;
        if complete(&actual, &changed) {
            // Even changed-net critical contacts remain owned by the prefix.
            if !subtract(&fixed_tracks, &multiset(&actual.board.tracks)).is_empty()
                || !subtract(&fixed_vias, &multiset(&actual.board.vias)).is_empty()
                || !same_unaffected(&actual.board, &trial, &changed)
            {
                return Ok(None);
            }
            return Ok(Some((actual.board.clone(), Some(actual), changed, expansion)));
        }
        if expansion == maximum_dependency_expansions {
            return Ok(None);
        }
        let probe = search(fixed, &changed, "probe_only", expansion, &mut on_progress)?;
        if !complete(&probe, &changed) {
            return Ok(None);
        }
        // Probe paths are proposals only. Ask the exact index which preserved
        // tracks/vias they intersect, including through-via holes/all copper layers.
        let mut clearance = RoutingClearanceIndex::new(fixed)?;
        for track in movable_tracks {
            if !changed.contains(&track.net) {
                clearance.add_track(track.clone(), false, None)?;
            }
        }
        for via in movable_vias {
            if !changed.contains(&via.net) {
                clearance.add_via(via.clone(), false, None)?;
            }
        }
        let mut blockers: BTreeSet<String> = BTreeSet::new();
        let mut locked = false;
        for track in subtract(&multiset(&probe.board.tracks), &fixed_tracks).keys() {
            if changed.contains(&track.net) {
                let (names, immutable) = clearance.blocking_track_nets(track)?;
                blockers.extend(names);
                locked |= immutable;
            }
        }
        for via in subtract(&multiset(&probe.board.vias), &fixed_vias).keys() {
            if changed.contains(&via.net) {
                let (names, immutable) = clearance.blocking_via_nets(via)?;
                blockers.extend(names);
                locked |= immutable;
            }
        }
        let enlarged: BTreeSet<String> = changed.union(&blockers).cloned().collect();
        if locked
            || enlarged == changed
            || !enlarged.is_subset(eligible)
            || enlarged.len() as i64 > maximum_local_blockers
        {
            return Ok(None);
        }
        changed = enlarged;
    }
    Ok(None)
}

fn layer_repr(layer: copper_physical::CopperLayer) -> String {
    let value = layer.value();
    let name = match value {
        "F.Cu" => "FRONT".to_owned(),
        "B.Cu" => "BACK".to_owned(),
        _ => format!("INTERNAL_{}", value.trim_start_matches("In").trim_end_matches(".Cu")),
    };
    format!("<CopperLayer.{name}: '{value}'>")
}

fn point_repr(point: copper_physical::geometry::Point) -> String {
    format!("Point(x_nm={}, y_nm={})", point.x_nm, point.y_nm)
}

fn tuple_repr(items: Vec<String>) -> String {
    match items.len() {
        1 => format!("({},)", items[0]),
        _ => format!("({})", items.join(", ")),
    }
}

/// `repr((fingerprint, board.tracks, board.vias))`.
fn copper_repr(fingerprint: &str, board: &PhysicalBoard) -> String {
    let string = copper_json::python_str_repr;
    let tracks = board
        .tracks
        .iter()
        .map(|t| {
            format!(
                "TrackSegment(net={}, start={}, end={}, width_nm={}, layer={})",
                string(&t.net),
                point_repr(t.start),
                point_repr(t.end),
                t.width_nm,
                layer_repr(t.layer)
            )
        })
        .collect();
    let vias = board
        .vias
        .iter()
        .map(|v| {
            format!(
                "Via(net={}, position={}, size_nm={}, drill_nm={}, from_layer={}, to_layer={}, technology={}, finish={})",
                string(&v.net),
                point_repr(v.position),
                v.size_nm,
                v.drill_nm,
                layer_repr(v.from_layer),
                layer_repr(v.to_layer),
                v.technology.as_deref().map_or_else(|| "None".to_owned(), string),
                string(&v.finish)
            )
        })
        .collect();
    format!("({}, {}, {})", string(fingerprint), tuple_repr(tracks), tuple_repr(vias))
}

/// `merge_local_detail`: preserve full-board metrics while replacing a
/// bounded net subset.
pub fn merge_local_detail(
    original: &RoutingPipelineResult,
    board: &PhysicalBoard,
    reroute: Option<&DetailedRoutingResult>,
) -> Result<RoutingPipelineResult> {
    let policy = copper_drc::Policy::default();
    let new: IndexMap<&str, &crate::detailed::DetailedNetResult> =
        reroute.map_or_else(IndexMap::new, |r| r.nets.iter().map(|item| (item.net.as_str(), item)).collect());
    let drc = crate::drc_report::run_physical_drc(board, policy)?;
    let opens: IndexSet<&str> = drc
        .findings()
        .iter()
        .filter(|finding| finding.code == "DRC-OPEN-NET")
        .flat_map(|finding| finding.nets.iter().map(String::as_str))
        .collect();
    let locked = original.plane_stitch.as_ref().map_or(&original.critical.board, |stitch| &stitch.board);
    let zone_nets = copper_pcb::zones::distribution_zone_nets(board);
    let tracks = subtract(
        &multiset(board.tracks.iter().filter(|track| !zone_nets.contains(&track.net))),
        &multiset(&locked.tracks),
    );
    let vias =
        subtract(&multiset(board.vias.iter().filter(|via| !zone_nets.contains(&via.net))), &multiset(&locked.vias));
    let nets: Vec<crate::detailed::DetailedNetResult> = original
        .detailed
        .nets
        .iter()
        .map(|item| {
            let source = new.get(item.net.as_str()).copied().unwrap_or(item);
            let mut merged = source.clone();
            merged.connected = source.connected && !opens.contains(item.net.as_str());
            merged.track_count = tracks.iter().filter(|(t, _)| t.net == item.net).map(|(_, c)| c).sum();
            merged.via_count = vias.iter().filter(|(v, _)| v.net == item.net).map(|(_, c)| c).sum();
            merged.length_nm = tracks
                .iter()
                .filter(|(t, _)| t.net == item.net)
                .map(|(t, count)| {
                    ((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64).round_ties_even()
                        as i64
                        * count
                })
                .sum();
            merged
        })
        .collect();
    let mut metrics = original.detailed.metrics.clone();
    metrics.routed_net_count = nets.iter().filter(|item| item.connected).count() as i64;
    metrics.unrouted_net_count = nets.iter().filter(|item| !item.connected).count() as i64;
    metrics.track_count = nets.iter().map(|item| item.track_count).sum();
    metrics.via_count = nets.iter().map(|item| item.via_count).sum();
    metrics.total_length_nm = nets.iter().map(|item| item.length_nm).sum();
    let fingerprint: String =
        sha2::Sha256::digest(copper_repr(&original.detailed.routing_fingerprint, board).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
    let mut detailed = original.detailed.clone();
    detailed.board = board.clone();
    detailed.nets = nets;
    detailed.metrics = metrics;
    detailed.routing_fingerprint = fingerprint;
    let (detailed, drc, duplicate_stitch) = close_detailed_lands(&detailed, policy)?;
    let passed = original.placement_and_global.full_route_certified
        && original.critical.status != CriticalRoutingStatus::Failed
        && detailed.status == DetailedRoutingStatus::Success
        && drc.decision() == copper_drc::Decision::Pass;
    let mut merged = original.clone();
    merged.detailed = detailed;
    merged.drc = drc;
    merged.status = if passed { PhysicalFlowStatus::Pass } else { PhysicalFlowStatus::Fail };
    merged.duplicate_pad_stitch = Some(duplicate_stitch);
    Ok(merged)
}
