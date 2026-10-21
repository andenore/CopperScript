"""Bounded additive completion of native-rejected power transitions.

An existing plated escape is never removed or converted into a surface track.
Exact native bindings authorize a trial only; fresh native acceptance remains
the caller's responsibility after complete same-source recertification.
"""
from dataclasses import replace
from hashlib import sha256
from math import ceil

from .copper_connectivity import copper_contacts_overlap
from .drc import _explicit_copper_contacts, physical_board_digest
from .plane_verify import native_object_map
from .power_areas import allocate_power_areas


def repair_native_power_transitions(plan, options, evidence):
    from .power_geometry import (apply_power_area_plan, _AreaClearance, _attachment_branch,
        _attachment_pairs, _candidate_shapes, _certify_pool)
    if (plan.source_board is None or physical_board_digest(plan.source_board)!=plan.snapshot):
        raise ValueError('stale power transition source')
    board,status=apply_power_area_plan(plan.source_board,plan)
    if status not in {'contact_routed_native_pending','already_contact_routed'}:
        return plan
    if not evidence.matches(board):
        raise ValueError('stale power transition evidence')
    if (len(plan.conflict_trials)>=options.maximum_conflict_trials
            or plan.allocation_states>=options.allocation_states or not options.maximum_branch_layers):
        return plan
    bindings=native_object_map(board)
    nets={c.allocation.domain for c in plan.selected}
    eligible=set()
    for finding in evidence.native_findings:
        if finding.category!='violation' or finding.type!='via_dangling':continue
        for item in finding.items:
            if (item.kind=='via' and item.net in nets and item.index is not None
                    and bindings.get(item.uuid)==item):
                eligible.add(item.index)
    if not eligible:return plan
    net=min(board.vias[i].net for i in eligible)
    indices=tuple(sorted(i for i in eligible if board.vias[i].net==net))
    old=next(c for c in plan.selected if c.allocation.domain==net)
    label='native-transition:'+evidence.report_digest[:20]
    observation=dict(net=net,source_snapshot=plan.snapshot,board_snapshot=evidence.board_digest,
        native_report=evidence.report_digest,via_indices=indices,attempts=[],expanded_states=0)
    used=0;failures=[]
    def finish(trial_status, **changes):
        observation['status']=trial_status
        return replace(plan,**changes,
            conflict_trials=(*plan.conflict_trials,(net,label,trial_status)),
            conflict_contexts=(*plan.conflict_contexts,(net,tuple(c.allocation.id for c in plan.selected
                if c.allocation.id != old.allocation.id))),
            native_transition_trials=(*plan.native_transition_trials,observation),
            search_failures=(*plan.search_failures,*failures))
    # A new conductor can also affect another domain's modeled return. This
    # geometry-only repair cannot reuse any pre-change extracted network; keep
    # the original proposal until complete electrical reassessment is supported.
    if options.electrical_models or any(c.allocation.network is not None for c in plan.selected):
        return finish('transition_electrical_reassessment_required')
    if len(indices)>options.maximum_new_contacts:return finish('transition_contact_budget_exhausted')
    rule=next((r for r in board.net_routing_rules if r.net==net),None)
    width=max(board.rules.minimum_track_width_nm,(rule.width_nm or 0) if rule else 0)
    additions=[];temporary=board
    for index in indices:
        via=board.vias[index]
        objects,_,_=_explicit_copper_contacts(temporary,only_nets=frozenset({net}),include_internal_connections=False)
        contact=next(o for o in objects if o.identity==f'via:{index}')
        attached={l for o in objects if not o.identity.startswith('via:')
            and copper_contacts_overlap(contact,o) for l in o.layers}
        if len(attached)!=1:return finish('transition_layer_use_unsupported')
        layers=[l for l in (options.allowed_layers or board.stackup.copper_layers)
            if l in contact.layers and l not in attached
            and (not rule or not rule.allowed_layers or l in rule.allowed_layers)]
        choices=[]
        for layer in layers:
            points={p for o in objects if o.identity!=contact.identity and layer in o.layers
                and o.identity.startswith(('pad:','via:','track:')) for p in o.shape.spine if p!=via.position}
            pairs=sorted((abs(p.x_nm-via.position.x_nm)+abs(p.y_nm-via.position.y_nm),
                via.position.x_nm,via.position.y_nm,p.x_nm,p.y_nm,via.position,p) for p in points)
            if pairs:choices.append((pairs[0][0],layer in (board.stackup.copper_layers[0],board.stackup.copper_layers[-1]),layer,pairs))
        choices.sort(key=lambda row:(row[0],row[1],row[2].value))
        branch=None
        for _,_,layer,pairs in choices[:options.maximum_branch_layers]:
            if used>=options.branch_search_states:return finish('transition_search_budget_exhausted')
            work=[]
            branch=_attachment_branch(temporary,net,layer,_attachment_pairs(pairs,options.maximum_anchors),
                width,_AreaClearance(temporary,options),options,options.branch_search_states-used,0,failures,work=work)
            used+=sum(work);observation['expanded_states']=used
            observation['attempts'].append(dict(via_index=index,layer=layer.value,
                expanded_states=sum(work),connected=bool(branch)))
            if branch:break
        if not branch:return finish('transition_no_path')
        additions.extend(t for t in branch if t not in temporary.tracks)
        temporary=replace(board,tracks=(*board.tracks,*additions))
    if not additions:return finish('transition_no_addition')
    identity=sha256(repr((old.allocation.id,tuple(additions))).encode()).hexdigest()[:20]
    alternative=replace(old,tracks=(*old.tracks,*additions),allocation=replace(old.allocation,id=identity,
        resources=frozenset({f'candidate:{identity}'})))
    # These are explicit tracks; no zone outline is treated as connectivity.
    extra_shapes=_candidate_shapes(plan.source_board,replace(alternative,tracks=tuple(additions),vias=(),zones=()))
    alternative=replace(alternative,allocation=replace(alternative.allocation,
        area_nm2=old.allocation.area_nm2+ceil(sum(shape.area for shape in extra_shapes.values()))))
    pool=(*plan.candidates,alternative)
    bound=options.maximum_domains*options.candidates_per_domain+options.maximum_conflict_trials
    if len(pool)>bound:return finish('transition_candidate_budget_exhausted')
    shapes={c.allocation.id:_candidate_shapes(plan.source_board,c) for c in pool}
    pool=_certify_pool(plan.source_board,pool,shapes,_AreaClearance(plan.source_board,options))
    selected=({c.allocation.id for c in plan.selected}-{old.allocation.id})|{identity}
    assignment=allocate_power_areas(tuple(c.allocation for c in pool if c.allocation.id in selected),
        domains=tuple(o.net for o in plan.obligations),snapshot=plan.snapshot,maximum_candidates=bound,
        maximum_states=options.allocation_states-plan.allocation_states,allow_unassessed_geometry=True)
    states=plan.allocation_states+assignment.states
    if not assignment.selected:
        return finish('transition_assignment_rejected',allocation_states=states,exhausted=assignment.exhausted)
    proposed=replace(plan,candidates=pool,selected_ids=tuple(c.id for c in assignment.selected),allocation_states=states)
    candidate,status=apply_power_area_plan(plan.source_board,proposed)
    if status!='contact_routed_native_pending':return finish(status,allocation_states=states)
    if candidate.tracks[:len(board.tracks)]!=board.tracks:
        # Candidate order may change, so verify exact held multiplicity below.
        from collections import Counter
        if Counter(board.tracks)-Counter(candidate.tracks):return finish('transition_held_copper_lost',allocation_states=states)
    if candidate.vias!=board.vias or candidate.zones!=board.zones:
        from collections import Counter
        if Counter(candidate.vias)!=Counter(board.vias) or Counter(candidate.zones)!=Counter(board.zones):
            return finish('transition_held_copper_changed',allocation_states=states)
    observation['added_tracks']=[dict(start_nm=(t.start.x_nm,t.start.y_nm),end_nm=(t.end.x_nm,t.end.y_nm),
        layer=t.layer.value,width_nm=t.width_nm) for t in additions]
    observation['selected_ids']=proposed.selected_ids
    return finish('geometry_proposal',candidates=pool,selected_ids=proposed.selected_ids,
        allocation_states=states,status='geometry_proposal')
