"""Conservative, bounded finite-volume DC screening of actual copper.

Only complete square cells inside supported copper participate. Narrow features
lost at this pitch remain unresolved, never healed. Pads are point boundary/load
ports at mesh resolution; barrel resistance and material data must be explicit.
This screening mesh is not an electromagnetic, thermal or signoff solver.
"""
from dataclasses import dataclass, replace
from fractions import Fraction
from math import ceil, cos, floor, pi

from .physical import CopperLayer, PadReference
from .drc import _explicit_copper_contacts, physical_board_digest, non_plated_holes, placed_pad_shape
from .power_network import DCNetwork, ResistorEdge, _exact, solve_dc


@dataclass(frozen=True)
class CopperMaterial:
    layer: CopperLayer
    thickness_nm: int
    resistivity_ohm_m: Fraction
    temperature_c: Fraction
    provenance: str

    def __post_init__(self):
        rho = _exact(self.resistivity_ohm_m, 'resistivity')
        temperature = _exact(self.temperature_c, 'temperature')
        if type(self.thickness_nm) is not int or self.thickness_nm <= 0 or rho <= 0 or not self.provenance:
            raise ValueError('copper material needs positive thickness/resistivity and provenance')
        object.__setattr__(self, 'resistivity_ohm_m', rho)
        object.__setattr__(self, 'temperature_c', temperature)


@dataclass(frozen=True)
class PowerPort:
    name: str
    net: str
    terminal: PadReference
    layer: CopperLayer
    withdrawal_a: Fraction
    voltage_v: Fraction | None = None
    minimum_voltage_v: Fraction | None = None
    local_return: str | None = None

    def __post_init__(self):
        if not self.name or not self.net:
            raise ValueError('power port requires a name and net')
        for name in ('withdrawal_a', 'voltage_v', 'minimum_voltage_v'):
            value = getattr(self, name)
            if value is not None:
                object.__setattr__(self, name, _exact(value, name))


@dataclass(frozen=True)
class BarrelResistance:
    contact: str  # Exact explicit-copper occurrence identity, e.g. via:3.
    first: CopperLayer
    second: CopperLayer
    resistance_ohms: Fraction
    provenance: str

    def __post_init__(self):
        value = _exact(self.resistance_ohms, 'barrel resistance')
        if not self.contact or not self.provenance or value <= 0 or self.first == self.second:
            raise ValueError('barrel resistance needs a distinct span and explicit source')
        object.__setattr__(self, 'resistance_ohms', value)


@dataclass(frozen=True)
class PowerElectricalModel:
    net: str
    ports: tuple[PowerPort, ...]
    materials: tuple[CopperMaterial, ...]
    barrels: tuple[BarrelResistance, ...] = ()
    pitch_nm: int = 125000
    maximum_cells: int = 16384
    maximum_iterations: int = 2048
    # External model interfaces are declared, not synthesized from part names.
    regulator_model: str | None = None
    package_model: str | None = None
    capacitor_models: tuple[tuple[str, str], ...] = ()
    refinement_tolerance_v: Fraction | None = None

    def __post_init__(self):
        for name in ('ports', 'materials', 'barrels', 'capacitor_models'):
            object.__setattr__(self, name, tuple(getattr(self, name)))
        if not self.net or any(type(v) is not int or v <= 0 for v in (self.pitch_nm, self.maximum_cells, self.maximum_iterations)):
            raise ValueError('electrical model requires a net and finite positive bounds')
        if len({p.name for p in self.ports}) != len(self.ports) or len({m.layer for m in self.materials}) != len(self.materials):
            raise ValueError('duplicate electrical port or copper material')
        keys = [(b.contact, frozenset((b.first, b.second))) for b in self.barrels]
        if len(set(keys)) != len(keys):
            raise ValueError('duplicate barrel resistance')
        if self.refinement_tolerance_v is not None:
            tolerance = _exact(self.refinement_tolerance_v, 'refinement voltage tolerance')
            if tolerance <= 0:
                raise ValueError('refinement tolerance must be positive')
            object.__setattr__(self, 'refinement_tolerance_v', tolerance)


@dataclass(frozen=True)
class ExtractedPowerScreen:
    snapshot: str
    status: str
    network: DCNetwork | None = None
    limits: tuple = ()
    references: tuple = ()
    unresolved: tuple[str, ...] = ()
    cells: int = 0
    pitch_nm: int = 0
    solution: object = None
    port_voltages_v: tuple = ()

    def report(self):
        return dict(snapshot=self.snapshot, status=self.status, unresolved=self.unresolved,
            cells=self.cells, pitch_nm=self.pitch_nm,
            maximum_kcl_residual_a=self.solution.maximum_kcl_residual_a if self.solution else None,
            voltages_v=dict(self.port_voltages_v),
            scope='DC mesh screening; no AC, thermal, current-density or hardware qualification')


def screen_power_model(board, model, evidence=None, *, external_boundaries=()):
    from .power_geometry import _shape_geometry, _conflict_geometry
    snapshot = physical_board_digest(board)
    def failed(reason, cells=0):
        return ExtractedPowerScreen(snapshot, 'unresolved', unresolved=(reason,), cells=cells, pitch_nm=model.pitch_nm)
    if evidence is not None and (not evidence.matches(board) or not evidence.filled_board_digest or not evidence.report_digest):
        return failed('stale_or_unbound_native_evidence')
    if not model.ports or not model.materials:
        return failed('electrical_inputs_missing')
    nets = {p.net for p in model.ports}
    members = {n.name: set(n.pads) for n in board.nets}
    if model.net not in members or not members[model.net] <= {p.terminal for p in model.ports if p.net == model.net}:
        return failed('terminal_current_obligations_missing')
    if any(p.terminal not in members.get(p.net, ()) for p in model.ports):
        return failed('foreign_model_terminal')
    boundaries = {p for d in board.power_domains if d.net == model.net for p in d.sources}
    boundaries.update(f.terminal for f in board.power_feeds if f.net == model.net)
    boundaries.update(b.terminal for b in external_boundaries if b.net == model.net)
    if any(p.net == model.net and p.voltage_v is not None and p.terminal not in boundaries for p in model.ports):
        return failed('voltage_boundary_without_source_provenance')
    if any(p.withdrawal_a > 0 and (p.minimum_voltage_v is None or p.local_return is None) for p in model.ports):
        return failed('load_limits_or_local_returns_missing')
    names = {p.name for p in model.ports}
    if any(p.local_return and p.local_return not in names for p in model.ports):
        return failed('unknown_local_return')
    try:
        from shapely.geometry import box, Polygon
        from shapely.ops import unary_union
        from .rf_geometry import native_fill_polygon
    except ImportError:
        return failed('geometry_dependency_unavailable')
    contacts, pad_nodes, _ = _explicit_copper_contacts(board, only_nets=frozenset(nets), include_internal_connections=False)
    # Drilled voids cut every overlapping conductor, including same-net tracks.
    # Subtracting each via annulus before union would accidentally heal its hole
    # with the track crossing its centre. Include foreign/unconnected drills too.
    all_contacts, _, _ = _explicit_copper_contacts(board, include_internal_connections=False)
    voids = {layer: [_conflict_geometry(shape) for _, shape in non_plated_holes(board)]
             for layer in board.stackup.copper_layers}
    for layer in voids:
        voids[layer].extend(Polygon([(p.x_nm, p.y_nm) for p in cutout.vertices]) for cutout in board.outline.cutouts)
    for contact in all_contacts:
        if contact.drill:
            for layer in contact.layers:
                if layer not in contact.capped_layers:
                    voids[layer].append(_conflict_geometry(contact.drill))
    from .physical import PadKind, PadShape
    from .placement import transformed_local_point
    for pose in board.placements:
        for pad in board.footprints[pose.footprint].pads:
            if pad.kind is not PadKind.THROUGH_HOLE or pad.drill is None:
                continue
            drill = replace(pad, kind=PadKind.SMD, size=pad.drill, drill=None,
                shape=PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm else PadShape.OVAL)
            shape = placed_pad_shape(transformed_local_point(pose, pad.position), drill, pose)
            for layer in voids:
                voids[layer].append(_conflict_geometry(shape))
    voids = {layer: unary_union(shapes) for layer, shapes in voids.items()}
    materials = {m.layer: m for m in model.materials}
    thicknesses = {l.copper_layer: l.thickness_nm for l in board.stackup.physical_layers if l.copper_layer}
    if any(layer in thicknesses and m.thickness_nm != thicknesses[layer] for layer, m in materials.items()):
        return failed('material_stackup_thickness_mismatch')
    geometries, contact_geometry, contact_by_id = {}, {}, {c.identity: c for c in contacts}
    for contact in contacts:
        shape = _shape_geometry(contact.shape)
        for layer in contact.layers:
            copper = shape
            if contact.drill and layer not in contact.capped_layers:
                # Subtract a circumscribed void, never optimistic annular copper.
                drill = _shape_geometry(contact.drill).buffer(contact.drill.radius_nm*(1/cos(pi/32)-1))
                copper = copper.difference(drill)
            geometries.setdefault((contact.net, layer), []).append(copper)
            contact_geometry[contact.identity, layer] = copper
    if evidence is not None:
        for fragment in evidence.filled_regions:
            if fragment.net not in nets:
                continue
            try:
                geometry = native_fill_polygon({'outer': fragment.vertices_nm, 'holes': []})
                layer = CopperLayer(fragment.layer)
            except ValueError:
                return failed('unsupported_native_fragment')
            if geometry.geom_type != 'Polygon' or not geometry.is_valid:
                return failed('unsupported_native_fragment')
            geometries.setdefault((fragment.net, layer), []).append(geometry)
    if any(layer not in materials for _, layer in geometries):
        return failed('copper_material_missing')
    # Preserve board-relative origin, so rigid translation preserves mesh tests.
    ox = min(p.x_nm for p in board.outline.vertices)
    oy = min(p.y_nm for p in board.outline.vertices)
    pitch = model.pitch_nm
    cells, edges, used = {}, [], 0
    for (net, layer), pieces in sorted(geometries.items(), key=lambda i: (i[0][0], i[0][1].value)):
        copper = unary_union(pieces).difference(voids[layer])
        if copper.is_empty:
            continue
        x0, y0, x1, y1 = copper.bounds
        xs, xe = floor((x0-ox)/pitch), ceil((x1-ox)/pitch)
        ys, ye = floor((y0-oy)/pitch), ceil((y1-oy)/pitch)
        # Bound examined cells as well as retained/solved nodes.
        used += (xe-xs)*(ye-ys)
        if used > model.maximum_cells:
            return failed('mesh_cell_budget_exhausted', len(cells))
        for x in range(xs, xe):
            for y in range(ys, ye):
                square = box(ox+x*pitch, oy+y*pitch, ox+(x+1)*pitch, oy+(y+1)*pitch)
                if copper.covers(square):
                    cells[net, layer, x, y] = (f'{net}:{layer.value}:{x}:{y}', square)
    if not cells:
        return failed('no_complete_mesh_cells')
    for (net, layer, x, y), (node, _) in cells.items():
        m = materials[layer]
        resistance = m.resistivity_ohm_m * 10**9 / m.thickness_nm
        for dx, dy in ((1, 0), (0, 1)):
            neighbor = cells.get((net, layer, x+dx, y+dy))
            if neighbor:
                edges.append(ResistorEdge(f'sheet:{len(edges)}', node, neighbor[0], resistance))
    def contact_cells(identity, layer):
        contact = contact_by_id[identity]
        geometry = contact_geometry.get((identity, layer))
        if geometry is None:
            return []
        return sorted(((shape.centroid.distance(geometry.centroid), node) for (net, l, _, _), (node, shape) in cells.items()
                       if net == contact.net and l == layer and shape.intersection(geometry).area > 0))
    expected_barrels = set()
    for c in contacts:
        if len(c.layers) <= 1:
            continue
        for a, b in zip(c.layers, c.layers[1:]):
            expected_barrels.add((c.identity, frozenset((a, b))))
    provided = {(b.contact, frozenset((b.first, b.second))) for b in model.barrels}
    if expected_barrels != provided:
        return failed('barrel_models_missing_or_foreign', len(cells))
    for barrel in model.barrels:
        first, second = contact_cells(barrel.contact, barrel.first), contact_cells(barrel.contact, barrel.second)
        if not first or not second:
            return failed('barrel_contact_unresolved_at_mesh_pitch', len(cells))
        edges.append(ResistorEdge(f'barrel:{len(edges)}', first[0][1], second[0][1], barrel.resistance_ohms))
    aliases, fixed, loads, cell_terminals = {}, {}, {}, {}
    for port in model.ports:
        nodes = pad_nodes.get(port.terminal, ())
        if len(nodes) != 1:
            return failed('ambiguous_model_terminal', len(cells))
        choices = contact_cells(nodes[0], port.layer)
        if not choices:
            return failed('terminal_unresolved_at_mesh_pitch', len(cells))
        node = choices[0][1]
        if node in cell_terminals and cell_terminals[node] != port.terminal:
            return failed('merged_terminals_require_refinement', len(cells))
        cell_terminals[node] = port.terminal
        aliases[port.name] = node
        loads[node] = loads.get(node, Fraction(0)) + port.withdrawal_a
        if port.voltage_v is not None:
            if node in fixed and fixed[node] != port.voltage_v:
                return failed('conflicting_voltage_boundaries', len(cells))
            fixed[node] = port.voltage_v
    # Include every isolated mesh node; detached islands must not disappear.
    for node, _ in cells.values():
        loads.setdefault(node, Fraction(0))
    # Multiple port constraints on one cell combine conservatively only when
    # their references agree. Coarse coalescence otherwise requires refinement.
    limits, references = {}, {}
    for port in model.ports:
        if port.minimum_voltage_v is not None:
            node, returned = aliases[port.name], aliases.get(port.local_return)
            if node in references and references[node] != returned:
                return failed('merged_port_references_require_refinement', len(cells))
            limits[node] = max(limits.get(node, port.minimum_voltage_v), port.minimum_voltage_v)
            if returned:
                references[node] = returned
    network = DCNetwork(tuple(edges), fixed, loads)
    solution = solve_dc(network, maximum_nodes=model.maximum_cells, maximum_iterations=model.maximum_iterations)
    unresolved = () if solution.status == 'solved' else (solution.status,)
    status = 'dc_mesh_screen_pass' if solution.status == 'solved' else 'unresolved'
    if solution.status == 'solved' and any(solution.voltages_v[n] - solution.voltages_v.get(references.get(n), 0) < float(v)
                                          for n, v in limits.items()):
        status, unresolved = 'voltage_limit_failed', ('voltage_limit_failed',)
    return ExtractedPowerScreen(snapshot, status, network, tuple(limits.items()), tuple(references.items()),
                                unresolved, len(cells), pitch, solution,
                                tuple((name, solution.voltages_v[node]) for name, node in aliases.items())
                                if solution.status == 'solved' else ())


def refine_power_model(board, model, evidence, *, external_boundaries=()):
    """Two fixed meshes, one fresh native fill; no unbounded convergence loop."""
    coarse = screen_power_model(board, model, evidence, external_boundaries=external_boundaries)
    if model.pitch_nm < 2:
        return dict(status='refinement_pitch_exhausted', coarse=coarse.report())
    fine = screen_power_model(board, replace(model, pitch_nm=model.pitch_nm//2), evidence,
                              external_boundaries=external_boundaries)
    if not coarse.port_voltages_v or not fine.port_voltages_v:
        return dict(status='refinement_unresolved', coarse=coarse.report(), fine=fine.report())
    cv, fv = dict(coarse.port_voltages_v), dict(fine.port_voltages_v)
    change = max((abs((cv[p.name]-cv.get(p.local_return, 0))-(fv[p.name]-fv.get(p.local_return, 0)))
                  for p in model.ports), default=0)
    status = ('refinement_tolerance_missing' if model.refinement_tolerance_v is None else
              'refinement_screen_pass' if coarse.status == fine.status == 'dc_mesh_screen_pass'
                  and change <= float(model.refinement_tolerance_v) else 'refinement_unresolved')
    return dict(status=status, maximum_port_voltage_change_v=change,
                coarse=coarse.report(), fine=fine.report())


def assess_power_plan_fill(board, plan, evidence, options, *, baseline_evidence=None):
    """Fresh source-to-fragment and local association observations for a plan."""
    from .filled_connectivity import filled_component_roots, native_item_identity
    from .plane_verify import native_object_map
    if evidence is None:
        return dict(status='native_tool_not_requested', domains=[], electrical=[])
    if not evidence.matches(board) or not evidence.filled_board_digest or not evidence.report_digest:
        return dict(status='stale_or_unbound_native_evidence', domains=[], electrical=[])
    partition_check = None
    if plan.partitioned_zone_ids or physical_board_digest(board) != plan.snapshot:
        partition_check = compare_power_fill_connectivity(plan.source_board, baseline_evidence, board, evidence)
    try:
        import shapely  # noqa: F401
    except ImportError:
        return dict(status='geometry_dependency_unavailable', domains=[], electrical=[])
    nets = frozenset(o.net for o in plan.obligations)
    members = {n.name: set(n.pads) for n in board.nets}
    if any(set(o.terminals) != members.get(o.net) for o in plan.obligations):
        return dict(status='stale_terminal_obligations', domains=[], electrical=[])
    roots = filled_component_roots(board, evidence, nets)
    items = {}
    for item in native_object_map(board).values():
        if item.kind == 'pad':
            items.setdefault(PadReference(item.component, item.pad), []).append(item)
    def root(pad):
        matches = items.get(pad, ())
        return roots.get(native_item_identity(matches[0])) if len(matches) == 1 else None
    domains = []
    unknown = any(i.kind == 'unknown' for f in evidence.native_findings for i in f.items)
    for obligation in plan.obligations:
        boundaries = (*obligation.sources, *(f.terminal for f in obligation.feeds))
        source_roots = {root(p) for p in boundaries} - {None}
        pending = [str(p) for p in obligation.terminals if root(p) is None or root(p) not in source_roots]
        local = [a for a in evidence.decoupling_assessments if a.link in obligation.local_links]
        local_ok = len(local) == len(obligation.local_links) and all(a.status == 'connected' for a in local)
        selected_opens = any((f.category == 'unconnected' or f.type == 'isolated_copper') and any(i.net == obligation.net for i in f.items)
                             for f in evidence.native_findings)
        connected = len(source_roots) == 1 and not pending and local_ok and not unknown and not selected_opens
        domains.append(dict(net=obligation.net, status='native_connected' if connected else 'unresolved',
            pending_terminals=pending, local_associations_connected=local_ok,
            upstream_obligations=[f.upstream_net for f in obligation.feeds]))
    electrical = [refine_power_model(board, model, evidence,
        external_boundaries=options.external_boundaries) for model in options.electrical_models]
    return dict(status='native_connected' if all(d['status'] == 'native_connected' for d in domains)
                and not evidence.other_violation_count and (partition_check is None or partition_check['status'] == 'preserved')
                else 'unresolved', domains=domains, electrical=electrical, partition_check=partition_check,
                native_other_violations=evidence.other_violation_count,
                electrical_inputs_missing=[n for n in sorted(nets) if n not in {m.net for m in options.electrical_models}])


def compare_power_fill_connectivity(before, baseline, after, evidence):
    """Every original pad contact group must survive an authorized partition."""
    from .filled_connectivity import filled_component_roots
    if (before is None or baseline is None or not baseline.matches(before)
            or not baseline.filled_board_digest or not baseline.report_digest
            or not evidence.matches(after) or not evidence.filled_board_digest or not evidence.report_digest):
        return dict(status='fresh_baseline_required', lost_groups=[])
    if before.nets != after.nets or before.placements != after.placements:
        return dict(status='source_inventory_changed', lost_groups=[])
    if any(i.kind == 'unknown' for result in (baseline, evidence) for f in result.native_findings for i in f.items):
        return dict(status='unmapped_native_object', lost_groups=[])
    try:
        import shapely  # noqa: F401
    except ImportError:
        return dict(status='geometry_dependency_unavailable', lost_groups=[])
    nets = frozenset(n.name for n in before.nets)
    old = filled_component_roots(before, baseline, nets)
    new = filled_component_roots(after, evidence, nets)
    groups = {}
    for identity, root in old.items():
        if identity.startswith('pad:'):
            groups.setdefault(root, []).append(identity)
    lost = [sorted(group) for group in groups.values()
            if any(p not in new for p in group) or len({new.get(p) for p in group}) != 1]
    status = ('preserved' if not lost and evidence.island_count <= baseline.island_count
              and not evidence.other_violation_count else 'connectivity_or_native_regression')
    return dict(status=status, lost_groups=lost, baseline_islands=baseline.island_count,
                candidate_islands=evidence.island_count)
