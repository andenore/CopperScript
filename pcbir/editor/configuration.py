"""Choose editor layers from authoritative source intent and explicit settings."""
import re

from ..elaborate import elaborate
from ..physicalize import PrototypePhysicalOptions


def physical_options(design, *, layers=None, fab_profile='generic'):
    """An explicit count remains strict; auto selects the smallest supported stack.

    Layer inference never chooses a fabrication vendor or fabricates dielectric
    properties. A declared stackup remains exact, including during source edits.
    """
    flat = elaborate(design.electrical)
    required = {'F.Cu', 'B.Cu'}
    for constraint in flat.constraints:
        if constraint.kind.value not in {'routing', 'copper_zone'}:
            continue
        for key in ('layers', 'allowed_layers', 'shared_reference_layer'):
            value = constraint.parameters.get(key)
            if isinstance(value, str):
                required.update(x.strip() for x in value.split(',') if x.strip())
    mechanical = design.mechanical
    if mechanical:
        for keepout in mechanical.copper_keepouts:
            required.update(layer.value for layer in keepout.layers)
    # The supported physicalizer stops at In4.Cu. The existing binder will
    # diagnose unsupported macro layers; do not approximate a larger stack.
    minimum = max((int(match.group(1)) + 2 for layer in required
                   if (match := re.fullmatch(r'In([1-9][0-9]*)\.Cu', layer))), default=2)
    if layers is None:
        if mechanical and mechanical.stackup:
            layers = len(mechanical.stackup.copper_layers)
        else:
            if minimum < 6:
                from ..macro_bindings import prepare_macro_bindings
                for macro in prepare_macro_bindings(flat, locked=True, offline=True):
                    required.update(macro.data['required_layers'])
                minimum = max((int(match.group(1)) + 2 for layer in required
                               if (match := re.fullmatch(r'In([1-9][0-9]*)\.Cu', layer))), default=2)
            minimum = max(minimum, {'jlcpcb-four-layer': 4, 'jlcpcb-six-layer': 6}.get(fab_profile, 2))
            layers = next((count for count in (2, 4, 6) if count >= minimum), None)
            if layers is None:
                raise ValueError('source requires more than six copper layers; the editor supports 2, 4 or 6')
    if minimum > layers:
        raise ValueError(f'source requires {", ".join(sorted(required))}, outside the selected {layers}-layer stack; '
                         'omit --layers for automatic selection or choose a compatible explicit stack')
    return PrototypePhysicalOptions(copper_layers=layers, fabrication_profile=fab_profile)
