"""Strict, exact-input adapter for the opt-in route-board power planner."""
import json
from dataclasses import fields

from .physical import CopperKeepout, CopperLayer, PadReference, Point, PolygonRing, PolygonWithHoles
from .power_geometry import PowerAreaOptions, ExternalPowerBoundary
from .power_extract import BarrelResistance, CopperMaterial, PowerElectricalModel, PowerPort


def load_power_area_options(path=None, *, nets=()):
    try:
        return _load_options(path, nets=nets)
    except (TypeError, KeyError, AttributeError) as exc:
        raise ValueError(f'invalid power area configuration: {exc}') from exc


def _load_options(path, *, nets):
    data = json.loads(path.read_text(encoding='utf-8-sig')) if path else {}
    if not isinstance(data, dict):
        raise ValueError('power area configuration must be an object')
    if path and data.pop('schema', None) != 'copperscript-power-area/v0.1':
        raise ValueError('unsupported power area configuration schema')
    if set(data) - {f.name for f in fields(PowerAreaOptions)}:
        raise ValueError('unknown power area configuration field')
    if nets:
        data['nets'] = tuple(nets)
    data['allowed_layers'] = tuple(CopperLayer(l) for l in data.get('allowed_layers', ()))
    data['external_boundaries'] = tuple(ExternalPowerBoundary(**{**b, 'terminal': PadReference(**b['terminal'])})
                                        for b in data.get('external_boundaries', ()))
    models = []
    for source in data.get('electrical_models', ()):
        row = dict(source)
        ports = []
        for entry in row.get('ports', ()):
            p = dict(entry)
            p['terminal'] = PadReference(**p['terminal'])
            p['layer'] = CopperLayer(p['layer'])
            ports.append(PowerPort(**p))
        row['ports'] = tuple(ports)
        row['materials'] = tuple(CopperMaterial(**{**m, 'layer': CopperLayer(m['layer'])}) for m in row.get('materials', ()))
        row['barrels'] = tuple(BarrelResistance(**{**b, 'first': CopperLayer(b['first']), 'second': CopperLayer(b['second'])})
                               for b in row.get('barrels', ()))
        models.append(PowerElectricalModel(**row))
    data['electrical_models'] = tuple(models)
    corridors = []
    for entry in data.get('protected_corridors', ()):
        if set(entry) != {'id', 'layers', 'vertices_nm'}:
            raise ValueError('protected corridor requires id, layers and vertices_nm')
        if any(len(p) != 2 or any(type(v) is not int for v in p) for p in entry['vertices_nm']):
            raise ValueError('protected corridor coordinates must be integer nanometres')
        corridors.append(CopperKeepout(entry['id'], tuple(CopperLayer(l) for l in entry['layers']),
            PolygonWithHoles(PolygonRing(tuple(Point(*p) for p in entry['vertices_nm'])))))
    data['protected_corridors'] = tuple(corridors)
    try:
        return PowerAreaOptions(**data)
    except TypeError as exc:
        raise ValueError(f'invalid power area configuration: {exc}') from exc
