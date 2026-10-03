"""Generic named-library registry. Part data belongs to library packages."""
from collections.abc import Callable
from importlib.metadata import entry_points
from .model import PartDefinition


LIBRARIES: dict[str, Callable[[], dict[str, PartDefinition]]] = {}


def library_factory(name: str) -> Callable[[], dict[str, PartDefinition]] | None:
    """Resolve a registered factory or installed library entry point.

    The engine has no concrete part data or knowledge of example libraries.
    Applications may register factories directly; distributions may expose
    them in the ``copperscript.libraries`` entry-point group.
    """
    if name in LIBRARIES:
        return LIBRARIES[name]
    matches = tuple(entry_points(group="copperscript.libraries", name=name))
    if len(matches) > 1:
        raise ValueError(f"multiple installed libraries named {name!r}")
    return matches[0].load() if matches else None
