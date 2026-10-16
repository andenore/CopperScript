"""Exact proposal-only transfer of witnessed plane contacts to bypass ownership."""
from collections import Counter
from dataclasses import dataclass,replace
from .physical import PhysicalBoard,TrackSegment
from .plane import PlaneStitchResult

@dataclass(frozen=True,slots=True)
class PlaneContactProposal:
    source: PhysicalBoard
    result: PlaneStitchResult


def transfer_witnessed_plane_contacts(owner, frame, proposed, contacts):
    """Retain only declared explicit paths, never a zone/net-label connection."""
    from .package_access import _ordinary_probe_frame,_verified_bypass_links
    if not isinstance(contacts,PlaneContactProposal):
        return None
    seed,result=contacts.source,contacts.result
    if not isinstance(seed,PhysicalBoard) or not isinstance(result,PlaneStitchResult):
        return None
    nt,nv=result.added_track_count,result.added_via_count
    if (type(nt) is not int or type(nv) is not int or min(nt,nv)<0
            or len(result.board.tracks)!=len(seed.tracks)+nt
            or len(result.board.vias)!=len(seed.vias)+nv
            or _ordinary_probe_frame(owner,seed) is None
            or _ordinary_probe_frame(seed,result.board) is None
            or _ordinary_probe_frame(owner,frame) is None
            or len(frame.tracks)!=len(result.board.tracks)
            or len(frame.vias)!=len(result.board.vias)):
        return None
    if (any(a is not b for a,b in zip(seed.tracks,result.board.tracks))
            or any(a is not b for a,b in zip(seed.vias,result.board.vias))
            or any(a is not b for a,b in zip(frame.tracks,result.board.tracks))
            or any(a is not b for a,b in zip(frame.vias,result.board.vias))):
        return None
    available_tracks=result.board.tracks[len(seed.tracks):]
    available_vias=result.board.vias[len(seed.vias):]
    # A copied occurrence must not relabel critical, immutable or array copper.
    if (Counter(available_tracks)&Counter(seed.tracks)
            or Counter(available_vias)&Counter(seed.vias)
            or len(set(available_tracks))!=len(available_tracks)
            or len(set(available_vias))!=len(available_vias)):
        return None
    paths,via_paths={},{}
    links=_verified_bypass_links(proposed.board,{l.net for l in owner.decoupling_links},
        witness_paths=paths,witness_vias=via_paths,admitted_witnesses=proposed.decoupling_witnesses)
    tids={id(t) for l in links for t in paths.get(l,())}
    vids={id(v) for l in links for v in via_paths.get(l,())}
    for witness in proposed.decoupling_witnesses:
        if witness.link not in links:
            continue
        for path in (witness.supply_path,witness.return_path):
            for step in path.steps:
                (tids if isinstance(step.edge,TrackSegment) else vids).add(id(step.edge))
    tracks=tuple(t for t in available_tracks if id(t) in tids)
    vias=tuple(v for v in available_vias if id(v) in vids)
    return replace(proposed,created_tracks=(*tracks,*proposed.created_tracks),
        created_vias=(*vias,*proposed.created_vias),
        added_track_count=len(tracks)+proposed.added_track_count,
        added_via_count=len(vias)+proposed.added_via_count,
        decoupling_tracks=(*tracks,*proposed.decoupling_tracks),
        decoupling_vias=(*vias,*proposed.decoupling_vias))
