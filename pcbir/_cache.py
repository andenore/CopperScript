"""Identity keys that retain immutable inputs in bounded memo tables."""


class IdentityKey:
    """Avoid repeatedly hashing large frozen geometry; never accept mutable data.

    Retaining the input prevents identity reuse while a cache entry is alive.
    Callers must bound their caches and include every input that affects results.
    """

    __slots__ = ("value",)

    def __init__(self, value: object) -> None:
        self.value = value

    def __hash__(self) -> int:
        return id(self.value)

    def __eq__(self, other: object) -> bool:
        return isinstance(other, IdentityKey) and self.value is other.value
