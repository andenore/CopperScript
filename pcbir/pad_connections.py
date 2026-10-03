"""Package connectivity facts, independent of coordinates and CAD syntax."""
from dataclasses import dataclass
from typing import Iterable


@dataclass(frozen=True, slots=True)
class InternalPadGroup:
    """Always-conductive component connection, not a switch or a net tie.

    One number joins its duplicate physical lands. Multiple numbers join all
    their lands. Membership never means that required current/thermal contacts
    can be omitted; only declare groups for which one external contact suffices.
    """

    numbers: tuple[str, ...]

    def __post_init__(self) -> None:
        numbers = tuple(self.numbers)
        if not numbers or any(not isinstance(n, str) or not n or any(c.isspace() or c in ',;' for c in n) for n in numbers):
            raise ValueError("internal pad groups require nonempty pad numbers")
        if len(numbers) != len(set(numbers)):
            raise ValueError("internal pad group repeats a pad number")
        object.__setattr__(self, "numbers", tuple(sorted(numbers)))


def validate_internal_pad_groups(groups: Iterable[InternalPadGroup],
                                known_numbers: Iterable[str]) -> tuple[InternalPadGroup, ...]:
    result = tuple(groups)
    seen = set()
    known_numbers = set(known_numbers)
    for group in result:
        if not isinstance(group, InternalPadGroup):
            raise ValueError("internal pad groups must be InternalPadGroup values")
        if set(group.numbers) - set(known_numbers):
            raise ValueError("internal pad group references unknown pad number")
        if seen.intersection(group.numbers):
            raise ValueError("internal pad groups overlap")
        seen.update(group.numbers)
    return tuple(sorted(result, key=lambda group: group.numbers))


def merge_internal_pad_groups(*collections: Iterable[InternalPadGroup]) -> tuple[InternalPadGroup, ...]:
    """Union corroborating package/imported facts into disjoint groups."""
    groups = []
    for collection in collections:
        for group in collection:
            current = set(group.numbers)
            untouched = []
            for existing in groups:
                if current.intersection(existing):
                    current.update(existing)
                else:
                    untouched.append(existing)
            groups = [*untouched, current]
    return tuple(sorted((InternalPadGroup(tuple(g)) for g in groups), key=lambda g: g.numbers))
