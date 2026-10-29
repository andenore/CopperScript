"""Migration messages for route-board options retired after the Rust port."""

RETIRED_OPTIONS = {
    "--unified-direct": "use --unified-set direct=true",
    "--unified-no-decouplers": "use --unified-set pre_route_decouplers=false",
    "--unified-retain-escape-copper": "use --unified-set retain_escape_copper=true",
    "--unified-iterations": "use --unified-set maximum_iterations=N",
    "--unified-search-states": "use --unified-set maximum_expansions=N",
    "--unified-pair-states": "use --unified-set critical_pair_state_limit=N",
    "--fine-regions": "use --unified-set fine_regions=off|arrays|dense",
    "--power-islands": "use --unified-set power_islands=auto|LAYER (off is power_islands=null)",
    "--no-route-smoothing": "use --unified-set route_smoothing=false",
    "--stitch-zone-pads": "declared copper zones are stitched automatically",
    "--detailed-feedback-trials": "use --congestion-feedback-trials N; old effective budget was max(N, detailed + critical)",
    "--critical-feedback-trials": "use --congestion-feedback-trials N; old effective budget was max(N, detailed + critical)",
    "--pitch-mm": "use --unified-pitch-mm for routing, or --local-demand-pitch-mm with --local-demand-cost; omit an unused pitch",
}


def retired_option_error(arguments):
    """Explain a removed exact flag before loading source or starting work."""
    for argument in arguments:
        if argument == "--":
            break
        name = argument.partition("=")[0]
        if name in RETIRED_OPTIONS:
            return f"{name} was removed; {RETIRED_OPTIONS[name]}"
    return None
