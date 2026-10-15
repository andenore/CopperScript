"""One bounded source-projection hint and anchored placement repair.

Projection is a selection heuristic, never copper clearance or exit authority.
Only clean source poses enter this transaction; normal preflight owns routing.
"""
from dataclasses import dataclass, field, replace

from .drc import physical_board_digest, placed_pad_shape
from .geometry import shapes_clear
from .physical import PadKind, PadReference
from .placement import (_fixed_placements, _legal, _pack_local_components,
                        placement_solution_is_legal, transformed_local_point)
from .routing import _placement_fingerprint
from .routing_layers import routing_layers


@dataclass(frozen=True, slots=True)
class SourceProjectionEvidence:
    capacitor_pad: PadReference
    failed_pad: PadReference
    capacitor_side: str
    failed_side: str
    copper_projection_overlap: bool
    via_envelope_proximity: bool


@dataclass(frozen=True, slots=True)
class CoupledPlacementAttempt:
    source_digest: str
    options: str
    reference: str | None
    outcome: str
    claimed: bool
    borrowed_probe_index: int | None
    evidence: tuple[SourceProjectionEvidence, ...] = ()
    source_shape_pairs: int = 0
    source_shape_predicates: int = 0
    requested_position_nm: tuple[int, int] | None = None
    requested_rotation: str | None = None
    neighbors: tuple[str, ...] = ()
    changed_references: tuple[str, ...] = ()
    requested_candidates_considered: int = 0
    neighbor_probes: int = 0
    neighbor_probe_limit: int = 0
    source_shape_pair_limit: int = 0
    source_shape_predicate_limit: int = 0
    pack_work: tuple[tuple[str, object], ...] = ()


def _protected(board, original, options):
    refs = set(_fixed_placements(board, original, options))
    refs.update(m.reference for c in board.rigid_clusters for m in c.members)
    refs.update(r.pad.component for r in board.via_in_pad_rules)
    refs.update(t.reference for r in board.relative_rules for t in r.targets)
    # Every macro belongs to an explicitly protected rigid cluster.
    return refs


def _region_member(board, ref, protected):
    return (ref not in protected and any(r.reference == ref and r.region is not None
                                        for r in board.placement_rules))


def _unique_associations(links):
    grouped = {}
    for link in links:
        grouped.setdefault(link.capacitor.component, []).append(link)
    return {ref: items[0] for ref, items in grouped.items() if len(items) == 1}


def _select_source_obstruction(baseline, ranked, options):
    board = baseline.source
    original = {p.reference: p for p in board.placements}
    protected = _protected(board, original, options)
    links = _unique_associations(board.decoupling_links)
    owners = [ref for ref in ranked if ref in links][:options.exact_repair_candidates]
    pending = sorted(baseline.pending_pads)[:options.exact_repair_candidates]
    roots = []
    rules = {r.net: r for r in board.net_routing_rules}
    nets = {p: n.name for n in board.nets for p in n.pads}
    for ref in pending:
        pose = original.get(ref.component)
        if pose is None:
            continue
        lands = [p for p in board.footprints[pose.footprint].pads if p.number == ref.pad]
        net = nets.get(ref)
        rule = rules.get(net)
        if (len(lands) != 1 or lands[0].kind is not PadKind.SMD or net is None
                or rule is not None and rule.max_vias == 0
                or len(routing_layers(board, net, rule)) < 2):
            continue
        roots.append((ref, pose, placed_pad_shape(transformed_local_point(pose, lands[0].position), lands[0], pose)))
    scored, pairs, predicates = [], 0, 0
    envelope = board.rules.default_via_size_nm // 2 + board.rules.minimum_clearance_nm
    for order, ref in enumerate(owners):
        if not _region_member(board, ref, protected):
            continue
        pose, link = original[ref], links[ref]
        lands = [p for p in board.footprints[pose.footprint].pads if p.kind in {PadKind.SMD, PadKind.THROUGH_HOLE}]
        by_number = {p.number: p for p in lands}
        # Exactly two unambiguous electrical lands: this is the declared bound.
        if (len(lands) != 2 or len(by_number) != 2
                or set(by_number) != {link.capacitor.pad, link.return_pad.pad}):
            continue
        evidence, overlaps, near = [], set(), set()
        for number, pad in by_number.items():
            shape = placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose)
            for failed, target, root_shape in roots:
                if failed.component != link.target.component:
                    continue
                pairs += 1
                overlap = not shapes_clear(shape, root_shape, 0)
                proximity = not shapes_clear(shape, root_shape, envelope)
                predicates += 2
                if overlap: overlaps.add(failed)
                if proximity: near.add(failed)
                if overlap or proximity:
                    evidence.append(SourceProjectionEvidence(PadReference(ref, number), failed,
                        pose.side.value, target.side.value, overlap, proximity))
        if overlaps or near:
            scored.append(((-len(overlaps), -len(near), order), ref, tuple(evidence)))
    if not scored:
        return None, (), pairs, predicates
    _, ref, evidence = min(scored)
    return ref, evidence, pairs, predicates


@dataclass(slots=True)
class CoupledPlacementState:
    """Shared across feedback rounds; no refund, resource reset, or extra slot."""
    considered: bool = False
    claimed: bool = False
    failed_slots: int = 0
    next_probe_index: int = 1
    available_slots: int = 0
    reserved_fingerprint: str | None = None
    reserved_probe_index: int | None = None
    attempts: list[CoupledPlacementAttempt] = field(default_factory=list)

    def prepass(self, baseline, ranked, proposals, options):
        if self.considered or self.available_slots <= 0:
            return None
        self.considered = True
        board = baseline.source
        digest = physical_board_digest(board)
        selected, evidence, pairs, predicates = _select_source_obstruction(baseline, ranked, options)
        common = dict(source_digest=digest, options=repr(options), reference=selected,
                      evidence=evidence, source_shape_pairs=pairs, source_shape_predicates=predicates,
                      neighbor_probe_limit=options.exact_repair_candidates,
                      source_shape_pair_limit=2*options.exact_repair_candidates**2,
                      source_shape_predicate_limit=4*options.exact_repair_candidates**2)
        if selected is None:
            self.attempts.append(CoupledPlacementAttempt(**common, outcome='no_source_projection_hint',
                claimed=False, borrowed_probe_index=None))
            return None
        # The selected owner gets ONE bounded prepass before legal lookahead.
        # Even an unsupported/no-pack request spends the borrowed slot.
        self.claimed = True
        slot = self.next_probe_index
        original = {p.reference: p for p in board.placements}
        protected = _protected(board, original, options)
        eligible = [ref for ref in original if ref != selected and _region_member(board, ref, protected)]
        fixed_base = {ref: p for ref, p in original.items() if ref != selected and ref not in eligible}
        outcome, desired, neighbors, packed, work = 'no_suitable_blocked_request', None, (), None, {}
        requested_count, neighbor_probes = 0, 0
        neighbor_remaining = options.exact_repair_candidates
        if board.tracks or board.vias or board.zone_fills or not placement_solution_is_legal(board, original, options):
            outcome = 'invalid_source_context'
        else:
            for requested in proposals[selected]:
                requested_count += 1
                # Reject intrinsic/region/protected violations before release.
                if not _legal(requested, fixed_base, board, options):
                    continue
                if _legal(requested, {r: p for r, p in original.items() if r != selected}, board, options):
                    continue
                if neighbor_remaining <= 0:
                    outcome = 'neighbor_probe_exhausted'
                    break
                ordered = sorted(eligible, key=lambda ref: (
                    (original[ref].position.x_nm-requested.position.x_nm)**2
                    +(original[ref].position.y_nm-requested.position.y_nm)**2, ref))
                sampled = ordered[:neighbor_remaining]
                neighbor_probes += len(sampled)
                neighbor_remaining -= len(sampled)
                blocked = [ref for ref in sampled
                           if not _legal(requested, {ref: original[ref]}, board, options)]
                if not blocked or len(blocked)+1 > options.exact_repair_limit:
                    continue
                base = {r: p for r, p in original.items() if r not in blocked}
                base[selected] = requested
                if not _legal(requested, {r: p for r, p in base.items() if r != selected}, board, options):
                    continue  # Unsampled/protected blocker remains held.
                desired, neighbors = requested, tuple(blocked)
                packed = _pack_local_components(board, neighbors, base, original, options,
                    allow_general=True, _contact_anchors=tuple(original[ref] for ref in neighbors), _work=work)
                outcome = 'bounded_pack_failed' if packed is None else 'complete_source_gate_failed'
                break  # No second member set or requested pose, even on failure.
        if packed is None and desired is None and neighbor_remaining == 0:
            outcome = 'neighbor_probe_exhausted'
        if packed is not None:
            mutable = {selected, *neighbors}
            exact = (set(packed) == set(original) and packed[selected] == desired
                     and all(packed[r] == original[r] for r in original if r not in mutable)
                     and all(replace(packed[r], position=original[r].position,
                                     rotation_degrees=original[r].rotation_degrees) == original[r] for r in original))
            if exact and placement_solution_is_legal(board, packed, options):
                trial = replace(board, placements=tuple(packed[p.reference] for p in board.placements))
                if replace(trial, placements=board.placements) == board:
                    outcome = 'geometry_candidate'
                    self.reserved_fingerprint = _placement_fingerprint(trial)
                    self.reserved_probe_index = slot
                    changed = tuple(r for r in original if packed[r] != original[r])
                else:
                    packed = None
            else:
                packed = None
        if outcome != 'geometry_candidate':
            self.failed_slots += 1
            changed = ()
        self.attempts.append(CoupledPlacementAttempt(**common, outcome=outcome, claimed=True,
            borrowed_probe_index=slot, requested_position_nm=None if desired is None else (desired.position.x_nm,desired.position.y_nm),
            requested_rotation=None if desired is None else str(desired.rotation_degrees), neighbors=neighbors,
            changed_references=changed, requested_candidates_considered=requested_count,
            neighbor_probes=neighbor_probes, pack_work=tuple(sorted(work.items()))))
        return (selected, trial) if outcome == 'geometry_candidate' else None
