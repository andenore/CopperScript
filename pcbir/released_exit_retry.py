"""One append-only ordinary exit retry after disposable plane contacts leave."""
from collections import Counter
from dataclasses import dataclass, fields, replace
from types import MappingProxyType
from .physical import PadReference, RouteKind


@dataclass(frozen=True, slots=True)
class ReleasedExitRetryReport:
    status: str = "not_needed"
    passes: int = 0
    maximum_passes: int = 1
    selected_pads: tuple[PadReference, ...] = ()
    completed_pads: tuple[PadReference, ...] = ()
    remaining_pads: tuple[PadReference, ...] = ()
    added_tracks: int = 0
    added_vias: int = 0
    reason: str = ""


def complete_released_contact_exits(owner, incumbent, settings):
    """Caller must establish an actual contact release; never iterate this pass.

    Only originally pending general, non-bypass nets may append copper. Held
    handoffs, source/macro/array occurrences and semantic ownership are immutable.
    Every gate failure returns the complete incumbent plus a rejection report.
    """
    from .fanout import route_fanout
    from .bypass_reservations import verified_bypass_reservations
    from .decoupling_paths import refresh_crosslayer_witnesses, _budget_reason
    from .fanout_bypass_completion import _ports_valid, _hard
    from .routing_clearance import RoutingClearanceIndex
    from .package_access import _verified_bypass_links
    board=incumbent.board
    names={pad:n.name for n in board.nets for pad in n.pads}
    rules={r.net:r for r in board.net_routing_rules}
    semantic={l.net for l in board.decoupling_links}
    selected=tuple(pad for pad in incumbent.pending_pads if pad in names
        and names[pad] not in semantic
        and (names[pad] not in rules or rules[names[pad]].kind is RouteKind.GENERAL))
    report=ReleasedExitRetryReport(selected_pads=selected,remaining_pads=selected)
    def reject(reason):
        return replace(incumbent,released_exit_retry=replace(report,status="rejected",reason=reason))
    if not selected:
        return replace(incumbent,released_exit_retry=report)
    if (incumbent.added_track_count!=len(incumbent.created_tracks)
        or incumbent.added_via_count!=len(incumbent.created_vias)
        or len(board.tracks)!=len(owner.tracks)+len(incumbent.created_tracks)
        or len(board.vias)!=len(owner.vias)+len(incumbent.created_vias)
        or any(a is not b for a,b in zip(board.tracks,(*owner.tracks,*incumbent.created_tracks)))
        or any(a is not b for a,b in zip(board.vias,(*owner.vias,*incumbent.created_vias)))
        or any(getattr(board,f.name)!=getattr(owner,f.name) for f in fields(board)
               if f.name not in {"tracks","vias"})):
        return reject("invalid_incumbent_ownership")
    if verified_bypass_reservations(owner,incumbent)[2]:
        return reject("invalid_incumbent_bypass_ownership")
    index=RoutingClearanceIndex(board)
    if not _ports_valid(board,incumbent.accesses,incumbent.surface_accesses,incumbent.boundary_accesses,index):
        return reject("invalid_incumbent_exit")
    verified=_verified_bypass_links(board,semantic,admitted_witnesses=incumbent.decoupling_witnesses)
    baseline=_hard(board)
    nets=frozenset(names[pad] for pad in selected)
    report=replace(report,passes=1)
    late=route_fanout(board,settings,only_nets=nets)
    candidate=late.board
    if (len(candidate.tracks)<len(board.tracks) or len(candidate.vias)<len(board.vias)
        or any(a is not b for a,b in zip(board.tracks,candidate.tracks))
        or any(a is not b for a,b in zip(board.vias,candidate.vias))
        or any(getattr(candidate,f.name)!=getattr(board,f.name) for f in fields(board)
               if f.name not in {"tracks","vias"})):
        return reject("changed_incumbent_source_or_copper")
    tracks=candidate.tracks[len(board.tracks):];vias=candidate.vias[len(board.vias):]
    if (len(tracks)!=len(late.created_tracks) or len(vias)!=len(late.created_vias)
        or any(a is not b for a,b in zip(tracks,late.created_tracks))
        or any(a is not b for a,b in zip(vias,late.created_vias))
        or late.decoupling_tracks or late.decoupling_vias or late.decoupling_witnesses
        or any(t.net not in nets for t in (*tracks,*vias))
        or Counter(tracks)&Counter(board.tracks) or Counter(vias)&Counter(board.vias)
        or any(n!=1 for n in Counter(tracks).values()) or any(n!=1 for n in Counter(vias).values())):
        return reject("ambiguous_or_unrelated_generated_ownership")
    held=set(incumbent.routing_accesses)
    new={pad:anchor for pad,anchor in late.accesses.items() if pad in selected and pad not in held}
    surfaces={pad:anchor for pad,anchor in (late.surface_accesses or {}).items() if pad in selected and pad not in held}
    completed=tuple(pad for pad in selected if pad in new or pad in surfaces)
    if not completed:
        return replace(incumbent,released_exit_retry=replace(report,status="unchanged"))
    fresh=RoutingClearanceIndex(candidate)
    witnesses,reason=refresh_crosslayer_witnesses(candidate,incumbent.decoupling_witnesses,index=fresh)
    if reason or not verified<=_verified_bypass_links(candidate,semantic,admitted_witnesses=witnesses):
        return reject("lost_verified_bypass")
    if (not _ports_valid(candidate,incumbent.accesses,incumbent.surface_accesses,incumbent.boundary_accesses,fresh)
        or not _ports_valid(candidate,new,surfaces,None,fresh)):
        return reject("invalid_combined_exits")
    if any(_budget_reason(candidate,net) for net in nets) or _hard(candidate)-baseline:
        return reject("physical_or_source_budget_gate")
    result=replace(incumbent,board=candidate,
        created_tracks=(*incumbent.created_tracks,*tracks),created_vias=(*incumbent.created_vias,*vias),
        added_track_count=incumbent.added_track_count+len(tracks),added_via_count=incumbent.added_via_count+len(vias),
        accesses=MappingProxyType({**incumbent.accesses,**new}),
        surface_accesses=MappingProxyType({**(incumbent.surface_accesses or {}),**surfaces}),
        decoupling_witnesses=witnesses,
        pending_pads=tuple(pad for pad in incumbent.pending_pads if pad not in completed),
        pin_analysis=tuple(replace(a,diagnostic="resolved after disposable plane-contact release")
                           if a.pad in completed else a for a in incumbent.pin_analysis),
        released_exit_retry=replace(report,status="completed",completed_pads=completed,
            remaining_pads=tuple(pad for pad in selected if pad not in completed),
            added_tracks=len(tracks),added_vias=len(vias)))
    if verified_bypass_reservations(owner,result)[2]:
        return reject("final_bypass_ownership_gate")
    return result
