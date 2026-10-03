"""Separate simulation plans, immutable circuit selection, and isolated ngspice runs."""
from .model import SimulationError, SimulationPlan, load_plan, parse_plan


def run_simulation(*args, **kwargs):
    from .ngspice import run_simulation as run
    return run(*args, **kwargs)


__all__ = ["SimulationError", "SimulationPlan", "load_plan", "parse_plan", "run_simulation"]
