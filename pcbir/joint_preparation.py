"""Bounded automatic signal/contact alternatives on a private preparation.

Only exact recorded blockers can authorize a dependency release. The result
is a contact geometry proposal; normal area planning and fresh native gates
still decide whether a power preparation can be accepted.
"""
from dataclasses import asdict, dataclass, replace, is_dataclass
from itertools import chain

from .copper_ownership import (CopperUse, append_copper_ownership, prepare_copper_trial,
    rebuild_copper_ownership, prospective_contact_uses)
from .drc import physical_board_digest
from .escape_assignment import EscapeAssignmentOptions
from .fanout import route_fanout, _package_candidates, _two_leg_candidates, _legal_choices
from .joint_access import (AccessBundle, AccessRequirement, JointAccessOptions,
                           allocate_joint_access, _hard_findings, _prove_requirement, _materialize)
from .physical import BoardSide, CopperLayer, Point, RouteKind, Via
from .pin_escape import RoutingAccess, verified_fanout_path, verified_routing_access, checked_access_paths
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex, _geometry_identity


@dataclass(frozen=True, slots=True)
class JointPreparationOptions:
    maximum_trials: int = 4
    maximum_roots: int = 8
    maximum_occurrences: int = 64
    maximum_obligations: int = 12
    alternatives_per_obligation: int = 8
    sites_per_signal: int = 4096
    sites_per_contact: int = 128
    maximum_site_queries: int = 24_576
    maximum_generated_candidates: int = 128
    maximum_bypass_proposals: int = 64
    maximum_reserved_occurrences: int = 1024
    assignment: EscapeAssignmentOptions = EscapeAssignmentOptions()

    def __post_init__(self):
        for name in self.__dataclass_fields__:
            if name != 'assignment' and (type(getattr(self, name)) is not int or getattr(self, name) <= 0):
                raise ValueError('joint preparation bounds must be positive integers')


@dataclass(frozen=True)
class JointPreparationResult:
    board: object
    ownership: object
    status: str
    baseline: object = None
    reservations: object = None
    requirements: tuple = ()
    ports: tuple = ()
    trials: tuple = ()
    site_queries: int = 0
    generated_candidates: int = 0
    bypass_proposals: int = 0
    search_states: int = 0
    pair_checks: int = 0
    pair_queries: int = 0
    dependency_trials: int = 0
    path_search_states: int = 0

    def report(self):
        def plain(value):
            if is_dataclass(value):
                return plain(asdict(value))
            if isinstance(value, dict):
                return {k:plain(v) for k,v in value.items()}
            if isinstance(value, (tuple,list)):
                return tuple(plain(v) for v in value)
            return value
        result = dict(status=self.status, board_digest=physical_board_digest(self.board),
            baseline_pending=[str(p) for p in self.baseline.pending_pads] if self.baseline else [],
            trials=plain(self.trials), site_queries=self.site_queries, generated_candidates=self.generated_candidates,
            bypass_proposals=self.bypass_proposals, search_states=self.search_states,
            pair_checks=self.pair_checks, pair_queries=self.pair_queries,
            dependency_trials=self.dependency_trials, path_search_states=self.path_search_states,
            reserved_ports=len(self.ports),
            reservation_digest=physical_board_digest(self.reservations) if self.reservations is not None else None,
            native_acceptance=False)
        if self.status == 'early_reservations' and self.baseline is not None:
            result['early_fanout'] = dict(
                added_tracks=self.baseline.added_track_count, added_vias=self.baseline.added_via_count,
                assignment=asdict(self.baseline.assignment) if self.baseline.assignment else None,
                unobserved_phases=sorted({phase for p in self.baseline.pin_analysis if p.access_diagnostics
                                         for phase in p.access_diagnostics.unobserved_phases}))
        return result


def _signal_uses(board, fanout):
    membership = {p: n.name for n in board.nets for p in n.pads}
    index = RoutingClearanceIndex(fanout.board)
    uses = []
    for pad, anchor in sorted(fanout.routing_accesses.items()):
        verify = verified_fanout_path if isinstance(anchor, Point) else verified_routing_access
        path = verify(fanout.board, pad, membership[pad], anchor, index)
        if path is None:
            raise ValueError('ordinary reservation lacks a fresh terminal witness')
        position = anchor if isinstance(anchor, Point) else anchor.launch_position
        vias = tuple(v for v in fanout.board.vias if v.net == membership[pad] and v.position == position)
        uses.append(CopperUse(f'signal:{pad.component}.{pad.pad}', 'signal', (pad,), path, vias))
    return tuple(uses)


def ordinary_preparation_scope(board, only_nets=None):
    protected = {d.net for d in board.power_domains} | {z.net for z in board.zones}
    protected.update(r.net for r in board.net_routing_rules if r.kind is not RouteKind.GENERAL)
    return frozenset(n.name for n in board.nets if n.name not in protected
                     and (only_nets is None or n.name in only_nets))


def prepare_ordinary_reservations(board, ownership, fanout_options, *, only_nets=None,
                                 reserved_tracks=(),reserved_vias=()):
    """Bounded ordinary surface-first planning before contact producers run.

    The returned copper is a temporary obstacle scene, never a power prefix or
    native acceptance. Each retained port has an exact fresh terminal witness.
    """
    if ownership.frozen or not ownership.matches(board):
        raise ValueError('fresh disposable contact ownership required')
    scope = ordinary_preparation_scope(board, only_nets)
    depth = 1 if fanout_options.surface_preference_depth is None else fanout_options.surface_preference_depth
    from .copper_reservations import reserve_copper_scene
    scene=reserve_copper_scene(board,reserved_tracks,reserved_vias)
    baseline = route_fanout(scene.board, replace(fanout_options, prefer_surface_escapes=True,
                                         surface_preference_depth=depth, surface_fallback=True), only_nets=scope)
    _signal_uses(board, baseline)  # Verify actual paths, including reused vias.
    return JointPreparationResult(board, ownership, 'early_reservations', baseline,
        baseline.board, ports=tuple(baseline.routing_accesses.items()))


def refresh_ordinary_reservations(reservation, board, ownership):
    """Bind unchanged reserved occurrences to a freshly prepared power scene."""
    from .copper_reservations import reserve_copper_scene
    if ownership.frozen or not ownership.matches(board):
        raise ValueError('fresh disposable contact ownership required')
    original = reservation.board
    if (board.tracks[:len(original.tracks)] != original.tracks
            or board.vias[:len(original.vias)] != original.vias):
        raise ValueError('early reservation source prefix changed')
    tracks = reservation.reservations.tracks[len(original.tracks):]
    vias = reservation.reservations.vias[len(original.vias):]
    scene = reserve_copper_scene(board, tracks, vias)
    updated = replace(reservation, board=board, ownership=ownership, reservations=scene.board)
    if not joint_reservations_preserved(updated, board):
        raise ValueError('early ordinary reservations obstructed')
    return updated


def _requirements(frame, root, membership, additional=None):
    uses = {u.id: u for u in frame.ownership.uses}
    links = {'bypass:' + _geometry_identity(l): l for l in frame.held.decoupling_links}
    required = []
    for identity in frame.requirements:
        use = uses[identity]
        if use.kind == 'bypass':
            link = links.get(identity)
            if link is None:
                return ()
            required.append(AccessRequirement(identity, 'bypass', link.net, use.terminals, link=link))
        elif use.kind == 'contact':
            net = membership[use.terminals[0]]
            layers = tuple(dict.fromkeys(l for z in frame.held.zones if z.net == net for l in z.layers))
            if not layers:
                return ()
            required.append(AccessRequirement(identity, 'contact', net, use.terminals, layers))
        elif use.kind == 'signal':
            required.append(AccessRequirement(identity, 'signal', membership[use.terminals[0]], use.terminals))
        else:
            return ()
    if additional is None and root is not None:
        additional = AccessRequirement(f'signal:{root.component}.{root.pad}', 'signal', membership[root], (root,))
    if additional is not None and additional.id not in {r.id for r in required}:
        required.append(additional)
    return tuple(required)


@dataclass(frozen=True)
class _BypassProbe:
    pad: object
    requirement: object
    access_diagnostics: object


def _bypass_probes(board, links, index, objects, fanout_options, options, remaining):
    """Observe blocked local launches, retaining only exact negotiable roots.

    A bounded bypass failure can require relocating a generated return/contact
    even when every ordinary exit already exists. Pads and held signal exits
    remain protected. These probes are observations, never copper candidates.
    """
    from .access_diagnostics import AccessAttempt, AccessDiagnostics
    from .decoupling_paths import _local_launch_sites
    from .routing_vias import physical_via_span
    poses = {p.reference:p for p in board.placements}
    membership = {p:n.name for n in board.nets for p in n.pads}
    roots = []
    for link in links:
        requirement = AccessRequirement('bypass:'+_geometry_identity(link),'bypass',link.net,
            (link.target,link.capacitor,link.return_pad),link=link)
        for terminal in (link.capacitor,link.target,link.return_pad):
            if len(roots) >= options.maximum_roots or not remaining['sites']:
                return tuple(roots)
            pose = poses[terminal.component]
            lands = tuple(p for p in board.footprints[pose.footprint].pads if p.number==terminal.pad)
            if len(lands)!=1:
                continue
            pad=lands[0];start=transformed_local_point(pose,pad.position);net=membership[terminal]
            side=CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
            span=physical_via_span(board,board.stackup.copper_layers[0],board.stackup.copper_layers[-1])
            if span is None:
                continue
            settings=replace(fanout_options,step_nm=fanout_options.refinement_step_nm)
            sites=_local_launch_sites(board,pose,pad,settings,
                min(options.sites_per_contact,remaining['sites']),clearance=index,net=net)
            samples=[];visited=0;protected=0
            def observe(point,path,via,records):
                nonlocal protected
                if not records:
                    return
                if any(r.occurrence not in objects or not objects[r.occurrence].negotiable for r in records):
                    protected+=1
                    return
                if len(samples)<24:
                    samples.append(AccessAttempt(point,'bypass','negotiable_contact_blocked',path,via,tuple(records)))
            rule=next((r for r in board.net_routing_rules if r.net==net),None)
            width=rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
            for point in sites:
                if not remaining['sites']:
                    break
                remaining['sites']-=1;visited+=1
                via=Via(net,point,board.rules.default_via_size_nm,board.rules.default_via_drill_nm,*span)
                records=index.via_blocker_records(via)
                if records:
                    observe(point,(),via,records)
                else:
                    def rejected(path,track,reason):
                        observe(point,path,via,index.track_blocker_records(track))
                    tuple(checked_access_paths(board,index,net,start,point,width,side,
                        allow_orthogonal=False,on_rejected=rejected))
            diagnostics=AccessDiagnostics(physical_board_digest(board),visited,
                (('bypass','protected_local_probes',protected),),tuple(samples),len(samples)==24,'finite_bypass_launch_pool')
            roots.append(_BypassProbe(terminal,requirement,diagnostics))
    return tuple(roots)


def _signal_choices(board, requirement, fanout_options, limit, sites):
    """Reuse ordinary launch predicates and site families with a shared cap."""
    from .hard_macros import macro_owned_pads
    if requirement.terminals[0] in macro_owned_pads(board):
        return (), 0
    pad_ref = requirement.terminals[0]
    pose = next(p for p in board.placements if p.reference == pad_ref.component)
    pads = tuple(p for p in board.footprints[pose.footprint].pads if p.number == pad_ref.pad)
    if len(pads) != 1:
        return (), 0
    pad = pads[0]
    start = transformed_local_point(pose, pad.position)
    fine = replace(fanout_options, step_nm=fanout_options.refinement_step_nm,
                   maximum_two_leg_candidates=fanout_options.maximum_refined_two_leg_candidates)
    endpoints = chain(_package_candidates(board, pose, pad, fanout_options),
        _two_leg_candidates(start, pose.position, fanout_options) if fanout_options.two_leg_escapes else (),
        _package_candidates(board, pose, pad, fine),
        _two_leg_candidates(start, pose.position, fine) if fanout_options.two_leg_escapes else ())
    index = RoutingClearanceIndex(board)
    seen, choices, queries = set(), [], 0
    for point in endpoints:
        if point in seen:
            continue
        seen.add(point)
        if queries >= sites:
            break
        queries += 1
        for path, via in _legal_choices(board, index, requirement.net, start, pose, pad, (point,), multiple_orders=True):
            choices.append((path, (() if via is None else (via,)), ((pad_ref, point),)))
            if len(choices) >= limit:
                return tuple(choices), queries
    return tuple(choices), queries


def _incumbent(board, requirement, use, baseline):
    if use is None:
        return None
    if requirement.kind == 'signal':
        port = baseline.routing_accesses.get(use.terminals[0])
        return (use.tracks, use.vias, ((use.terminals[0], port),)) if port is not None else None
    if requirement.kind == 'bypass':
        return use.tracks, use.vias, ()
    # Verify the old complete witness against this fresh held scene too.
    index = RoutingClearanceIndex(_materialize(board, (AccessBundle('old', 'old', physical_board_digest(board),
        {requirement.id}, use.tracks, use.vias),)))
    for via in use.vias:
        ports = ((use.terminals[0], via.position),)
        if _prove_requirement(index.board, requirement, dict(ports), index):
            return use.tracks, use.vias, ports
    return None


def _alternatives(frame, requirements, baseline, fanout_options, options, remaining):
    from .plane import propose_plane_contacts, PlaneStitchOptions
    from .decoupling_paths import propose_crosslayer_alternatives
    uses = {u.id: u for u in frame.ownership.uses}
    digest = physical_board_digest(frame.held)
    candidates, observations = [], []
    for r in requirements:
        limit = min(options.alternatives_per_obligation, remaining['candidates'])
        if not limit:
            return tuple(candidates), (*observations, (r.id, 'candidate_budget_exhausted'))
        choices = []
        old = _incumbent(frame.held, r, uses.get(r.id), baseline)
        if old is not None:
            choices.append(old)
        available = min(options.sites_per_signal if r.kind == 'signal' else options.sites_per_contact, remaining['sites'])
        reason = 'site_budget_exhausted'
        if available and r.kind == 'signal':
            generated, used = _signal_choices(frame.held, r, fanout_options, limit, available)
            remaining['sites'] -= used
            choices.extend(generated)
            reason = 'finite_signal_pool'
        elif available and r.kind == 'contact':
            proposed = propose_plane_contacts(frame.held, r.terminals[0],
                PlaneStitchOptions(include_surface_zones=True, maze_step_nm=fanout_options.refinement_step_nm),
                maximum_alternatives=limit, maximum_sites=available)
            remaining['sites'] -= proposed.site_queries
            for (tracks, via), target in zip(proposed.choices, proposed.targets):
                if via is not None:
                    choices.append((tracks, (via,), ((r.terminals[0], via.position),)))
                elif target is not None and any(v.net == r.net and v.position == target for v in frame.held.vias):
                    # Preserve the actual reused plated target. A surface
                    # lead-in is not an inner-layer tail, and an empty suffix
                    # can still reuse a complete existing physical witness.
                    choices.append((tracks, (), ((r.terminals[0], target),)))
                elif tracks:
                    # A reused plated transition may have an inner-layer tail
                    # ending in the zone. Keep its actual layer-specific port;
                    # the far endpoint must not be relabelled as a new via.
                    tail = tuple(t for t in tracks if t.layer is tracks[-1].layer)
                    port = RoutingAccess(tail[-1].end, tail[-1].layer, tail[0].start, tail)
                    choices.append((tracks, (), ((r.terminals[0], port),)))
            reason = proposed.stop_reason
        elif r.kind == 'bypass' and remaining['bypass']:
            proposed = propose_crosslayer_alternatives(frame.held, r.link, fanout_options=fanout_options,
                maximum_alternatives=limit, maximum_proposals=remaining['bypass'])
            remaining['bypass'] -= proposed.attempted
            choices.extend((p.created_tracks, p.created_vias, ()) for p in proposed.choices)
            reason = proposed.stop_reason
            # Surface feeds keep their original explicit-association predicate.
            from .decoupling_paths import _land
            a, b = _land(frame.held, r.link.target), _land(frame.held, r.link.capacitor)
            if a and b and a[2].layer is b[2].layer:
                rule = next((x for x in frame.held.net_routing_rules if x.net == r.net), None)
                width = rule.width_nm if rule and rule.width_nm else frame.held.rules.default_track_width_nm
                paths = checked_access_paths(frame.held, RoutingClearanceIndex(frame.held), r.net,
                    a[2].position, b[2].position, width, a[2].layer)
                choices.extend((path, (), ()) for path in paths)
        unique = tuple(dict.fromkeys(choices))[:limit]
        for i, (tracks, vias, ports) in enumerate(unique):
            candidates.append(AccessBundle(f'{r.id}:{i}', r.id, digest, {r.id}, tracks, vias, ports, frozenset(vias)))
        remaining['candidates'] -= len(unique)
        observations.append((r.id, reason, len(unique)))
    return tuple(candidates), tuple(observations)


def _new_contact_uses(board, frame):
    from .package_access import _verified_bypass_links
    paths, vias = {}, {}
    links = _verified_bypass_links(board, {l.net for l in board.decoupling_links}, witness_paths=paths, witness_vias=vias)
    old = {u.id: u for u in frame.ownership.uses}
    uses = {u.id: u for u in prospective_contact_uses(board,
        (old[r].terminals[0] for r in frame.requirements if old[r].kind == 'contact'))}
    for link in links:
        identity = 'bypass:' + _geometry_identity(link)
        if identity in frame.requirements:
            uses[identity] = CopperUse(identity, 'bypass', old[identity].terminals, paths.get(link, ()), vias.get(link, ()))
    return tuple(uses[r] for r in frame.requirements if r in uses)


def negotiate_preparation_contacts(board, ownership, fanout_options, *, options=None, only_nets=None,
                                   fixed_reservations=None, bypass_links=()):
    """One bounded automatic controller; accepted/frozen preparations reject."""
    options = options or JointPreparationOptions()
    if ownership.frozen or not ownership.matches(board):
        raise ValueError('fresh disposable contact ownership required')
    membership = {p: n.name for n in board.nets for p in n.pads}
    scope = ordinary_preparation_scope(board, only_nets)
    if bypass_links and (not isinstance(bypass_links,frozenset) or not bypass_links<=frozenset(board.decoupling_links)):
        raise ValueError('declared immutable bypass subset required')
    if fixed_reservations is None:
        baseline = route_fanout(board, fanout_options, only_nets=scope)
    else:
        if fixed_reservations.board != board or not joint_reservations_preserved(fixed_reservations,board):
            raise ValueError('fresh fixed ordinary reservations required')
        baseline = replace(fixed_reservations.baseline,board=fixed_reservations.reservations)
    augmented = append_copper_ownership(ownership, board, baseline.board, 'ordinary-reservations',
        uses=_signal_uses(board, baseline), trial_mutable=fixed_reservations is None)
    objects = {o.identity: o for o in augmented.occurrences}
    original_objects = {o.identity: o for o in ownership.occurrences}
    index = RoutingClearanceIndex(baseline.board)
    remaining = dict(sites=options.maximum_site_queries, candidates=options.maximum_generated_candidates,
                     bypass=options.maximum_bypass_proposals, states=options.assignment.maximum_search_states,
                     checks=options.assignment.maximum_pair_checks, queries=options.assignment.maximum_pair_queries)
    trials, seen, seen_frames, attempted = [], set(), set(), 0
    roots = sorted((p for p in baseline.pin_analysis if p.pad in baseline.pending_pads),
                   key=lambda p: (not p.legal_candidate_count, p.pad))[:options.maximum_roots]
    if bypass_links:
        roots = _bypass_probes(baseline.board,tuple(l for l in board.decoupling_links if l in bypass_links),
            index,objects,fanout_options,options,remaining)
        trials.extend(dict(root=str(r.pad),requirement=r.requirement.id,status='bypass_launch_probe',
            observed_sites=r.access_diagnostics.observed_site_queries,
            eligible_samples=len(r.access_diagnostics.samples),outcomes=r.access_diagnostics.outcome_counts,
            source_digest=r.access_diagnostics.source_digest) for r in roots)
    def finish(status, candidate=board, ledger=ownership, proposal=None, requirements=(), ports=None):
        if proposal is None:
            proposal = baseline.board
        if ports is None:
            ports = tuple(baseline.routing_accesses.items())
        return JointPreparationResult(candidate, ledger, status, baseline, proposal, requirements, ports,
            tuple(trials), options.maximum_site_queries-remaining['sites'],
            options.maximum_generated_candidates-remaining['candidates'],
            options.maximum_bypass_proposals-remaining['bypass'],
            options.assignment.maximum_search_states-remaining['states'],
            options.assignment.maximum_pair_checks-remaining['checks'],
            options.assignment.maximum_pair_queries-remaining['queries'], attempted)
    for root in roots:
        for sample in root.access_diagnostics.samples if root.access_diagnostics else ():
            records = {b for t in sample.tracks for b in index.track_blocker_records(t)}
            if sample.via is not None:
                records.update(index.via_blocker_records(sample.via))
            seeds = frozenset(r.occurrence for r in records)
            if not seeds or seeds in seen:
                continue
            seen.add(seeds)
            # Require an actually blocked contact producer, never a net-level
            # release or another attempt at the existing ordinary-only CSP.
            if not any(s in original_objects and original_objects[s].negotiable for s in seeds):
                continue
            if any(s not in objects or not objects[s].negotiable for s in seeds):
                trials.append(dict(root=str(root.pad), status='protected_blocker', seeds=sorted(seeds)))
                continue
            if attempted >= options.maximum_trials or min(remaining.values()) <= 0:
                return finish('joint_work_budget_exhausted')
            try:
                frame = prepare_copper_trial(baseline.board, augmented, seeds, maximum_occurrences=options.maximum_occurrences)
                pg_seeds = tuple(s for s in frame.released if s in original_objects)
                pg_frame = prepare_copper_trial(board, ownership, pg_seeds, maximum_occurrences=options.maximum_occurrences)
            except ValueError as error:
                trials.append(dict(root=str(root.pad), status=str(error), seeds=sorted(seeds)))
                continue
            required = _requirements(frame, root.pad, membership,getattr(root,'requirement',None))
            if not required or len(required) > options.maximum_obligations:
                trials.append(dict(root=str(root.pad), status='unsupported_or_large_dependency_group',
                                   released=len(frame.released), obligations=len(frame.requirements)))
                continue
            # Different blocked samples can release the same complete owner
            # group. Regeneration sees only this held scene and requirements;
            # repeating it would consume another trial without new information.
            frame_key = (frozenset(frame.released), tuple(r.id for r in required))
            if frame_key in seen_frames:
                trials.append(dict(root=str(root.pad), status='duplicate_dependency_trial',
                                   seeds=sorted(seeds), released=frame.released))
                continue
            seen_frames.add(frame_key)
            attempted += 1
            candidates, observations = _alternatives(frame, required, baseline, fanout_options, options, remaining)
            assignment = replace(options.assignment, maximum_search_states=remaining['states'],
                maximum_pair_checks=remaining['checks'], maximum_pair_queries=remaining['queries'])
            result = allocate_joint_access(frame, required, candidates, JointAccessOptions(
                maximum_candidates=options.maximum_generated_candidates, assignment=assignment))
            remaining['states'] -= result.search_states
            remaining['checks'] -= result.pair_checks
            remaining['queries'] -= result.pair_queries
            trial = dict(root=str(root.pad), status=result.status, seeds=sorted(seeds),
                released=frame.released, requirements=frame.requirements, observations=observations,
                selected=[b.id for b in result.selected], assessments=result.assessments)
            trials.append(trial)
            if result.status != 'geometry_proposal':
                continue
            ports = {**baseline.routing_accesses, **{p:a for b in result.selected for p,a in b.ports}}
            fresh = RoutingClearanceIndex(result.board)
            if any((verified_fanout_path if isinstance(a, Point) else verified_routing_access)(
                    result.board, p, membership[p], a, fresh) is None for p,a in ports.items()):
                trial['status'] = 'previous_exit_or_new_port_lost'
                continue
            contacts = tuple(b for b in result.selected if all(
                next(r for r in required if r.id == identity).kind != 'signal' for identity in b.covers))
            candidate = _materialize(pg_frame.held, contacts)
            try:
                ledger = rebuild_copper_ownership(pg_frame, candidate, 'joint-contacts',
                                                  uses=_new_contact_uses(candidate, pg_frame))
                if getattr(root,'requirement',None) is not None:
                    from .package_access import _verified_bypass_links
                    from .copper_ownership import record_copper_uses
                    r=root.requirement;paths,vias={},{}
                    if r.link not in _verified_bypass_links(candidate,{r.net},witness_paths=paths,witness_vias=vias):
                        raise ValueError('new bypass root lacks a fresh complete witness')
                    ledger=record_copper_uses(ledger,candidate,(CopperUse(r.id,'bypass',r.terminals,
                        paths.get(r.link,()),vias.get(r.link,())),))
            except ValueError as error:
                trial['status'] = str(error)
                continue
            reservations = replace(candidate, tracks=result.board.tracks, vias=result.board.vias)
            return finish('geometry_proposal', candidate, ledger, reservations, required, tuple(ports.items()))
    return finish('finite_joint_pool_unresolved' if baseline.pending_pads or bypass_links else 'not_needed')


def joint_reservations_preserved(result, board):
    """Fresh physical proof after area growth; ordinary ends are not admitted."""
    if result is None or result.reservations is None:
        return True
    from .copper_reservations import reserved_copper_additions
    try:
        additions, vias = reserved_copper_additions(result.board,result.reservations)
    except ValueError:
        return False
    candidate = replace(board, tracks=(*board.tracks,*additions), vias=(*board.vias,*vias))
    if _hard_findings(candidate):
        return False
    index = RoutingClearanceIndex(candidate)
    membership = {p:n.name for n in board.nets for p in n.pads}
    return all((verified_fanout_path if isinstance(a, Point) else verified_routing_access)(
        candidate, p, membership[p], a, index) is not None for p,a in result.ports)


def joint_routing_corridors(result, *, maximum_occurrences=1024):
    """Conservative envelopes of freshly verified ordinary launch reservations.

    These are temporary planning obstacles, never committed source keepouts or
    fabricated signal copper. Every annulus is protected on its actual plated
    span. Existing power/return geometry is not moved or exempted by this API.
    """
    from .geometry import RoundedConvexShape
    from .physical import CopperKeepout, PolygonRing, PolygonWithHoles
    from .power_geometry import _conflict_geometry
    if type(maximum_occurrences) is not int or maximum_occurrences <= 0:
        raise ValueError('positive reservation occurrence bound required')
    if not joint_reservations_preserved(result, result.board):
        raise ValueError('fresh verified ordinary reservations required')
    if result.reservations is None:
        return ()
    from .copper_reservations import reserved_copper_additions
    tracks, vias = reserved_copper_additions(result.board,result.reservations)
    if len(tracks) + len(vias) > maximum_occurrences:
        raise ValueError('ordinary reservation occurrence budget exhausted')
    board = result.board
    rules = {r.net:r for r in board.net_routing_rules}
    corridors = []
    for kind, values in (('track',tracks),('via',vias)):
        for i, value in enumerate(values):
            if kind == 'track':
                spine, radius, layers = (value.start,value.end), (value.width_nm+1)//2, (value.layer,)
            else:
                a,b = sorted((board.stackup.copper_layers.index(value.from_layer),
                              board.stackup.copper_layers.index(value.to_layer)))
                spine, radius, layers = (value.position,), (value.size_nm+1)//2, board.stackup.copper_layers[a:b+1]
            rule = rules.get(value.net)
            # The planner adds its normal gap. Preserve a larger source-side
            # gap too, without treating a synthetic corridor as a new net.
            extra = max(0,(rule.clearance_nm or 0)-board.rules.minimum_clearance_nm) if rule else 0
            envelope = _conflict_geometry(RoundedConvexShape(spine, radius+extra+2))
            vertices = tuple(Point(round(x),round(y)) for x,y in list(envelope.exterior.coords)[:-1])
            identity = 'ordinary-reservation:' + _geometry_identity((physical_board_digest(board),kind,i,value))[:24]
            corridors.append(CopperKeepout(identity, tuple(layers), PolygonWithHoles(PolygonRing(vertices))))
    return tuple(corridors)
