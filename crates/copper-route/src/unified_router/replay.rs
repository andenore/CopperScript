//! `replay_unified`: route the inputs saved by `save_inputs_path` again,
//! with option overrides and a narrowed net selection.

use indexmap::IndexSet;

use copper_json::Value;
use copper_physical::CopperLayer;

use crate::detailed::DetailedRoutingResult;
use crate::progress::ProgressCallback;
use crate::state_codec::{SavedInputs, load_inputs};
use crate::unified_error::{Result, fail, not_implemented};

/// The keyword arguments of `replay_unified` besides the state path.
#[derive(Clone, Debug, Default)]
pub struct ReplayRequest {
    pub only_nets: Option<IndexSet<String>>,
    pub only_components: Option<IndexSet<String>>,
    /// `bga_fanout`: `plan_bga_fanouts` keyword arguments (`{}` for its defaults).
    pub bga_fanout: Option<Vec<(String, Value)>>,
    pub power_islands: Option<String>,
    pub exact_escape: Option<String>,
    pub overrides: Vec<(String, Value)>,
}

fn sorted_names(names: impl IntoIterator<Item = String>) -> String {
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names.join(", ")
}

/// The saved inputs after the option, net and component narrowing
/// `replay_unified` applies before any stage runs.
pub fn prepare_replay(text: &str, path: &str, request: &ReplayRequest) -> Result<SavedInputs> {
    let mut saved = load_inputs(text, path)?;
    saved.options = saved.options.with_overrides(&request.overrides)?;
    let mut only_nets = request.only_nets.clone();
    if let Some(components) = &request.only_components {
        let touching: IndexSet<String> = saved
            .board
            .nets
            .iter()
            .filter(|net| net.pads.iter().any(|pad| components.contains(&pad.component)))
            .map(|net| net.name.clone())
            .collect();
        only_nets = Some(match only_nets {
            None => touching,
            Some(nets) => nets.union(&touching).cloned().collect(),
        });
    }
    if let Some(only) = only_nets {
        let known: IndexSet<&str> = saved.board.nets.iter().map(|net| net.name.as_str()).collect();
        let unknown: Vec<String> = only.iter().filter(|net| !known.contains(net.as_str())).cloned().collect();
        if !unknown.is_empty() {
            return fail(format!("unknown replay nets: {}", sorted_names(unknown)));
        }
        if let Some(saved_nets) = &saved.only_nets {
            let excluded: Vec<String> = only.iter().filter(|net| !saved_nets.contains(*net)).cloned().collect();
            if !excluded.is_empty() {
                return fail(format!("nets outside saved replay selection: {}", sorted_names(excluded)));
            }
        }
        saved.only_nets = Some(match &saved.only_nets {
            None => only,
            Some(saved_nets) => saved_nets.iter().filter(|net| only.contains(*net)).cloned().collect(),
        });
    }
    Ok(saved)
}

/// `replay_unified(path, on_progress, ...)` on the state file's text.
pub fn replay_unified(
    text: &str,
    path: &str,
    request: &ReplayRequest,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<DetailedRoutingResult> {
    let mut saved = prepare_replay(text, path, request)?;
    if let Some(islands) = &request.power_islands
        && islands != "auto"
        && CopperLayer::parse(islands).is_none()
    {
        return fail(format!("{} is not a valid CopperLayer", copper_json::python_str_repr(islands)));
    }
    if let Some(islands) = &request.power_islands {
        let layer = if islands == "auto" {
            crate::power_islands::auto_power_layer(&saved.board)
        } else {
            CopperLayer::parse(islands)
        };
        if let Some(layer) = layer {
            let partition = if saved.options.power_islands_ball_grid_only {
                crate::power_islands::plan_ball_grid_islands(&saved.board, layer)?
            } else {
                crate::power_islands::plan_power_islands(&saved.board, layer)?
            };
            saved.board.zones.extend(partition.zones.iter().cloned());
            if let Some(callback) = on_progress.as_mut() {
                callback.observe("power_islands", "finished", partition.report());
            }
        }
    }
    if request.bga_fanout.is_some() {
        return not_implemented("replay-unified --refanout (BGA fanout)");
    }
    if request.exact_escape.is_some() {
        return not_implemented("replay-unified --exact-escape (exact escape)");
    }
    let map = |items: &Option<Vec<(copper_pcb::board::PadReference, crate::pin_escape::RoutingAccess)>>| {
        items.as_ref().map(|items| items.iter().cloned().collect::<indexmap::IndexMap<_, _>>())
    };
    if saved.options.save_inputs_path.as_deref().is_some_and(|path| !path.is_empty())
        && (saved.fanout_created_vias.is_some() || saved.fanout_created_tracks.is_some())
    {
        return not_implemented("unified router save_inputs_path with fanout-created copper");
    }
    let fanout = map(&saved.fanout_accesses);
    let required = map(&saved.required_accesses);
    super::route::route_unified(
        &saved.board,
        &saved.global_route,
        &saved.options,
        fanout.as_ref(),
        saved.only_nets.as_ref(),
        required.as_ref(),
        None,
        on_progress,
    )
}
