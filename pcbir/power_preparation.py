"""Native-backed, append-only power/return preparation before ordinary exits.

The explicit bypass ledger is not relabelled as plane connectivity. This owns
the complete accepted prefix and a separate, snapshot-bound native ledger.
Changed copper requires fresh native evidence; changed source requires a new
preparation. No accepted explicit return is removed to make room for a plane.
"""
from collections import Counter
from dataclasses import dataclass, fields, replace

from .drc import physical_board_digest
from .progress import emit, critical_progress
from .copper_ownership import (begin_copper_ownership, append_copper_ownership,
                               prospective_contact_uses, CopperUse)


@dataclass(frozen=True)
class PowerPreparation:
    source: object
    board: object
    critical: object
    plan: object = None
    baseline_evidence: object = None
    evidence: object = None
    status: str = 'unresolved'
    observation: object = None
    native_trials: tuple = ()
    return_contact_observations: tuple = ()
    ownership: object = None
    joint_access: object = None
    joint_access_validation: object = None
    contact_snapshot: object = None
    contact_completion_trials: tuple = ()
    area_contact_trials: tuple = ()
    package_patterns: tuple = ()

    @property
    def accepted(self):
        return self.status == 'native_connected'

    def report(self):
        return dict(status=self.status, source_digest=physical_board_digest(self.source),
            accepted_board_digest=physical_board_digest(self.board),
            native_filled_digest=self.evidence.filled_board_digest if self.evidence else None,
            native_report_digest=self.evidence.report_digest if self.evidence else None,
            held_tracks=len(self.board.tracks), held_vias=len(self.board.vias),
            held_zones=len(self.board.zones), observation=self.observation,
            native_trials=self.native_trials,
            return_contact_observations=[o.report() for o in self.return_contact_observations],
            copper_ownership=self.ownership.report() if self.ownership is not None else None,
            joint_access=self.joint_access.report() if self.joint_access is not None else None,
            joint_access_validation=self.joint_access_validation,
            contact_completion_trials=self.contact_completion_trials,
            area_contact_trials=self.area_contact_trials,
            package_patterns=self.package_patterns,
            contact_snapshot_digest=physical_board_digest(self.contact_snapshot.board) if self.contact_snapshot else None)


@dataclass(frozen=True)
class ContactPreparationSnapshot:
    source: object
    critical: object
    board: object
    ownership: object
    selected_nets: tuple
    returns: frozenset
    return_observations: tuple = ()
    source_digest: str = ''
    early_reservations: object = None
    contact_completion_trials: tuple = ()
    package_patterns: tuple = ()
    package_reserved_tracks: tuple = ()
    package_reserved_vias: tuple = ()
    package_reservation_digest: str = ''

    def __post_init__(self):
        if not self.source_digest:
            object.__setattr__(self, 'source_digest', physical_board_digest(self.source))
        object.__setattr__(self, 'selected_nets', tuple(self.selected_nets))
        object.__setattr__(self, 'returns', frozenset(self.returns))
        if not self.package_reservation_digest:
            from .routing_clearance import _geometry_identity
            object.__setattr__(self,'package_reservation_digest',_geometry_identity(
                (self.source_digest,self.package_reserved_tracks,self.package_reserved_vias)))

    def matches(self):
        from .critical import CriticalRoutingStatus
        from .routing_clearance import _geometry_identity,RoutingClearanceIndex
        if self.package_reservation_digest!=_geometry_identity(
                (self.source_digest,self.package_reserved_tracks,self.package_reserved_vias)):
            return False
        produced=set(self.selected_nets)|{l.return_net for l in self.source.decoupling_links if l.net in self.selected_nets}
        if any(v.net in produced for v in (*self.package_reserved_tracks,*self.package_reserved_vias)):
            return False
        if self.package_reserved_tracks or self.package_reserved_vias:
            index=RoutingClearanceIndex(self.board)
            if (not index.candidate_vias_clear(self.package_reserved_vias) or any(
                    not index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in self.package_reserved_tracks)):
                return False
        held = self.critical.board
        if (self.source_digest != physical_board_digest(self.source)
                or any(getattr(self.source, f.name) != getattr(held, f.name) for f in fields(held)
                if f.name not in {'tracks', 'vias', 'metadata'})
                or held.tracks[:len(self.source.tracks)] != self.source.tracks
                or held.vias[:len(self.source.vias)] != self.source.vias):
            return False
        if (self.critical.status is CriticalRoutingStatus.FAILED or self.ownership.frozen
                or not self.ownership.matches(self.board)
                or self.ownership.source_digest != physical_board_digest(held)
                or self.board.tracks[:len(held.tracks)] != held.tracks
                or self.board.vias[:len(held.vias)] != held.vias):
            return False
        if self.early_reservations is not None and (
                self.early_reservations.board != self.board
                or self.early_reservations.ownership != self.ownership):
            return False
        if any(getattr(held, f.name) != getattr(self.board, f.name) for f in fields(held)
               if f.name not in {'tracks', 'vias', 'metadata'}):
            return False
        return all(not o.trial_mutable and o.producer == 'held-source'
            for o in self.ownership.occurrences if o.index < (
                len(held.tracks) if o.kind == 'track' else len(held.vias)))


def join_return_contacts(board, required, options):
    """Complete declared return contacts through real plated components.

    Applies the same bounded, exact component join as power access. No source,
    zone fill or new via is inferred. The primary layers come from actual
    authored return zones; only permitted local branch layers are searched.
    """
    from .power_geometry import DomainObligation, _join_layer_contacts
    if not options.maximum_branch_layers:
        return board, ()
    required = frozenset(required)
    outer = {board.stackup.copper_layers[0], board.stackup.copper_layers[-1]}
    observations = []
    for net in board.nets:
        pads = tuple(sorted(required.intersection(net.pads)))
        if not pads:
            continue
        layers = tuple(l for l in board.stackup.copper_layers if l not in outer
                       and any(z.net == net.name and l in z.layers for z in board.zones))
        obligation = DomainObligation(net.name, (), (), pads, (), 'plane_contacts_only')
        for layer in layers[:options.maximum_layer_alternatives]:
            board = _join_layer_contacts(board, obligation, layer, options, observations)
    return board, tuple(observations)


def assess_native_return_contacts(board, evidence, required):
    """Require every selected return land, not merely one target per bypass."""
    from .physical import PadReference
    from .plane_verify import native_object_map
    from .filled_connectivity import filled_component_roots, native_item_identity
    required = frozenset(required)
    if not required:
        return dict(status='not_requested', nets=[])
    if (evidence is None or not evidence.matches(board)
            or not evidence.filled_board_digest or not evidence.report_digest):
        return dict(status='fresh_native_evidence_required', nets=[])
    if any(i.kind == 'unknown' for f in evidence.native_findings for i in f.items):
        return dict(status='unmapped_native_object', nets=[])
    try:
        import shapely  # noqa: F401
    except ImportError:
        return dict(status='geometry_dependency_unavailable', nets=[])
    membership = {p: n.name for n in board.nets for p in n.pads}
    unknown = sorted(required-membership.keys())
    if unknown:
        return dict(status='unknown_return_terminals', nets=[], terminals=[str(p) for p in unknown])
    nets = frozenset(membership[p] for p in required)
    roots = filled_component_roots(board, evidence, nets)
    items = {}
    for item in native_object_map(board).values():
        if item.kind == 'pad':
            items.setdefault(PadReference(item.component, item.pad), []).append(item)
    rows = []
    for net in sorted(nets):
        groups, unmapped = {}, []
        for pad in sorted(p for p in required if membership[p] == net):
            contacts = items.get(pad, ())
            components = {roots.get(native_item_identity(i)) for i in contacts if i.net == net}
            if not components or None in components:
                unmapped.append(str(pad))
            for root in components-{None}:
                groups.setdefault(root, set()).add(str(pad))
        rows.append(dict(net=net, status='native_connected' if len(groups) == 1 and not unmapped
                         else 'required_return_contacts_open',
                         groups=sorted(sorted(g) for g in groups.values()), unmapped_terminals=unmapped))
    return dict(status='native_connected' if all(r['status'] == 'native_connected' for r in rows)
                else 'required_return_contacts_open', nets=rows)


def preparation_preserved(preparation, board):
    """Exact source/pose intent and occurrence-counted prefix ownership."""
    held = preparation.board
    if not preparation.accepted:
        return False
    if any(getattr(held, f.name) != getattr(board, f.name) for f in fields(held)
           if f.name not in {'tracks', 'vias', 'zones', 'zone_fills', 'metadata'}):
        return False
    diagnostics = {'critical_routing', 'detailed_routing', 'fabrication_ready',
                   'plane_stitching', 'package_access'}
    if ({k: v for k, v in held.metadata.items() if k not in diagnostics}
            != {k: v for k, v in board.metadata.items() if k not in diagnostics}):
        return False
    return all(not (Counter(getattr(held, name)) - Counter(getattr(board, name)))
               for name in ('tracks', 'vias', 'zones'))


def required_return_contacts(board, plan):
    """Exact return lands selected by this plan's local associations."""
    from .decoupling_paths import _return_targets
    return frozenset(p for o in plan.obligations for link in o.local_links
                     for p in (link.return_pad, *_return_targets(board, link)))


def validate_power_preparation(preparation, board, evidence, options):
    """Recompute association/fragment joins; never trust stored connected flags."""
    from .power_extract import assess_power_plan_fill, compare_power_fill_connectivity
    from .decoupling_fill import assess_decoupling_fill
    if not preparation_preserved(preparation, board):
        return dict(status='power_preparation_owner_changed')
    if evidence is None or not evidence.matches(board):
        return dict(status='fresh_native_evidence_required')
    evidence = replace(evidence, decoupling_assessments=assess_decoupling_fill(board, evidence))
    result = assess_power_plan_fill(board, preparation.plan, evidence, options,
                                   baseline_evidence=preparation.baseline_evidence)
    required = required_return_contacts(board, preparation.plan)
    returns = assess_native_return_contacts(board, evidence, required)
    preservation = compare_power_fill_connectivity(preparation.board, preparation.evidence, board, evidence)
    return dict(status='native_connected' if result['status'] == 'native_connected'
                and preservation['status'] == 'preserved'
                and returns['status'] in {'native_connected', 'not_requested'} else 'power_preparation_regression',
                domains=result, preservation=preservation, return_contacts=returns)


def complete_preparation_bypasses(board, ownership, options, fanout_options, *, on_progress=None):
    """Use the remaining private joint-trial allowance after returns exist.

    Early reservations replace late joint negotiation, so this phase shares its
    configured trial bound instead of granting retries per failed association.
    Only source-declared local links are attempted; no old copper is released.
    """
    from .package_access import _verified_bypass_links
    from .decoupling import route_decouplers
    from .decoupling_paths import refresh_crosslayer_witnesses
    from .copper_reservations import reserve_copper_scene
    from .routing_clearance import _geometry_identity
    if ownership.frozen or not ownership.matches(board):
        raise ValueError('fresh disposable contact ownership required')
    selected = set(options.nets or (d.net for d in board.power_domains))-set(options.plane_return_nets)
    if any(v.net in selected or v.net in {l.return_net for l in board.decoupling_links if l.net in selected}
           for v in (*options.reserved_tracks, *options.reserved_vias)):
        raise ValueError('temporary reservation overlaps bypass producer net')
    verified = _verified_bypass_links(board, selected)
    pending = tuple(l for l in board.decoupling_links if l.net in selected and l not in verified)
    trials = []
    for link in pending[:options.joint_contact_trials]:
        identity = 'bypass:' + _geometry_identity(link)
        emit(on_progress, 'late_local_bypass', 'started', target=str(link.target), capacitor=str(link.capacitor))
        scene = reserve_copper_scene(board, options.reserved_tracks, options.reserved_vias)
        routed = route_decouplers(scene.board, only_nets=frozenset({link.net}),
            only_links=frozenset({link}), fanout_options=fanout_options)
        candidate = scene.extract(routed.board)
        witnesses, reason = refresh_crosslayer_witnesses(candidate, routed.admitted_path_witnesses)
        paths, vias = {}, {}
        proved = _verified_bypass_links(candidate, {link.net}, witness_paths=paths,
            witness_vias=vias, admitted_witnesses=witnesses) if not reason else ()
        status = 'local_link_restored' if link in proved else reason or 'finite_local_pool_unresolved'
        if status == 'local_link_restored':
            use = CopperUse(identity, 'bypass', (link.target,link.capacitor,link.return_pad),
                            paths.get(link,()),vias.get(link,()))
            ownership = append_copper_ownership(ownership,board,candidate,'late-local-bypass',
                uses=(use,),trial_mutable=True)
            board = candidate
        trials.append(dict(requirement=identity,target=str(link.target),capacitor=str(link.capacitor),
                           status=status, pending=[reason for _,reason in routed.pending]))
        emit(on_progress, 'late_local_bypass', 'finished', status=status)
    return board, ownership, tuple(trials)


def prepare_power_distribution(source, guides, options, fanout_options, *,
                               native_verifier=None, pair_state_limit=None, on_progress=None,
                               ordinary_nets=None, on_contact_snapshot=None):
    """One bounded clean-source critical/local/contact/area/native transaction.

    ``native_verifier(board, stage)`` must return fresh native adapter evidence.
    No placement trial, source filtering, retrospective rip-up or budget growth.
    Rejection returns the exact original source, with the actual diagnostic plan.
    """
    from .critical import route_critical_nets
    emit(on_progress, 'power_return_preparation', 'started', nets=options.plane_return_nets)
    critical = route_critical_nets(source, guides, pair_state_limit=pair_state_limit,
                                  bundle_repair_limit=0, on_progress=critical_progress(on_progress))
    if native_verifier is None:
        from .critical import CriticalRoutingStatus
        from .hard_macros import materialize_hard_macros
        original = materialize_hard_macros(source)
        status = 'critical_routes_incomplete' if critical.status is CriticalRoutingStatus.FAILED else 'native_tool_required'
        emit(on_progress, 'power_return_preparation', 'finished', status=status)
        return PowerPreparation(original, original, critical, status=status)
    result = prepare_power_contacts(source, critical, options, fanout_options,
                                    ordinary_nets=ordinary_nets, on_progress=on_progress)
    if result.status != 'contacts_prepared':
        return result
    snapshot = result.contact_snapshot
    if on_contact_snapshot is not None:
        on_contact_snapshot(snapshot.board, snapshot.ownership)
    return prepare_power_from_contacts(snapshot, options, fanout_options, native_verifier=native_verifier,
                                       ordinary_nets=ordinary_nets, on_progress=on_progress)


def prepare_power_contacts(source, critical, options, fanout_options, *, ordinary_nets=None, on_progress=None):
    """Private bounded producer stage, reusable with an exact critical snapshot.

    No accepted power scene is modified. Early ordinary copper only obstructs
    disposable contact searches; fresh native acceptance remains a later gate.
    """
    from .critical import CriticalRoutingStatus
    from .hard_macros import materialize_hard_macros
    from .decoupling import route_decouplers
    from .decoupling_paths import _return_targets
    from .decoupling_paths import refresh_crosslayer_witnesses
    from .copper_reservations import reserve_copper_scene
    from .plane import stitch_zone_pads, PlaneStitchOptions
    original = materialize_hard_macros(source)
    result = PowerPreparation(original, original, critical)
    def reject(status, **kwargs):
        emit(on_progress, 'power_return_preparation', 'finished', status=status)
        return replace(result, status=status, **kwargs)
    if critical.status is CriticalRoutingStatus.FAILED:
        return reject('critical_routes_incomplete')
    selected = options.nets or tuple(d.net for d in source.power_domains)
    if not set(options.plane_return_nets) <= set(selected):
        return reject('unselected_plane_return_domain')
    links = tuple(l for l in original.decoupling_links if l.net in selected)
    if any(not any(l.net == net for l in links) for net in options.plane_return_nets):
        return reject('missing_plane_return_association')
    if any(not _return_targets(original, l) for l in links):
        return reject('unsupported_return_terminal')
    board = critical.board
    ownership = begin_copper_ownership(board)
    if not ContactPreparationSnapshot(original, critical, board, ownership, tuple(selected), frozenset()).matches():
        return reject('stale_critical_snapshot')
    package_reports=();package_tracks=();package_vias=()
    if options.package_patterns_before_contacts:
        from .package_preparation import prepare_package_patterns
        prior_tracks=len(options.reserved_tracks);prior_vias=len(options.reserved_vias)
        board,ownership,options,reports=prepare_package_patterns(board,ownership,selected,links,options,
            on_progress=on_progress)
        package_reports=reports
        package_tracks=options.reserved_tracks[prior_tracks:];package_vias=options.reserved_vias[prior_vias:]
        result=replace(result,package_patterns=reports)
        if any(not isinstance(r,dict) or not r['candidates'] for r in reports):
            return reject('package_pattern_preparation_unresolved')
    early = None
    if options.reserve_ordinary_before_contacts:
        from .joint_preparation import prepare_ordinary_reservations
        emit(on_progress, 'early_ordinary_reservations', 'started')
        early = prepare_ordinary_reservations(board, ownership, fanout_options, only_nets=ordinary_nets,
            reserved_tracks=options.reserved_tracks,reserved_vias=options.reserved_vias)
        from .copper_reservations import reserved_copper_additions
        tracks, vias = reserved_copper_additions(board, early.reservations)
        # The ordinary scene already contains the package/external reservations.
        # Keep its exact occurrence difference; concatenating them again would
        # invent a second drill at every previously reserved via.
        options = replace(options, reserved_tracks=tracks, reserved_vias=vias)
        emit(on_progress, 'early_ordinary_reservations', 'finished', ports=len(early.ports),
             pending=len(early.baseline.pending_pads))
    produced_nets = set(selected) | {l.return_net for l in links}
    if any(v.net in produced_nets for v in (*options.reserved_tracks, *options.reserved_vias)):
        return reject('temporary_reservation_overlaps_producer_net')
    result = replace(result, ownership=ownership)
    def bypass_uses(routed, names, admitted=()):
        from .package_access import _verified_bypass_links
        from .routing_clearance import _geometry_identity
        paths, vias = {}, {}
        verified = _verified_bypass_links(routed, names, witness_paths=paths, witness_vias=vias,
                                         admitted_witnesses=admitted)
        return tuple(CopperUse('bypass:' + _geometry_identity(link), 'bypass',
            (link.target, link.capacitor, link.return_pad), paths.get(link, ()), vias.get(link, ()))
            for link in routed.decoupling_links if link in verified)
    for net in selected:
        if net not in options.plane_return_nets:
            emit(on_progress, 'local_bypass_contacts', 'started', net=net)
            scene = reserve_copper_scene(board, options.reserved_tracks, options.reserved_vias)
            routed = route_decouplers(scene.board, only_nets=frozenset((net,)), fanout_options=fanout_options)
            candidate = scene.extract(routed.board)
            witnesses, reason = refresh_crosslayer_witnesses(candidate, routed.admitted_path_witnesses)
            if reason:
                return reject('reserved_bypass_witness_invalid')
            ownership = append_copper_ownership(ownership, board, candidate, f'local-bypasses:{net}',
                uses=bypass_uses(candidate, {net}, witnesses), trial_mutable=True)
            board = candidate
            emit(on_progress, 'local_bypass_contacts', 'finished', net=net,
                 pending=len(routed.pending), tracks=len(routed.created_tracks), vias=len(routed.created_vias))
    returned = frozenset(p for l in links for p in (l.return_pad, *_return_targets(board, l)))
    scene = reserve_copper_scene(board, options.reserved_tracks, options.reserved_vias)
    stitched = stitch_zone_pads(scene.board, PlaneStitchOptions(only_pads=returned,
        include_surface_zones=True, maze_step_nm=options.step_nm,
        maze_state_budget=options.search_states))
    candidate = scene.extract(stitched.board)
    ownership = append_copper_ownership(ownership, board, candidate, 'return-contacts',
        uses=stitched.copper_uses, trial_mutable=True)
    board = candidate
    before_join = board
    board, return_observations = join_return_contacts(board, returned, options)
    ownership = append_copper_ownership(ownership, before_join, board, 'return-bridges',
        uses=prospective_contact_uses(board, returned), trial_mutable=True)
    result = replace(result, return_contact_observations=return_observations, ownership=ownership)
    completion_trials = ()
    if early is not None:
        from .joint_preparation import refresh_ordinary_reservations
        board, ownership, completion_trials = complete_preparation_bypasses(
            board,ownership,options,fanout_options,on_progress=on_progress)
        try:
            early = refresh_ordinary_reservations(early, board, ownership)
        except ValueError:
            return reject('early_ordinary_reservations_invalid')
    snapshot = ContactPreparationSnapshot(original, critical, board, ownership, tuple(selected),
        returned, return_observations, early_reservations=early,contact_completion_trials=completion_trials,
        package_patterns=package_reports,package_reserved_tracks=package_tracks,package_reserved_vias=package_vias)
    return replace(result, status='contacts_prepared', board=board, ownership=ownership,
        contact_snapshot=snapshot,contact_completion_trials=completion_trials)


def prepare_power_from_contacts(snapshot, options, fanout_options, *, native_verifier=None,
                                ordinary_nets=None, on_progress=None, area_candidate_seed=None):
    """Resume the normal bounded stage from a private, exact producer snapshot.

    Useful for scoped iteration without repeating unchanged critical routing.
    Accepted power boards cannot serve as this checkpoint. Native baseline,
    contact replacement and final area evidence are always generated afresh.
    """
    from .power_geometry import plan_power_areas, apply_power_area_plan, reselect_power_area_plan
    from .power_extract import assess_power_plan_fill
    from .decoupling_fill import assess_decoupling_fill
    from .decoupling_paths import _return_targets
    from .hard_macros import validate_hard_macros
    from .pad_via_arrays import require_via_in_pad_arrays
    result = PowerPreparation(snapshot.source, snapshot.source, snapshot.critical,
        return_contact_observations=snapshot.return_observations, ownership=snapshot.ownership,
        contact_snapshot=snapshot,contact_completion_trials=snapshot.contact_completion_trials,
        package_patterns=snapshot.package_patterns)
    def reject(status, **kwargs):
        emit(on_progress, 'power_return_preparation', 'finished', status=status)
        return replace(result, status=status, **kwargs)
    selected = options.nets or tuple(d.net for d in snapshot.source.power_domains)
    expected = frozenset(p for link in snapshot.source.decoupling_links if link.net in selected
                         for p in (link.return_pad, *_return_targets(snapshot.source, link)))
    if not snapshot.matches() or tuple(selected) != snapshot.selected_nets or expected != snapshot.returns:
        return reject('stale_contact_snapshot')
    try:
        validate_hard_macros(snapshot.board)
        require_via_in_pad_arrays(snapshot.board, 'private contact preparation')
    except ValueError:
        return reject('protected_contact_snapshot_changed')
    if native_verifier is None:
        return reject('native_tool_required')
    options=replace(options,reserved_tracks=(*options.reserved_tracks,*snapshot.package_reserved_tracks),
                    reserved_vias=(*options.reserved_vias,*snapshot.package_reserved_vias))
    board, ownership, returned = snapshot.board, snapshot.ownership, snapshot.returns
    baseline = native_verifier(board, 'power-return-baseline')
    joint = snapshot.early_reservations
    if joint is not None:
        result = replace(result, joint_access=joint)
    remaining_trials = max(0, options.joint_contact_trials-len(snapshot.contact_completion_trials))
    pending_bypasses = frozenset()
    if joint is not None and remaining_trials:
        from .package_access import _verified_bypass_links
        explicit_nets = set(selected)-set(options.plane_return_nets)
        verified = _verified_bypass_links(board, explicit_nets)
        pending_bypasses = frozenset(l for l in board.decoupling_links
                                    if l.net in explicit_nets and l not in verified)
    if remaining_trials and (joint is None or pending_bypasses):
        from .joint_preparation import negotiate_preparation_contacts, JointPreparationOptions
        from .power_extract import compare_power_fill_connectivity
        emit(on_progress, 'joint_contact_access', 'started', maximum_trials=remaining_trials)
        extra = dict(fixed_reservations=joint, bypass_links=pending_bypasses) if joint is not None else {}
        proposal = negotiate_preparation_contacts(board, ownership, fanout_options,
            options=JointPreparationOptions(maximum_trials=remaining_trials),
            only_nets=frozenset() if joint is not None else ordinary_nets, **extra)
        if joint is None or proposal.status == 'geometry_proposal':
            joint = proposal
        result = replace(result, joint_access=joint,
                         joint_access_validation=dict(contact_negotiation=proposal.report()))
        if proposal.status == 'geometry_proposal':
            evidence = native_verifier(proposal.board, 'power-return-joint-contacts')
            preservation = compare_power_fill_connectivity(board, baseline, proposal.board, evidence)
            result = replace(result, joint_access_validation=preservation)
            if preservation['status'] != 'preserved':
                return reject('joint_contact_native_rejected')
            board, ownership, baseline = proposal.board, proposal.ownership, evidence
            result = replace(result, ownership=ownership)
        emit(on_progress, 'joint_contact_access', 'finished', status=proposal.status)
    planning_options = options
    if joint is not None:
        from .joint_preparation import joint_routing_corridors
        try:
            corridors = joint_routing_corridors(joint)
        except ValueError:
            return reject('joint_reservations_invalid')
        from .copper_reservations import reserved_copper_additions
        reserved_tracks, reserved_vias = reserved_copper_additions(joint.board,joint.reservations)
        planning_options = replace(options, protected_corridors=(*options.protected_corridors, *corridors),
            reserved_tracks=(*options.reserved_tracks, *reserved_tracks), reserved_vias=(*options.reserved_vias, *reserved_vias))
    if area_candidate_seed is None:
        plan = plan_power_areas(board, planning_options, on_progress=on_progress)
    else:
        from .power_geometry import refresh_power_area_reservations
        plan = refresh_power_area_reservations(board, area_candidate_seed, planning_options, on_progress=on_progress)
    active_options = planning_options
    spent_states = spent_regrowth = 0
    feedback_trials = []
    # Changed contact scenes never reuse a stale area pool or reset the area
    # allocator/regrowth allowance. Every replacement remains disposable until
    # the original native pad groups, all returns and ordinary exits survive.
    for feedback_attempt in range(options.area_contact_trials):
        if plan.selected or not plan.search_failures:
            break
        spent_states += plan.allocation_states
        spent_regrowth += len(plan.conflict_trials)
        if spent_states >= options.allocation_states:
            break
        from .power_contact_feedback import negotiate_power_area_contacts, PowerContactFeedbackOptions
        from .joint_preparation import JointPreparationOptions, joint_reservations_preserved
        from .power_extract import compare_power_fill_connectivity
        feedback = negotiate_power_area_contacts(board, ownership, plan, fanout_options, planning_options,
            options=PowerContactFeedbackOptions(joint=JointPreparationOptions(maximum_trials=1)),
            fixed_reservations=joint, on_progress=on_progress)
        record = dict(attempt=feedback_attempt+1, proposal=feedback.report(),
            allocation_states_spent=spent_states, regrowth_trials_spent=spent_regrowth)
        feedback_trials.append(record)
        result = replace(result, area_contact_trials=tuple(feedback_trials))
        if feedback.status != 'geometry_proposal':
            break
        stage = f'power-return-area-contacts-{feedback_attempt+1}'
        evidence = native_verifier(feedback.board, stage)
        preservation = compare_power_fill_connectivity(board, baseline, feedback.board, evidence)
        returns = assess_native_return_contacts(feedback.board, evidence, returned)
        ordinary_preserved = joint_reservations_preserved(feedback, feedback.board)
        record.update(stage=stage, preservation=preservation, returns=returns,
                      ordinary_preserved=ordinary_preserved)
        if (preservation['status'] != 'preserved'
                or returns['status'] not in {'native_connected','not_requested'} or not ordinary_preserved):
            record['status'] = 'native_contact_preservation_rejected'
            break
        board, ownership, baseline, joint = feedback.board, feedback.ownership, evidence, feedback
        active_options = replace(planning_options,
            allocation_states=options.allocation_states-spent_states,
            maximum_conflict_trials=max(0,options.maximum_conflict_trials-spent_regrowth))
        plan = plan_power_areas(board, active_options, on_progress=on_progress)
        record.update(status=plan.status, replan_snapshot=plan.snapshot,
            allocation_states_total=spent_states+plan.allocation_states,
            regrowth_trials_total=spent_regrowth+len(plan.conflict_trials))
    trials, rejected = [], []
    for attempt in range(options.maximum_native_trials):
        # Every trial starts from the same complete held baseline. Rejected
        # copper and native evidence never enter the next candidate.
        candidate, status = apply_power_area_plan(board, plan)
        if status not in {'contact_routed_native_pending', 'already_contact_routed'}:
            return reject(status, plan=plan, baseline_evidence=baseline, native_trials=tuple(trials))
        stage = 'power-return-candidate' + (f'-{attempt+1}' if attempt else '')
        evidence = native_verifier(candidate, stage)
        evidence = replace(evidence, decoupling_assessments=assess_decoupling_fill(candidate, evidence))
        observation = assess_power_plan_fill(candidate, plan, evidence, options, baseline_evidence=baseline)
        returns = assess_native_return_contacts(candidate, evidence, returned)
        observation['return_contacts'] = returns
        if returns['status'] not in {'native_connected', 'not_requested'}:
            observation['status'] = returns['status']
        if joint is not None:
            from .joint_preparation import joint_reservations_preserved
            if not joint_reservations_preserved(joint, candidate):
                observation['status'] = 'joint_ordinary_reservations_lost'
        trials.append(dict(stage=stage, selected_ids=plan.selected_ids,
            filled_board_digest=evidence.filled_board_digest, report_digest=evidence.report_digest,
            observation=observation))
        if observation['status'] == 'native_connected':
            # Areas are held output, not an invitation to mutate accepted power
            # copper. Native acceptance freezes the complete occurrence ledger.
            final_ownership = append_copper_ownership(ownership, board, candidate, 'distribution-areas',
                permitted_fields=frozenset({'metadata', 'zones', 'zone_fills'}))
            accepted = replace(result, board=candidate, plan=plan, baseline_evidence=baseline,
                evidence=evidence, status='native_connected', observation=observation, native_trials=tuple(trials),
                ownership=replace(final_ownership, frozen=True),
                joint_access=replace(joint, ownership=replace(joint.ownership, frozen=True)) if joint is not None else None)
            emit(on_progress, 'power_return_preparation', 'finished', status=accepted.status)
            return accepted
        rejected.append(plan.selected_ids)
        if attempt+1 < options.maximum_native_trials:
            from .power_transition_repair import repair_native_power_transitions
            repaired = repair_native_power_transitions(plan, active_options, evidence)
            changed = repaired.selected_ids != plan.selected_ids
            plan = repaired
            if changed and plan.selected:
                continue  # A new same-source proposal requires fresh native fill.
            alternative = reselect_power_area_plan(plan, active_options, rejected)
            if alternative.selected:
                plan = alternative
                continue
            # Retain the last native-tested plan and cumulative search usage.
            plan = replace(plan, allocation_states=alternative.allocation_states,
                           exhausted=alternative.exhausted)
        break
    return reject('native_power_return_rejected', plan=plan, baseline_evidence=baseline,
                  observation=observation, native_trials=tuple(trials))
