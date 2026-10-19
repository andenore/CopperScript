"""Prospective package contacts and distinct temporary ordinary reservations."""
from dataclasses import replace

from .package_patterns import circular_array,propose_package_patterns,PackagePatternOptions
from .package_support import package_contact_intents
from .copper_ownership import append_copper_ownership,CopperUse
from .routing_clearance import _geometry_identity
from .copper_reservations import reserve_copper_scene


def prepare_package_patterns(board,ownership,selected,links,options,*,on_progress=None):
    """Bounded source-derived patterns before local power/return producers.

    Supply/return contacts become exactly owned, protected prospective copper.
    Other nets remain temporary obstacles and need the normal ordinary handoff.
    Neither contact geometry nor this stage claims native/plane connectivity.
    """
    source=board;original_ownership=ownership;original_options=options;reports=[]
    produced=set(selected)|{l.return_net for l in links}
    components=tuple(sorted({l.target.component for l in links if circular_array(board,l.target.component)}))
    if len(components)>4:return source,original_ownership,original_options,('package_count_limit',)
    for component in components:
        intents=package_contact_intents(board,component)
        if not intents:continue
        scene=reserve_copper_scene(board,options.reserved_tracks,options.reserved_vias)
        offset=dict(options.package_pattern_offsets).get(component,options.package_pattern_offset_nm)
        result=propose_package_patterns(scene.board,component,intents,
            PackagePatternOptions(preferred_offset_nm=offset),on_progress=on_progress)
        reports.append(result.report())
        if not result.candidates:return source,original_ownership,original_options,tuple(reports)
        pattern=result.candidates[0]
        assert not pattern.deferred_components
        physical=tuple(c for c in pattern.contacts if c.net in produced)
        reserved=tuple(c for c in pattern.contacts if c.net not in produced)
        candidate=reserve_copper_scene(board,tuple(dict.fromkeys(t for c in physical for t in c.tracks)),
            tuple(dict.fromkeys(c.via for c in physical if not c.reused_via))).board
        uses=tuple(CopperUse('package-contact:'+_geometry_identity(c.intent.pad),'contact',(c.intent.pad,),
            c.tracks,(c.via,)) for c in physical)
        ownership=append_copper_ownership(ownership,board,candidate,'package-pattern',uses=uses,trial_mutable=False)
        board=candidate
        extra_tracks=tuple(t for t in dict.fromkeys(t for c in reserved for t in c.tracks)
                           if t not in options.reserved_tracks)
        extra_vias=tuple(v for v in dict.fromkeys(c.via for c in reserved if not c.reused_via)
                         if v not in options.reserved_vias)
        options=replace(options,reserved_tracks=(*options.reserved_tracks,*extra_tracks),
                        reserved_vias=(*options.reserved_vias,*extra_vias))
    return board,ownership,options,tuple(reports)
