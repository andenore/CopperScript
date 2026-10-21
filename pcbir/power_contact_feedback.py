"""Replay failed area joins into exact, disposable contact dependency trials.

Search samples only choose endpoints. Release authority comes from querying
every segment of a complete proposed path against the current held scene.
The path is a temporary obstacle during regeneration, never adopted copper
or evidence that the complete distribution network is connected.
"""
from dataclasses import dataclass, replace

from .copper_ownership import append_copper_ownership, prepare_copper_trial, rebuild_copper_ownership
from .copper_reservations import reserve_copper_scene, reserved_copper_additions
from .drc import physical_board_digest
from .joint_access import JointAccessOptions, allocate_joint_access, _materialize
from .joint_preparation import (JointPreparationOptions, JointPreparationResult, _alternatives,
    _requirements, _new_contact_uses, joint_reservations_preserved)
from .power_geometry import _AreaClearance, _tree_branch
from .copper_branches import (branch_track_occurrences, prepare_track_branch_trial,
                             rebuild_track_branch_ownership, RetainedJunctionClearance, track_endpoint_guards)
from .progress import emit


@dataclass(frozen=True)
class PowerContactFeedbackOptions:
    maximum_roots: int = 24
    maximum_path_states: int = 16000
    joint: JointPreparationOptions = JointPreparationOptions()
    maximum_branch_obligations: int = 128

    def __post_init__(self):
        if any(type(v) is not int or v <= 0 for v in
               (self.maximum_roots, self.maximum_path_states, self.maximum_branch_obligations)):
            raise ValueError('positive power/contact feedback bounds required')


def _discovery_occurrences(ledger, options):
    """Keep oversized/unsupported owner groups held during path discovery.

    Requirement dependencies and shared mutable occurrences form components.
    This is only a search filter; every resulting path still needs a fresh
    complete blocker query and the normal exact ownership closure.
    """
    adjacency = {u.id:set(u.dependencies) for u in ledger.uses}
    for identity, neighbors in tuple(adjacency.items()):
        for neighbor in tuple(neighbors):
            adjacency[neighbor].add(identity)
    for occurrence in ledger.occurrences:
        if occurrence.trial_mutable:
            for identity in occurrence.requirements:
                adjacency[identity].update(occurrence.requirements)
    kinds = {u.id:u.kind for u in ledger.uses}
    pending, eligible = set(adjacency), set()
    while pending:
        component, frontier = set(), {min(pending)}
        while frontier:
            component.update(frontier)
            frontier = {n for i in frontier for n in adjacency[i]}-component
        pending.difference_update(component)
        occurrences = tuple(o for o in ledger.occurrences
                            if o.trial_mutable and component.intersection(o.requirements))
        if (len(component) <= options.maximum_obligations and len(occurrences) <= options.maximum_occurrences
                and all(o.negotiable for o in occurrences)
                and all(kinds[i] in {'contact','bypass','signal'} for i in component)):
            eligible.update(o.identity for o in occurrences)
    return eligible


def _prove_branch_requirements(board, frame, membership):
    """Broad fresh validation is independent of the small geometric cut."""
    from .drc import explicit_copper_connectivity
    from .plane import _via_contacts_zone
    from .placement import resolved_copper_keepouts
    from .package_access import _verified_bypass_links
    required = _requirements(frame, None, membership)
    if len(required) != len(frame.requirements):
        return (), 'unsupported_branch_obligation'
    bypasses = _verified_bypass_links(board, {r.net for r in required if r.kind == 'bypass'})
    graph = explicit_copper_connectivity(board, only_nets=frozenset(r.net for r in required),
                                         include_internal_connections=False)
    keepouts = resolved_copper_keepouts(board)
    uses = {u.id: u for u in frame.ownership.uses}
    for r in required:
        if r.kind == 'bypass':
            if (r.link not in bypasses or not {r.link.target,r.link.capacitor,r.link.return_pad}<=set(r.terminals)):
                return (), 'branch_bypass_lost'
        elif r.kind == 'contact':
            nodes = graph.pad_nodes.get(r.terminals[0], ())
            roots = {graph.roots[n] for n in nodes}
            valid = False
            for i, via in enumerate(board.vias):
                if via.net != r.net or via not in uses[r.id].vias:
                    continue
                low, high = sorted((board.stackup.copper_layers.index(via.from_layer),
                                    board.stackup.copper_layers.index(via.to_layer)))
                span = set(board.stackup.copper_layers[low:high+1])
                if (nodes and roots == {graph.roots[f'via:{i}']}
                        and any(z.net == r.net and _via_contacts_zone(via.position,z,span,keepouts)
                                for z in board.zones)):
                    valid = True
                    break
            if not valid:
                return (), 'branch_contact_lost'
        else:
            return (), 'unsupported_branch_obligation'
    return required, None


def _replace_track_branches(board, ownership, scene_board, augmented, seeds, path,
                            joint_options, area_options, state_budget, membership, obligation_bound):
    """Keep all vias/junctions and replace only measured generated tracks."""
    pg_frame = prepare_track_branch_trial(board, ownership, seeds,
        maximum_occurrences=joint_options.maximum_occurrences)
    if len(pg_frame.requirements) > obligation_bound:
        return None, None, (), 0, 'branch_validation_budget_exhausted', {}
    scene = reserve_copper_scene(scene_board, path)
    guarded = append_copper_ownership(augmented, scene_board, scene.board, 'held-power-join')
    frame = prepare_track_branch_trial(scene.board, guarded, seeds,
        maximum_occurrences=joint_options.maximum_occurrences)
    if frame.sections != pg_frame.sections:
        return None, None, (), 0, 'held_area_changed_branch_junctions', {}
    held = frame.held
    paths, spent = [], 0
    detail = dict(repair='retained_track_junctions', released=frame.released,
        requirements=frame.requirements, sections=len(frame.sections), boundaries=frame.boundaries,
        retained_vias=len(board.vias))
    if not all(_AreaClearance(held, area_options).can_track(
            t.net,t.start,t.end,t.width_nm,t.layer) for t in path):
        return None, None, (), spent, 'incomplete_branch_blocker_witness', detail
    objects = {o.identity:o for o in frame.ownership.occurrences}
    for i, section in enumerate(frame.sections):
        old = objects[section.occurrence].value
        work, failures = [], []
        clearance = RetainedJunctionClearance(_AreaClearance(held,area_options),old.net,
                                              (section.start,section.end))
        # Distribution grids are too coarse for a narrow package lead-in.
        # Refine computational pitch from its actual width, without changing
        # copper dimensions or resetting the shared path-state allowance.
        local_options = replace(area_options,step_nm=min(area_options.step_nm,max(1,old.width_nm//2)),
                                coarse_step_nm=min(area_options.coarse_step_nm,area_options.step_nm))
        replacement = _tree_branch(held, old.net, old.layer, section.start, section.end,
            old.width_nm, clearance, local_options,
            max(0,(state_budget-spent)//(len(frame.sections)-i)), 0, failures, work=work)
        spent += sum(work)
        if not replacement:
            detail['failed_section'] = section
            detail['branch_failures'] = tuple(failures)
            detail['branch_step_nm'] = local_options.step_nm
            detail['anchor_clearance'] = tuple(clearance.base.can_track(
                old.net,p,p,old.width_nm,old.layer) for p in (section.start,section.end))
            return None, None, (), spent, 'finite_branch_path_unresolved', detail
        paths.append(replacement)
        held = replace(held, tracks=(*held.tracks, *replacement))
    # Structural capture separately proves exact retained contacts in the full
    # scene and rebases only the PG board; temporary path/area copper is excluded.
    try:
        rebuilt_scene, _ = rebuild_track_branch_ownership(frame, paths)
        candidate, ledger = rebuild_track_branch_ownership(pg_frame, paths)
        from .joint_access import _hard_findings
        if _hard_findings(rebuilt_scene):
            return None, None, (), spent, 'branch_physical_gate_rejected', detail
        required, reason = _prove_branch_requirements(candidate, pg_frame, membership)
    except ValueError as error:
        return None, None, (), spent, str(error), detail
    if reason:
        return None, None, (), spent, reason, detail
    return candidate, ledger, required, spent, 'geometry_proposal', detail


def negotiate_power_area_contacts(board, ownership, plan, fanout_options, area_options, *,
                                  options=None, fixed_reservations=None, on_progress=None):
    """Return a contact proposal or the exact unchanged private preparation.

    Immutable, ambiguous and unattributed occurrences, ordinary exits, macros,
    via arrays and protected zones remain obstacles even in path discovery.
    A successful result still requires fresh native preservation and complete
    area replanning. No accepted or frozen preparation may enter this API.
    """
    options = options or PowerContactFeedbackOptions()
    joint_options = options.joint
    if ownership.frozen or not ownership.matches(board):
        raise ValueError('fresh disposable contact ownership required')
    if plan.source_board != board or plan.snapshot != physical_board_digest(board):
        raise ValueError('same-source power plan required')
    if fixed_reservations is not None and (fixed_reservations.board != board
            or not joint_reservations_preserved(fixed_reservations, board)):
        raise ValueError('fresh fixed ordinary reservations required')
    baseline = fixed_reservations.baseline if fixed_reservations else None
    ordinary_tracks, ordinary_vias = reserved_copper_additions(board,fixed_reservations.reservations) if fixed_reservations else ((),())
    reserved = replace(board,tracks=(*board.tracks,*ordinary_tracks),vias=(*board.vias,*ordinary_vias))
    # Exact prefix ownership includes the unchanged ordinary occurrences.
    reserved_ownership = append_copper_ownership(ownership, board, reserved, 'held-ordinary-reservations')
    by_id = {c.allocation.id:c for c in plan.candidates}
    contexts = {}
    def context(identities):
        if identities not in contexts:
            if any(i not in by_id or by_id[i].allocation.snapshot != plan.snapshot for i in identities):
                raise ValueError('same-source held area candidates required')
            held = tuple(by_id[i] for i in identities)
            scene = replace(reserved,
                tracks=(*reserved.tracks, *(t for c in held for t in c.tracks)),
                vias=(*reserved.vias, *(v for c in held for v in c.vias)),
                zones=(*reserved.zones, *(z for c in held for z in c.zones)), zone_fills=())
            augmented = append_copper_ownership(reserved_ownership, reserved, scene, 'held-area-prefix',
                permitted_fields=frozenset({'zones','zone_fills'}))
            objects = {o.identity:o for o in augmented.occurrences}
            complete = _discovery_occurrences(augmented,joint_options)
            branches = branch_track_occurrences(augmented)
            # Broad branch discovery cannot assume it can also relocate a via
            # in a small owner group: mixed cuts would force full-owner closure
            # again. When branch-only release extends the discovery pool, keep
            # every plated object held. Simple complete-owner scenes retain
            # their existing via relocation behavior.
            negotiable = branches if branches-complete else complete
            relaxed = replace(scene,
                tracks=tuple(t for i,t in enumerate(scene.tracks) if f'track:{i}' not in negotiable),
                vias=tuple(v for i,v in enumerate(scene.vias) if f'via:{i}' not in negotiable))
            discovery = _AreaClearance(relaxed,area_options)
            guards = track_endpoint_guards(scene,negotiable) if branches-complete else ()
            for guard in guards:
                discovery.base.add_track(guard,locked=True)
            discovery.retained_endpoint_guard_count = len(guards)
            contexts[identities] = (scene, augmented, objects, relaxed,
                discovery, _AreaClearance(scene,area_options))
        return contexts[identities]
    membership = {p:n.name for n in board.nets for p in n.pads}
    remaining = dict(sites=joint_options.maximum_site_queries, candidates=joint_options.maximum_generated_candidates,
        bypass=joint_options.maximum_bypass_proposals, states=joint_options.assignment.maximum_search_states,
        checks=joint_options.assignment.maximum_pair_checks, queries=joint_options.assignment.maximum_pair_queries)
    trials, seen, frames = [], set(), set()
    path_states = attempted = roots = 0

    def finish(status, candidate=board, ledger=ownership, reservations=reserved, requirements=()):
        return JointPreparationResult(candidate, ledger, status, baseline, reservations, requirements,
            fixed_reservations.ports if fixed_reservations else (), tuple(trials),
            joint_options.maximum_site_queries-remaining['sites'],
            joint_options.maximum_generated_candidates-remaining['candidates'],
            joint_options.maximum_bypass_proposals-remaining['bypass'],
            joint_options.assignment.maximum_search_states-remaining['states'],
            joint_options.assignment.maximum_pair_checks-remaining['checks'],
            joint_options.assignment.maximum_pair_queries-remaining['queries'], attempted, path_states)

    # Last conflict joins are usually more relevant than initial single-domain
    # failures. Repeated samples never spend another path or dependency trial.
    for failure in reversed(plan.search_failures):
        key = (failure.net, failure.layer, failure.start, failure.end, failure.width_nm, failure.held_candidate_ids)
        if key in seen:
            continue
        seen.add(key)
        if roots >= options.maximum_roots or path_states >= options.maximum_path_states:
            break
        roots += 1
        scene_board, augmented, objects, relaxed, discovery, original = context(failure.held_candidate_ids)
        work = []
        path = _tree_branch(relaxed, failure.net, failure.layer, failure.start, failure.end,
            failure.width_nm, discovery, area_options,
            min(area_options.search_states, max(1,(options.maximum_path_states-path_states)//
                (options.maximum_roots-roots+1))), 0, None, work=work)
        path_states += sum(work)
        trial = dict(root=key[0], layer=key[1].value,
            start_nm=(key[2].x_nm,key[2].y_nm), end_nm=(key[3].x_nm,key[3].y_nm),
            source_digest=plan.snapshot, status='no_complete_relaxed_path', path_search_states=sum(work))
        trial['held_candidate_ids'] = failure.held_candidate_ids
        trial['retained_endpoint_guards'] = discovery.retained_endpoint_guard_count
        trials.append(trial)
        if not path:
            continue
        # All blockers of all segments, freshly queried. Sampled identities
        # from the earlier tree search play no part in this release decision.
        records = {r for t in path for r in original.track_blocker_records(t)}
        seeds = frozenset(r.occurrence for r in records)
        trial.update(path=tuple(path), blockers=tuple(sorted(records, key=lambda r:(r.identity,r.occurrence))),
                     seeds=tuple(sorted(seeds)))
        if not seeds:
            trial['status'] = 'no_contact_blockers'
            continue
        if any(s not in objects or not objects[s].negotiable for s in seeds):
            trial['status'] = 'protected_or_unknown_blocker'
            continue
        if attempted >= joint_options.maximum_trials or min(remaining.values()) <= 0:
            return finish('joint_work_budget_exhausted')
        if seeds <= branch_track_occurrences(ownership):
            attempted += 1
            emit(on_progress, 'power_contact_feedback', 'branch_started', net=failure.net,
                 released=len(seeds), trial=attempted)
            try:
                candidate, ledger, required, used, status, detail = _replace_track_branches(
                    board, ownership, scene_board, augmented, seeds, path, joint_options, area_options,
                    options.maximum_path_states-path_states, membership, options.maximum_branch_obligations)
                path_states += used
                trial.update(detail, status=status, branch_path_search_states=used)
            except ValueError as error:
                trial['status'] = str(error)
                status = trial['status']
            emit(on_progress, 'power_contact_feedback', 'branch_finished', net=failure.net, status=status)
            if status == 'geometry_proposal':
                ordinary = replace(candidate,tracks=(*candidate.tracks,*ordinary_tracks),
                                   vias=(*candidate.vias,*ordinary_vias))
                if (fixed_reservations is None or joint_reservations_preserved(
                        replace(fixed_reservations, board=candidate, ownership=ledger, reservations=ordinary),candidate)):
                    return finish('geometry_proposal', candidate, ledger, ordinary, required)
                trial['status'] = 'previous_exit_lost'
            # A failed branch trial never leaks into a complete-owner trial.
            # Try other roots within the shared allowance instead.
            continue
        try:
            pg_frame = prepare_copper_trial(board, ownership, seeds,
                maximum_occurrences=joint_options.maximum_occurrences)
            # Bind the exact path as held copper before regenerating contacts.
            # It is deliberately removed from the returned PG-only candidate.
            scene = reserve_copper_scene(scene_board, path)
            guarded = append_copper_ownership(augmented, scene_board, scene.board, 'held-power-join')
            frame = prepare_copper_trial(scene.board, guarded, seeds,
                maximum_occurrences=joint_options.maximum_occurrences)
        except ValueError as error:
            trial['status'] = str(error)
            continue
        if not all(_AreaClearance(frame.held, area_options).can_track(
                t.net,t.start,t.end,t.width_nm,t.layer) for t in path):
            trial['status'] = 'incomplete_blocker_witness'
            continue
        required = _requirements(frame, None, membership)
        trial.update(released=frame.released, requirements=frame.requirements)
        if not required or len(required) > joint_options.maximum_obligations:
            trial['status'] = 'unsupported_or_large_dependency_group'
            continue
        frame_key = (frame.released, path)
        if frame_key in frames:
            trial['status'] = 'duplicate_dependency_trial'
            continue
        frames.add(frame_key)
        attempted += 1
        emit(on_progress, 'power_contact_feedback', 'started', net=failure.net,
             released=len(frame.released), obligations=len(required), trial=attempted)
        candidates, observations = _alternatives(frame, required, baseline, fanout_options, joint_options, remaining)
        assignment = replace(joint_options.assignment, maximum_search_states=remaining['states'],
            maximum_pair_checks=remaining['checks'], maximum_pair_queries=remaining['queries'])
        result = allocate_joint_access(frame, required, candidates,
            JointAccessOptions(maximum_candidates=joint_options.maximum_generated_candidates, assignment=assignment))
        remaining['states'] -= result.search_states
        remaining['checks'] -= result.pair_checks
        remaining['queries'] -= result.pair_queries
        trial.update(status=result.status, observations=observations, assessments=result.assessments,
                     selected=tuple(b.id for b in result.selected))
        emit(on_progress, 'power_contact_feedback', 'finished', net=failure.net, status=result.status,
             observations=observations, assessments=result.assessments)
        if result.status != 'geometry_proposal':
            continue
        candidate = _materialize(pg_frame.held, result.selected)
        try:
            ledger = rebuild_copper_ownership(pg_frame, candidate, 'area-contact-feedback',
                uses=_new_contact_uses(candidate, pg_frame))
        except ValueError as error:
            trial['status'] = str(error)
            continue
        ordinary = replace(candidate,tracks=(*candidate.tracks,*ordinary_tracks),vias=(*candidate.vias,*ordinary_vias))
        if fixed_reservations is not None:
            updated = replace(fixed_reservations, board=candidate, ownership=ledger, reservations=ordinary)
            if not joint_reservations_preserved(updated, candidate):
                trial['status'] = 'previous_exit_lost'
                continue
        return finish('geometry_proposal', candidate, ledger, ordinary, required)
    return finish('finite_power_contact_pool_unresolved')
