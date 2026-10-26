"""Physical witnesses for declared decoupling (bypass) associations."""

from __future__ import annotations

from .physical import RouteKind


def verified_bypass_links(board, only_nets, *, witness_paths=None, witness_vias=None, admitted_witnesses=()):
    """Fresh physical witnesses for explicit source bypass associations.

    Same-surface paths verify the feed only. Opposite-surface paths
    require both supply and declared component-return joins through normal vias.

    Optional witness_paths receives original input track identities, never
    reconstructed or synthesized ownership. No routing, via shortcut or connected
    flag is used as evidence. Actual
    preferred/breakout widths, every foreign land and all source copper apply.
    """
    from .routing_clearance import RoutingClearanceIndex
    from .pin_escape import _verified_land_path
    from .placement import transformed_pad_position
    from .physical import BoardSide, CopperLayer, PadKind
    from .routing_layers import routing_layers
    from .hard_macros import macro_owned_pads
    if witness_paths is not None:
        witness_paths.clear()
    if witness_vias is not None:
        witness_vias.clear()
    poses = {p.reference:p for p in board.placements}
    rules = {r.net:r for r in board.net_routing_rules}
    index, owned, verified = RoutingClearanceIndex(board), macro_owned_pads(board), set()
    for link in board.decoupling_links:
        if link.net not in only_nets or link.target in owned or link.capacitor in owned:
            continue
        rule=rules.get(link.net)
        if rule is not None and rule.kind is not RouteKind.GENERAL:
            continue
        target,cap=poses[link.target.component],poses[link.capacitor.component]
        lands=[p for ref,pose in ((link.target,target),(link.capacitor,cap))
               for p in board.footprints[pose.footprint].pads if p.number==ref.pad]
        if len(lands)!=2 or any(p.kind is not PadKind.SMD for p in lands):
            continue
        if cap.side is not target.side:
            from .decoupling_paths import verify_crosslayer_decoupling
            measured = verify_crosslayer_decoupling(board, link, index=index)
            if measured.witness is not None:
                verified.add(link)
                paths = (measured.witness.supply_path, measured.witness.return_path)
                if witness_paths is not None:
                    witness_paths[link] = tuple(dict.fromkeys(t for path in paths for t in path.tracks))
                if witness_vias is not None:
                    witness_vias[link] = tuple(dict.fromkeys(v for path in paths for v in path.vias))
            continue
        layer=CopperLayer.FRONT if target.side is BoardSide.FRONT else CopperLayer.BACK
        if layer not in routing_layers(board,link.net,rule):
            continue
        end=transformed_pad_position(board,cap,link.capacitor.pad)
        width=rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        path = _verified_land_path(board,lands[0],target,link.net,end,index,require_via=False,
                                   target_layer=layer,required_width_nm=width)
        if path is not None:
            verified.add(link)
            if witness_paths is not None:
                witness_paths[link] = path
    if admitted_witnesses:
        from .decoupling_paths import refresh_crosslayer_witnesses
        admitted, reason = refresh_crosslayer_witnesses(board,admitted_witnesses,index=index)
        if reason:
            if witness_paths is not None:
                witness_paths.clear()
            if witness_vias is not None:
                witness_vias.clear()
            return frozenset()
        for witness in admitted:
            if witness.link not in verified:
                continue
            if witness_paths is not None:
                witness_paths[witness.link] = tuple(dict.fromkeys((*witness_paths.get(witness.link,()),
                    *(t for path in (witness.supply_path,witness.return_path) for t in path.tracks))))
            if witness_vias is not None:
                witness_vias[witness.link] = tuple(dict.fromkeys((*witness_vias.get(witness.link,()),
                    *(v for path in (witness.supply_path,witness.return_path) for v in path.vias))))
    return frozenset(verified)
