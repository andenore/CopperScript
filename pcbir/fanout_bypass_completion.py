"""One bounded late completion of original cross-surface bypass obligations.

Ordinary source launches remain ordinary owners. This transaction may append
semantic tails only; its rejection retains the complete incumbent fanout.
"""
from __future__ import annotations
from collections import Counter
from dataclasses import dataclass, fields, replace
from types import MappingProxyType

from .physical import DecouplingLink


@dataclass(frozen=True, slots=True)
class LateBypassCompletionReport:
    status: str = "not_needed"
    passes: int = 0
    maximum_passes: int = 1
    selected_links: tuple[DecouplingLink, ...] = ()
    completed_links: tuple[DecouplingLink, ...] = ()
    remaining_links: tuple[DecouplingLink, ...] = ()
    added_tracks: int = 0
    added_vias: int = 0
    reason: str = ""
    trigger: str = "ordinary_launches"


def _hard(board):
    from .drc import DrcSeverity, run_physical_drc
    return {f.fingerprint for f in run_physical_drc(board).findings
            if f.severity is DrcSeverity.ERROR and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}}


def _ports_valid(board, accesses, surfaces, boundaries, index):
    from .pin_escape import verified_fanout_path, verified_routing_access
    names={pad:net.name for net in board.nets for pad in net.pads}
    return (all(pad in names and verified_fanout_path(board,pad,names[pad],anchor,index) is not None
                for pad,anchor in accesses.items())
            and all(pad in names and verified_routing_access(board,pad,names[pad],anchor,index) is not None
                    for ports in (surfaces or {},boundaries or {}) for pad,anchor in ports.items()))


def complete_late_bypasses(incumbent, original_pending, options, *, only_nets=None,
                           trigger="ordinary_launches"):
    """One pass over exact early pending opposite-face source links, no rounds.

    Per-link launch/site/proposal limits stay with route_decouplers and its
    public proposer. No source, area, critical, placement or ordinary reroute.
    """
    if trigger not in {"ordinary_launches", "released_plane_contacts"}:
        raise ValueError("unknown late bypass trigger")
    from .decoupling import route_decouplers
    from .decoupling_paths import (_budget_reason, _normal_via, _track_ok,
                                   refresh_crosslayer_witnesses)
    from .package_access import _verified_bypass_links
    from .routing_clearance import RoutingClearanceIndex
    board=incumbent.board
    poses={p.reference:p for p in board.placements}
    if any(link not in board.decoupling_links for link,_ in original_pending):
        raise ValueError("late bypass selection contains an undeclared original obligation")
    selected=tuple(dict.fromkeys(link for link,_ in original_pending
        if poses[link.capacitor.component].side is not poses[link.target.component].side
        and (only_nets is None or link.net in only_nets)))
    report=LateBypassCompletionReport(selected_links=selected,remaining_links=selected,trigger=trigger)
    def reject(reason):
        return replace(incumbent,late_bypass=replace(report,status="rejected",reason=reason))
    if not selected:
        return replace(incumbent,late_bypass=report,decoupling_pending_links=tuple(original_pending))
    if (incumbent.added_track_count!=len(incumbent.created_tracks)
            or incumbent.added_via_count!=len(incumbent.created_vias)
            or len(board.tracks)<len(incumbent.created_tracks) or len(board.vias)<len(incumbent.created_vias)
            or (incumbent.created_tracks and any(a is not b for a,b in zip(board.tracks[-len(incumbent.created_tracks):],incumbent.created_tracks)))
            or (incumbent.created_vias and any(a is not b for a,b in zip(board.vias[-len(incumbent.created_vias):],incumbent.created_vias)))):
        return reject("invalid_incumbent_generated_suffix")
    if not (Counter(incumbent.created_tracks)-Counter(incumbent.decoupling_tracks)
            or Counter(incumbent.created_vias)-Counter(incumbent.decoupling_vias)):
        return replace(incumbent,late_bypass=replace(report,reason="no_new_ordinary_copper"),
                       decoupling_pending_links=tuple(original_pending))
    index=RoutingClearanceIndex(board)
    old,reason=refresh_crosslayer_witnesses(board,incumbent.decoupling_witnesses,index=index)
    if reason:
        return reject("invalid_incumbent_witness:"+reason)
    if not _ports_valid(board,incumbent.accesses,incumbent.surface_accesses,incumbent.boundary_accesses,index):
        return reject("invalid_incumbent_port")
    nets=frozenset(link.net for link in board.decoupling_links)
    verified=_verified_bypass_links(board,nets,admitted_witnesses=old)
    baseline=_hard(board)
    report=replace(report,passes=1)
    late=route_decouplers(board,only_links=frozenset(selected),only_nets=only_nets,fanout_options=options)
    candidate=late.board
    # Exact occurrence identity/order, not a value-equal reconstructed prefix.
    if (any(getattr(candidate,f.name)!=getattr(board,f.name) for f in fields(board) if f.name not in {"tracks","vias"})
            or len(candidate.tracks)<len(board.tracks) or len(candidate.vias)<len(board.vias)
            or any(a is not b for a,b in zip(board.tracks,candidate.tracks))
            or any(a is not b for a,b in zip(board.vias,candidate.vias))):
        return reject("changed_incumbent_source_or_copper_prefix")
    tracks=candidate.tracks[len(board.tracks):];vias=candidate.vias[len(board.vias):]
    if (len(tracks)!=len(late.created_tracks) or len(vias)!=len(late.created_vias)
            or any(a is not b for a,b in zip(tracks,late.created_tracks))
            or any(a is not b for a,b in zip(vias,late.created_vias))
            or Counter(tracks)&Counter(board.tracks) or Counter(vias)&Counter(board.vias)
            or any(n!=1 for n in Counter(tracks).values()) or any(n!=1 for n in Counter(vias).values())):
        return reject("ambiguous_late_generated_ownership")
    fresh=RoutingClearanceIndex(candidate)
    admitted,reason=refresh_crosslayer_witnesses(candidate,(*old,*late.admitted_path_witnesses),index=fresh)
    if reason:
        return reject("lost_admitted_witness:"+reason)
    current=_verified_bypass_links(candidate,nets,admitted_witnesses=admitted)
    if not verified<=current:
        return reject("lost_verified_association")
    if any(_budget_reason(candidate,net.name) for net in candidate.nets):
        return reject("source_net_budget_exceeded")
    if (not all(_track_ok(candidate,fresh,t,t.net) for t in tracks)
            or not all(_normal_via(candidate,fresh,v) for v in vias)
            or _hard(candidate)-baseline):
        return reject("late_physical_gate_failed")
    if not _ports_valid(candidate,incumbent.accesses,incumbent.surface_accesses,incumbent.boundary_accesses,fresh):
        return reject("lost_incumbent_port")
    completed=tuple(link for link in selected if link in current)
    permitted={pad for link in completed for pad in (link.target,link.capacitor,link.return_pad)}
    permitted.update(w.return_target for w in admitted if w.link in completed)
    if any(pad not in permitted for pad in (late.accesses or {})):
        return reject("unrelated_late_port")
    # Existing via, surface and boundary handoffs win unchanged; a new semantic
    # witness may use another held launch without replacing the old handoff.
    held=set(incumbent.routing_accesses)
    new={p:a for p,a in (late.accesses or {}).items() if p not in held}
    if not _ports_valid(candidate,new,None,None,fresh):
        return reject("invalid_new_late_port")
    pending_reasons={link:reason for link,reason in late.pending}
    pending=tuple((link,pending_reasons.get(link,reason)) for link,reason in original_pending if link not in completed)
    result=replace(incumbent,board=candidate,created_tracks=(*incumbent.created_tracks,*tracks),
        created_vias=(*incumbent.created_vias,*vias),
        added_track_count=incumbent.added_track_count+len(tracks),added_via_count=incumbent.added_via_count+len(vias),
        decoupling_tracks=(*incumbent.decoupling_tracks,*tracks),decoupling_vias=(*incumbent.decoupling_vias,*vias),
        decoupling_witnesses=admitted,accesses=MappingProxyType({**incumbent.accesses,**new}),
        pending_pads=tuple(p for p in incumbent.pending_pads if p not in new),
        pin_analysis=tuple(replace(a,diagnostic="resolved by verified late bypass port")
                           if a.pad in new and a.pad in incumbent.pending_pads else a for a in incumbent.pin_analysis),
        decoupling_pending_links=pending,decoupling_pending=tuple((link.capacitor,reason) for link,reason in pending),
        late_bypass=replace(report,status="completed" if completed else "unchanged",completed_links=completed,
            remaining_links=tuple(link for link in selected if link not in completed),added_tracks=len(tracks),added_vias=len(vias)))
    # Final declared semantic ledger must be covered by actual complete paths;
    # held ordinary source edges were never promoted to semantic ownership.
    from .bypass_reservations import verified_bypass_reservations
    source=replace(board,tracks=board.tracks[:-len(incumbent.created_tracks)] if incumbent.created_tracks else board.tracks,
                   vias=board.vias[:-len(incumbent.created_vias)] if incumbent.created_vias else board.vias)
    if verified_bypass_reservations(source,result)[2]:
        return reject("final_semantic_ownership_admission_failed")
    return result
