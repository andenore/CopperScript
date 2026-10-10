"""Supplier construction and actual-width screening, never solver signoff."""
from decimal import Decimal

from .engineering import microstrip_impedance
from .power_integrity import number
from .qualification import check, hash_file, load_json


def engineering_input_files(plan_path, plan):
    files = {key: hash_file(plan_path.parent / relative)
             for key, relative in plan.get("input_files", {}).items()}
    path = plan.get("context", {}).get("stackup", {}).get("profile_path")
    if path:
        if "stackup_profile" in files:
            raise ValueError("reserved stackup_profile input binding")
        files["stackup_profile"] = hash_file(plan_path.parent / path)
    return files


def resolve_stackup_context(plan_path, plan):
    context = dict(plan.get("context", {}))
    selected = context.get("stackup", {})
    if selected.get("profile_path"):
        # Consumers select and review, but cannot silently override supplier facts.
        if set(selected) - {"profile_path", "reviewed"}:
            raise ValueError("supplier stackup facts must be changed in the profile, not overridden")
        profile = load_json(plan_path.parent / selected["profile_path"])
        if profile.get("schema") != "copperscript-fabrication-stackup/v0.1":
            raise ValueError("unknown fabrication stackup schema")
        context["stackup"] = {**profile, **selected}
    return context


def _source_complete(source):
    return (isinstance(source, dict) and all(source.get(k) for k in ("url", "revision", "locator"))
            and str(source["url"]).startswith("https://")
            and isinstance(source.get("sha256"), str) and len(source["sha256"]) == 64
            and all(c in "0123456789abcdef" for c in source["sha256"]))


def supplier_stackup_check(context, native):
    identifier = "engineering.stackup"
    if context.get("reviewed") is not True or not _source_complete(context.get("source")):
        return check(identifier, "incomplete", "Select/review supplier construction with exact provenance")
    missing, copper, total, previous = [], [], 0, None
    try:
        physical = context["physical_layers"]
        if not isinstance(physical, list) or not physical or not context.get("selected_profile"):
            raise ValueError("missing physical construction/profile")
        for index, layer in enumerate(physical):
            if layer["kind"] not in {"copper", "dielectric"} or layer["kind"] == previous:
                raise ValueError("stackup must alternate copper/dielectric")
            previous = layer["kind"]
            total += number(layer["thickness_nm"], "layer thickness")
            if layer["kind"] == "copper":
                copper.append(layer["name"])
            else:
                number(layer["relative_permittivity"], "permittivity", minimum=1)
                loss = layer.get("loss_tangent")
                if loss is None:
                    missing.append(f"physical_layers[{index}].loss_tangent")
                elif number(loss, "loss tangent", allow_zero=True) >= 1:
                    raise ValueError("loss tangent must be below one")
        if physical[0]["kind"] != "copper" or physical[-1]["kind"] != "copper" or len(set(copper)) != len(copper):
            raise ValueError("invalid copper endpoints/order")
        if copper != context["copper_layers"]:
            raise ValueError("profile copper order differs from physical construction")
        nominal = number(context["nominal_finished_thickness_nm"], "nominal thickness")
        tolerance = number(context["finished_thickness_tolerance_fraction"], "thickness tolerance", allow_zero=True)
        if tolerance >= 1 or not _source_complete(context.get("capabilities_source")):
            raise ValueError("nominal tolerance requires valid bounds and source provenance")
        if abs(total - nominal) > nominal * tolerance:
            raise ValueError("published construction exceeds finished thickness tolerance")
        if native is None:
            missing.append("native layer/thickness comparison")
        elif copper != native["layers"] or abs(native["board_thickness_nm"] - nominal) > 1:
            raise ValueError("selected nominal thickness/order differs from native board")
        if context.get("finished_via_plating_nm") is None:
            missing.append("finished_via_plating_nm (minimum, not published average)")
        else:
            number(context["finished_via_plating_nm"], "finished via plating")
    except (KeyError, TypeError, ValueError) as exc:
        return check(identifier, "fail", f"Invalid supplier stackup: {exc}")
    return check(identifier, "incomplete" if missing else "pass",
                 "Supplier construction selected; missing qualification inputs" if missing else "Reviewed supplier construction; external qualification still required",
                 selected_profile=context["selected_profile"], construction_thickness_nm=total,
                 nominal_finished_thickness_nm=nominal, missing_inputs=missing)


def outer_microstrip_inventory(native, context, *, identifier, net, reference_layer, reference_net):
    """Inventory real widths and estimate isolated OUTER microstrip only.

    Interior tracks, missing reference coverage and local coplanar/matching copper
    are never silently modelled as uniform exterior microstrip. Always incomplete:
    these estimates cannot replace mandatory simulation evidence.
    """
    from .rf_geometry import reference_coverage
    if native is None or not context.get("physical_layers"):
        return check(identifier, "incomplete", "Native geometry/supplier construction unavailable")
    if context.get("reviewed") is not True or not _source_complete(context.get("source")):
        return check(identifier, "incomplete", "Reviewed stackup provenance unavailable")
    physical = context["physical_layers"]
    layers = context["copper_layers"]
    if layers != native["layers"] or reference_layer not in layers:
        raise ValueError("screen stackup/reference differs from native board")
    tracks = [t for t in native["tracks"] if t["net"] == net]
    if not tracks:
        return check(identifier, "incomplete", "No routed tracks for selected net")
    entries = []
    for layer, width in sorted({(t["layer"], t["width_nm"]) for t in tracks}):
        selected = [t for t in tracks if (t["layer"], t["width_nm"]) == (layer, width)]
        item = dict(layer=layer, width_nm=width, track_count=len(selected), estimated_single_ended_ohms=None)
        entries.append(item)
        if layer not in {layers[0], layers[-1]}:
            item["reason"] = "Interior conductor requires a layer-specific embedded/stripline model"
            continue
        if abs(layers.index(layer) - layers.index(reference_layer)) != 1:
            item["reason"] = "Declared reference is not adjacent"
            continue
        index = next(i for i, p in enumerate(physical) if p.get("name") == layer)
        dielectric = physical[index + (1 if layer == layers[0] else -1)]
        copper = physical[index]
        result = microstrip_impedance(width, copper["thickness_nm"], dielectric["thickness_nm"], Decimal(str(dielectric["relative_permittivity"])))
        item.update(isolated_formula_ohms=float(result.value), dielectric_height_nm=dielectric["thickness_nm"], limitations=list(result.validity))
        probe = {**native, "tracks": selected}
        item["reference_coverage"] = reference_coverage(probe, identifier=identifier, net=net, reference_net=reference_net, reference_layer=reference_layer, margin_nm=0)
        if item["reference_coverage"]["status"] == "pass":
            item["estimated_single_ended_ohms"] = float(result.value)
        else:
            item["reason"] = "Solid-reference premise unestablished; formula value is hypothetical, not routed impedance"
    return check(identifier, "incomplete", "Actual-width isolated microstrip screening only; no differential/matching/solver approval",
                 evidence_grade="screening", net=net, reference_layer=reference_layer, reference_net=reference_net, width_inventory=entries)
