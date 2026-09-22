from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path

from pcbir import (
    CamGateStatus, CamQualificationProfile, ConstraintCheckStatus,
    ConstraintMode, NormalizedCamLayer, NormalizedConstraint, ToolIdentity,
    constraint_coverage, qualify_cam_artifacts,
    PyGerberAdapter, parse_xnc,
    parse_ipcd356, reconcile_drills, reconcile_test_net,
    BoardOutline, PhysicalBoard, PhysicalNet, Point, Via, nm_from_mm,
    CamCorpusCase, run_cam_qualification_matrix,
)


ROOT = Path(__file__).parents[1]


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


def test_unsafe_artifact_fails_even_when_second_tool_is_missing(tmp_path: Path) -> None:
    (tmp_path / "top.gbr").write_bytes(b"oversized")
    one, two = _tool("parser-a"), _tool("parser-b")
    profile = CamQualificationProfile("release", "2026.05", "2021.11", (one, two),
                                      maximum_file_bytes=4)
    result = qualify_cam_artifacts(tmp_path, profile, (_Adapter(one, "same"),))
    assert result.status is CamGateStatus.FAIL
    assert any("size limit" in finding for finding in result.findings)


def test_unconsumed_hard_constraint_blocks_release() -> None:
    constraints = (
        NormalizedConstraint("usb.skew", "usb", "route.skew", ConstraintMode.REQUIRE,
                             "<=100ps", ("board.copper:20",)),
    )
    result = constraint_coverage(constraints, {})
    assert result[0].status is ConstraintCheckStatus.BLOCKED


def test_pinned_pygerber_adapter_and_strict_xnc_parser(tmp_path: Path) -> None:
    gerber = tmp_path / "top.gbr"
    gerber.write_text(
        "G04 fixture*\n%FSLAX46Y46*%\n%MOMM*%\n"
        "%TF.FileFunction,Copper,L1,Top*%\n%TF.FilePolarity,Positive*%\n"
        "%ADD10C,1.0*%\nD10*\nX0000000000Y0000000000D02*\n"
        "X0010000000Y0000000000D01*\nM02*\n", encoding="ascii")
    layer = PyGerberAdapter().parse_gerber(gerber)
    assert layer.file_function == "Copper,L1,Top"
    assert layer.units == "mm" and layer.bounds_nm[2] > layer.bounds_nm[0]
    drill = tmp_path / "board.drl"
    drill.write_text("M48\nMETRIC\nT1C0.300\n%\nT1\nX1.000Y2.000\nM30\n", encoding="ascii")
    program = parse_xnc(drill)
    assert program.hits[0].diameter_nm == 300_000


def test_committed_cam_corpus_qualifies_pinned_pygerber() -> None:
    fixture = ROOT / "tests" / "fixtures" / "cam"
    cases = (
        CamCorpusCase(
            "positive-line",
            fixture / "positive_line.gbr",
            True,
            "Copper,L1,Top",
            "Positive",
            "mm",
            (-500_000, -500_000, 10_500_000, 10_500_000),
        ),
        CamCorpusCase(
            "negative-undefined-aperture",
            fixture / "negative_undefined_aperture.gbr",
            False,
        ),
        CamCorpusCase(
            "positive-inch-line",
            fixture / "positive_inch_line.gbr",
            True,
            "Copper,L2,Bot",
            "Positive",
            "inch",
            (-127_000, -127_000, 25_527_000, 25_527_000),
        ),
        CamCorpusCase(
            "negative-missing-end",
            fixture / "negative_missing_end.gbr",
            False,
        ),
    )

    pygerber = PyGerberAdapter()
    other = _tool("libgerbv")
    profile = CamQualificationProfile("corpus", "2026.05", "2021.11",
                                      (pygerber.identity, other))
    incomplete = run_cam_qualification_matrix(profile, cases, (pygerber,))
    assert incomplete.status is CamGateStatus.INCOMPLETE
    assert incomplete.corpus_hashes == tuple(sorted(incomplete.corpus_hashes))
    assert len(incomplete.cells) == 4
    matrix = run_cam_qualification_matrix(profile, cases, (
        pygerber, _CorpusAdapter(other),
    ))

    assert matrix.passed, matrix.cells


@dataclass
class _CorpusAdapter:
    identity: ToolIdentity

    def parse_gerber(self, path: Path) -> NormalizedCamLayer:
        if "negative" in path.name:
            raise ValueError("undefined aperture")
        if "inch" in path.name:
            return NormalizedCamLayer("Copper,L2,Bot", "Positive", "inch",
                                      (-127_000, -127_000, 25_527_000, 25_527_000),
                                      "fixture")
        return NormalizedCamLayer("Copper,L1,Top", "Positive", "mm",
                                  (-500_000, -500_000, 10_500_000, 10_500_000), "fixture")


def test_drill_multiset_and_ipcd356_partition_reconcile_to_physical_ir(tmp_path: Path) -> None:
    board = PhysicalBoard("Drill", BoardOutline.rectangle(10, 10), {}, (),
                          (PhysicalNet("N", ()),),
                          vias=(Via("N", Point.mm(1, 2), nm_from_mm("0.6"),
                                    nm_from_mm("0.3")),))
    drill = tmp_path / "board.drl"
    drill.write_text("M48\nMETRIC\nT1C0.300\n%\nT1\nX1.000Y2.000\nM30\n", encoding="ascii")
    assert reconcile_drills(board, (parse_xnc(drill, plated=True),)).passed

    from test_manufacturing import _board
    electrical = _board()
    d356 = tmp_path / "board.d356"
    d356.write_text(
        "P  CODE 00\nP  UNITS CUST 0\n"
        "327SIGNAL           J1    -1          A01X+001181Y-002362X0236Y0236R000S2\n"
        "327SIGNAL           J2    -1          A01X+006693Y-002362X0236Y0236R000S2\n999\n",
        encoding="ascii",
    )
    parsed = parse_ipcd356(d356)
    assert reconcile_test_net(electrical, parsed).passed
