from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path

from pcbir import (
    CamGateStatus, CamQualificationProfile, ConstraintCheckStatus,
    ConstraintMode, NormalizedCamLayer, NormalizedConstraint, ToolIdentity,
    constraint_coverage, qualify_cam_artifacts,
)


@dataclass
class _Adapter:
    identity: ToolIdentity
    digest: str

    def parse_gerber(self, path: Path) -> NormalizedCamLayer:
        return NormalizedCamLayer("Copper,L1,Top", "Positive", "mm", (0, 0, 1, 1), self.digest)


def _tool(name: str) -> ToolIdentity:
    return ToolIdentity(name, "1.0", sha256(name.encode()).hexdigest())


def test_cam_qualification_requires_pinned_tools_and_agreement(tmp_path: Path) -> None:
    (tmp_path / "top.gbr").write_text("gerber", encoding="ascii")
    one, two = _tool("parser-a"), _tool("parser-b")
    profile = CamQualificationProfile("release", "2026.05", "2021.11", (one, two))
    incomplete = qualify_cam_artifacts(tmp_path, profile, (_Adapter(one, "same"),))
    assert incomplete.status is CamGateStatus.INCOMPLETE
    passed = qualify_cam_artifacts(tmp_path, profile, (_Adapter(one, "same"), _Adapter(two, "same")))
    assert passed.status is CamGateStatus.PASS
    failed = qualify_cam_artifacts(tmp_path, profile, (_Adapter(one, "a"), _Adapter(two, "b")))
    assert failed.status is CamGateStatus.FAIL


def test_unconsumed_hard_constraint_blocks_release() -> None:
    constraints = (
        NormalizedConstraint("usb.skew", "usb", "route.skew", ConstraintMode.REQUIRE,
                             "<=100ps", ("board.copper:20",)),
    )
    result = constraint_coverage(constraints, {})
    assert result[0].status is ConstraintCheckStatus.BLOCKED
