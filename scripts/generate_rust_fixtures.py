"""Write the golden fixtures the Rust crates test against, from the Python oracle.

The Rust rewrite (docs/rust-port-plan.md) must reproduce Python's numeric,
JSON and state-file behaviour exactly. This script records that behaviour as
JSON fixtures under ``crates/*/tests/fixtures`` and the agent-guide topics the
binary embeds. Run it after a deliberate change to the oracle:

    python scripts/generate_rust_fixtures.py          # rewrite the fixtures
    python scripts/generate_rust_fixtures.py --check  # CI: fail if any is stale

Output is deterministic: seeded inputs, sorted keys and LF newlines on every
platform.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import struct
import sys
import tempfile
from dataclasses import replace
from decimal import Decimal, DivisionByZero, InvalidOperation, ROUND_HALF_EVEN, ROUND_HALF_UP
from fractions import Fraction
from pathlib import Path
from types import MappingProxyType

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

SCHEMA = "copperscript.rust-fixture/v1"
SEED = 20261027

DECIMAL_POOL = [
    "0", "-0", "0.00", "1", "-1", "3.3", "0.1", "2.5", "-2.5", "1.005", "100", "1E+3", "1e-9",
    "0.000001", "1000000", "12345.6789", "1E-30", "5E+20", "9999999999999999999999999999",
    "123456789012345678901234567890", "0.3333333333333333333333333333", "7", "-0.0000005",
]


def _write(path: Path, document) -> bytes:
    text = json.dumps(document, indent=1, sort_keys=True, ensure_ascii=True) + "\n"
    return text.encode("ascii")


def _random_decimal(rng: random.Random) -> str:
    digits = "".join(rng.choice("0123456789") for _ in range(rng.randint(1, 40))).lstrip("0") or "0"
    sign = rng.choice(["", "-"])
    return f"{sign}{digits}E{rng.randint(-45, 45)}"


def _outcome(operation):
    try:
        return {"result": str(operation())}
    except DivisionByZero:
        return {"error": "division_by_zero"}
    except InvalidOperation:
        return {"error": "division_undefined"}


def decimal_fixture() -> dict:
    from pcbir.physical import nm_from_mm
    from pcbir.quantities import decimal_text

    rng = random.Random(SEED)
    pool = DECIMAL_POOL + [_random_decimal(rng) for _ in range(12)]
    parse = []
    for text in pool + ["  1_000.5 ", ".5", "1.", "+7E-2", "0E+3", "-0.000", "1.50", "0.0000001", "1234567E-12"]:
        value = Decimal(text)
        parse.append({"text": text, "str": str(value), "fixed": format(value, "f"), "decimal_text": decimal_text(value)})
    binary = []
    operations = {"add": lambda a, b: a + b, "sub": lambda a, b: a - b, "mul": lambda a, b: a * b,
                  "div": lambda a, b: a / b}
    pairs = [(a, b) for a in DECIMAL_POOL for b in DECIMAL_POOL]
    pairs += [(_random_decimal(rng), _random_decimal(rng)) for _ in range(150)]
    for name, operation in operations.items():
        for a, b in pairs:
            binary.append({"op": name, "a": a, "b": b, **_outcome(lambda: operation(Decimal(a), Decimal(b)))})
    compare = [{"a": a, "b": b, "cmp": (Decimal(a) > Decimal(b)) - (Decimal(a) < Decimal(b))}
               for a, b in pairs[:200]]
    integral_inputs = pool + ["2.5", "3.5", "-2.5", "0.5", "-0.5", "1.4999", "-7.9", "123.456", "0.0000005"]
    integral = [{"value": text,
                 "half_even": str(Decimal(text).to_integral_value(rounding=ROUND_HALF_EVEN)),
                 "half_up": str(Decimal(text).to_integral_value(rounding=ROUND_HALF_UP)),
                 "int": str(int(Decimal(text)))} for text in integral_inputs]
    millimetres = ["0", "0.25", "1.6", "0.0000005", "-0.0000005", "0.00000049", "12.7", "-3.14159265",
                   "100.0000015", "0.1", "50", "0.075"]
    millimetres += [str(Decimal(rng.randint(-10**9, 10**9)) / Decimal(10 ** rng.randint(0, 9))) for _ in range(40)]
    nm = [{"mm": text, "nm": nm_from_mm(text)} for text in millimetres]
    normalize = [{"value": text, "normalize": str(Decimal(text).normalize())}
                 for text in pool + ["1.500", "-0.000", "100", "1E+2", "0.300", "120.0E-3"]]
    return {"schema": SCHEMA, "decimal": {"parse": parse, "binary": binary, "compare": compare,
                                          "integral": integral, "nm_from_mm": nm, "normalize": normalize}}


def quantity_fixture() -> dict:
    from pcbir import quantities

    rng = random.Random(SEED + 1)
    cases = []
    kinds = [quantities.Voltage, quantities.Resistance, quantities.Capacitance, quantities.Inductance,
             quantities.Length, quantities.Current, quantities.Frequency, quantities.Impedance,
             quantities.Time, quantities.Power, quantities.Energy, quantities.Charge]
    values = ["0", "1", "3.3", "100", "0.1", "4.7", "10", "2.2", "0.001", "1000", "47.5", "-1.8", "1.00"]
    for kind in kinds:
        for unit in kind.UNITS:
            for value in values + [str(Decimal(rng.randint(1, 99999)) / Decimal(10 ** rng.randint(0, 4)))]:
                quantity = kind.of(value, unit)
                cases.append({"kind": kind.__name__, "unit": unit, "value": value, "str": str(quantity),
                              "base_value": str(quantity.base_value)})
    errors = []
    for kind, unit in [(quantities.Voltage, "kV"), (quantities.Capacitance, "mF"), (quantities.Length, "in")]:
        try:
            kind.of("1", unit)
        except ValueError as error:
            errors.append({"kind": kind.__name__, "unit": unit, "message": str(error)})
    return {"schema": SCHEMA, "quantities": cases, "errors": errors}


def _float_cases(rng: random.Random) -> list[float]:
    fixed = [0.0, -0.0, 1.0, -1.0, 0.1, 0.5, 1e16, 1e15, 1234567890123456.0, 12345678901234567.0, 0.0001,
             0.00001, 1e-7, 1.5e300, 5e-324, 2.2250738585072014e-308, 1.7976931348623157e308, 123.456,
             2.675, 1 / 3, 2 / 3, 100.0, 1e22, 1e23, 9007199254740993.0, 0.30000000000000004, 3.3, 1e-05,
             0.000123, 99999999999999990.0]
    randoms = []
    while len(randoms) < 400:
        value = struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
        if value == value and abs(value) != float("inf"):
            randoms.append(value)
    decimals = [rng.randint(-10**6, 10**6) / 10 ** rng.randint(0, 8) for _ in range(200)]
    return fixed + randoms + decimals


def json_fixture() -> dict:
    rng = random.Random(SEED + 2)
    floats = [{"bits": struct.pack(">d", value).hex(), "repr": repr(value)} for value in _float_cases(rng)]
    strings = ["", "plain", "quote \" and backslash \\", "tab\tnew\nline\rcr\bbs\ffeed", "\x00\x01\x1f\x7f",
               "caf\u00e9", "\u2014 dash", "emoji \U0001F600", "\u2028\u2029", "/slash", "\U0010FFFF"]
    string_cases = [{"value": text, "ascii": json.dumps(text), "unicode": json.dumps(text, ensure_ascii=False)}
                    for text in strings]
    documents = []
    for index in range(6):
        document = _random_document(rng, depth=0)
        if index == 0:
            document = {"z": [], "a": {}, "m": [1, 2.5, None, True, False, "x"], "\u00e9": {"b": 1, "a": 2}}
        source = json.dumps(document)
        documents.append({
            "source": source,
            "default": json.dumps(document),
            "compact_sorted": json.dumps(document, sort_keys=True, separators=(",", ":")),
            "indent2": json.dumps(document, indent=2),
            "indent2_sorted": json.dumps(document, indent=2, sort_keys=True),
            "unicode": json.dumps(document, ensure_ascii=False),
        })
    return {"schema": SCHEMA, "floats": floats, "strings": string_cases, "documents": documents}


def _random_document(rng: random.Random, depth: int):
    kind = rng.choice(["dict", "list", "scalar"] if depth < 3 else ["scalar"])
    if kind == "dict":
        return {rng.choice(["a", "b", "key", "\u00e9t\u00e9", "Z", "_", "10", "2"]) + str(rng.randint(0, 9)):
                _random_document(rng, depth + 1) for _ in range(rng.randint(0, 4))}
    if kind == "list":
        return [_random_document(rng, depth + 1) for _ in range(rng.randint(0, 4))]
    return rng.choice([None, True, False, rng.randint(-10**20, 10**20), rng.random() * 10 ** rng.randint(-8, 20),
                       "text", "\u00b5m", 0, -0.0])


def _state_samples() -> list[tuple[str, object, dict]]:
    import numpy
    from pcbir.loader import load_design
    from pcbir.physical import BoardSide, CopperLayer, Point
    from pcbir.physicalize import PrototypePhysicalOptions, prototype_physicalize

    samples = []
    for name in ("valid_board", "hierarchical_board"):
        design = load_design(ROOT / "examples" / name / "board.copper", locked=True, offline=True)
        physical = prototype_physicalize(design, PrototypePhysicalOptions())
        if name == "valid_board":
            rotations, sides = (0, 90, 180, 270, 45), (BoardSide.FRONT, BoardSide.BACK)
            physical = replace(physical, placements=tuple(
                replace(p, rotation_degrees=Decimal(rotations[i % 5]), side=sides[(i // 2) % 2])
                for i, p in enumerate(physical.placements)))
        samples.append((name.replace("_", "-"), physical, {"source": f"examples/{name}/board.copper"}))
    synthetic = (
        Point(1_000_000, -250_000), CopperLayer.INTERNAL_2, BoardSide.BACK,
        Decimal("1.50"), Decimal("-0"), Decimal("1E-9"), Fraction(-6, 8), Fraction(10**30, 3),
        frozenset({3, 1, 2, "b", "a"}), {("x", 1), ("y", 0)}, frozenset(),
        MappingProxyType({"b": 1, "a": (1, 2)}), {(1, 2): "pair", "k": None, 3: [1.5, -0.0]},
        numpy.arange(6, dtype=numpy.int64).reshape(2, 3), numpy.array([0.5, -1e-9, 1e300], dtype=numpy.float64),
        numpy.array([True, False]), numpy.array([], dtype=numpy.uint8), numpy.int32(7), numpy.float64(0.25),
        ["caf\u00e9", "emoji \U0001F600", "ctrl \x01\t\n", ""], [1e16, 1e-5, 0.1, float("inf"), float("nan")],
        10**40, -1, True, None,
    )
    samples.append(("synthetic", synthetic, {"note": "every state tag", "count": 3,
                                             "nested": {"b": [1, 2.5], "a": None, "\u00e9": "x"}}))
    return samples


def state_fixtures() -> dict[str, bytes]:
    from pcbir.state_codec import save_state

    files = {}
    with tempfile.TemporaryDirectory() as directory:
        for name, value, metadata in _state_samples():
            path = save_state(Path(directory) / f"{name}.state.json", value, **metadata)
            # save_state writes text mode; fixtures keep LF on every platform.
            files[f"{name}.state.json"] = path.read_bytes().replace(b"\r\n", b"\n")
    return files


def agent_guide_topics() -> dict:
    from pcbir.agent_guide import GUIDE_VERSION, SCHEMA as GUIDE_SCHEMA, _TOPICS

    return {"schema": GUIDE_SCHEMA, "guide_version": GUIDE_VERSION,
            "topics": [{"name": name, "content": content} for name, content in _TOPICS.items()]}


# Hand-written sources aimed at each parser branch and diagnostic.
PARSE_SNIPPETS = [
    "", "board", "board B", "board B {", "frob B {}", "board 7 { }", "board B { } extra",
    "board B { componnet R1: X; }", "module M { nett N { A.1; } }", "board B { zzzz; }",
    'board B { use libary "x"; }', 'board B { use library x; }',
    "part P { pin A { x = 1; x = 2; } }", "board B { x @ }", 'board B { s = "a\\q"; }',
    'board B { component R: X { v = "abc }', "board_profile P { widget w { } }",
    "board B { mechanical { stackup { metal L1 { } } } }", "module M { mechanical { } }",
    "part P { silkscreen { } }", "part P { physical_macro M: a { } }", "board B { silkscreen { banner x { } } }",
    "board B { power_state S { R = 1; } }", "board B { configure U.P as C { S = 2V; } }",
    "board B { configure U.P to C { S = x; } }", "board B { interface I: i2c { bind U { SDA = 1; } } }",
    "board B { component R: X { v = ; } }", "device D { peripheral P: uart { sig TX: out; } }",
    "board B { physical_macro M: a { member r = U1; member r = U2; } }",
    "board B { physical_macro M: a { pin r = U1; } }", 'board B { physical_macro M: a { member "" = U1; } }',
    "macro M { x = { a = 1; a = 2; }; }", "macro M { pad U.1 N; }", "board B { net N { A; } }",
    "board B { constraint length(A, ) { } }", "board B { mechanical { hole h { at = (1, ); } } }",
    "board B { component R: X { v = -x; } }", "board B { component R: X { v = 1.2.3; } }",
    "device D { mux PA0: USART1 TX { } }", "board B { mechanical { outline rect { size = [(1mm, 2mm)]; size = 1; } } }",
    "board B { no_connect U1.NC1, { } }", "board B { interface I: x { a = 1; a = 2; } }",
    'board B {\n  use library "tiny";\n  import lib "github.com/x/y";\n  component R1: RESISTOR { value = 10kohm; tol = -1.5; big = 123456789012345678901234567890; f = 0.1; on = true; off = false; pin = U1.A; s = "q\\"\\n\\t\\\\"; }\n  component C1: lib.parts.CAP;\n  port VIN: power_in;\n  module SUB: lib.Sub;\n  net N1 { R1.1; SUB.VIN.X; }\n  no_connect U1.NC1, U1.NC2 { reason = "unused"; }\n  no_connect U1.NC3;\n  supply VIN { voltage = 3.3V; external = true; }\n  interface BUS: i2c { speed = 400kHz; bind U1 { SDA = PB7; SCL = PB6; } }\n  constraint length_match(N1, SUB/U2/U3.PIN, X.Y) { tolerance = +0.5mm; }\n  constraint keepout() { }\n  configure U1.USART1 as console { TX = PA9; }\n  power_state SLEEP { VIN = off; }\n  physical_macro BUCK: layout on SUB { member "U1" = U1; net VIN = port.VIN; }\n  physical_macro PLAIN: lib.Def { }\n  mechanical {\n    outline rectangle { size = (50mm, 40mm); corner_radius = 1mm; }\n    cutout c1 polygon { points = [(0mm, 0mm), (1mm, 0mm), (-1mm, -2.5mm)]; }\n    hole h1 { at = (5mm, 5mm); diameter = 3.2mm; }\n    rules { clearance = 0.2mm; }\n    stackup { copper F.Cu { thickness = 35um; } dielectric core { er = 4.5; } }\n    use lib.profiles.Pi as pi { offset = 1mm; }\n    use lib.profiles.Hat { }\n  }\n  silkscreen { branding { enabled = false; } text t1 { value = "Hi"; at = (1mm, 2mm); } }\n}',
    'device D {\n  vendor = "ST";\n  pad PA0 { ball = A1; }\n  power_domain VDD { min = 1.7V; }\n  peripheral USART1: uart { signal TX: output; signal RX: input { pull = up; } }\n  mux PA9: USART1.TX { af = 7; }\n  resource DMA1;\n  unit U: lib.Kind { a = PA0; }\n  group G: diff.pair { p = PA1; }\n  mode_group M { x = 1; }\n  pad_set S { pads = "PA0,PA1"; }\n  route USART1.TX { max_length = 10mm; }\n}',
    'part P { footprint = "R_0402"; pin "1" { number = "1"; } pin A { domains = "digital"; } }',
    'macro M {\n  import fp "x/y.kicad_mod";\n  version = 1;\n  evidence = { source = "datasheet"; pages = [1, 2]; ref = U.1; nested = { k = (1, -2mm); }; };\n  list = [];\n  tuple = ();\n  pad U.1 = VIN;\n  pad "U2"."EP" = "GND";\n  isolated U.3;\n  net VIN;\n  member U { footprint = fp; at = (1mm, 2mm); contact c1 { pad = U.1; } contact = 5; }\n  silkscreen S { }\n}',
    'board_profile Pi {\n  connector J1 { at = (3.5mm, 3.5mm); }\n  outline rectangle { size = (85mm, 56mm); }\n  use other.P as q { }\n}',
]


def _parse_outcome(text: str, filename: str) -> dict:
    from pcbir.parser import parse
    from pcbir.state_codec import encode
    from pcbir.syntax import CopperScriptError

    try:
        document = parse(text, filename)
    except CopperScriptError as error:
        location = error.location
        return {"error": {"code": error.code, "message": error.message, "filename": location.filename,
                          "offset": location.offset, "line": location.line, "column": location.column,
                          "hint": error.hint, "replacement": error.replacement, "text": str(error)}}
    body = json.dumps(encode(document), sort_keys=True, separators=(",", ":"))
    return {"digest": "sha256:" + hashlib.sha256(body.encode()).hexdigest()}


def parse_sources() -> list[str]:
    patterns = ("examples/**/*.copper", "tests/**/*.copper")
    # Package caches (.copper-cache) and other hidden trees are not repository sources.
    return sorted({path.relative_to(ROOT).as_posix() for pattern in patterns for path in ROOT.glob(pattern)
                   if not any(part.startswith(".") for part in path.relative_to(ROOT).parts)})


def mutate(text: str, mutation: list | None) -> str:
    if mutation is None:
        return text
    if mutation[0] == "truncate":
        return text[:mutation[1]]
    if mutation[0] == "delete":
        return text[:mutation[1]] + text[mutation[1] + 1:]
    return text[:mutation[1]] + mutation[2] + text[mutation[1]:]


def parse_fixture() -> dict:
    rng = random.Random(SEED + 3)
    cases = []
    for source in parse_sources():
        text = (ROOT / source).read_text(encoding="utf-8")
        mutations = [None]
        mutations += [["truncate", rng.randrange(len(text))] for _ in range(8)]
        mutations += [["delete", rng.randrange(len(text))] for _ in range(8)]
        mutations += [["insert", rng.randrange(len(text) + 1), rng.choice('{};=."@a1\\/-(),[]')] for _ in range(4)]
        for mutation in mutations:
            cases.append({"source": source, "mutation": mutation, **_parse_outcome(mutate(text, mutation), source)})
    for index, text in enumerate(PARSE_SNIPPETS):
        cases.append({"text": text, "filename": f"snippet-{index}.copper", **_parse_outcome(text, f"snippet-{index}.copper")})
    return {"schema": SCHEMA, "cases": cases}


def outputs() -> dict[Path, bytes]:
    crates = ROOT / "crates"
    result = {
        crates / "copper-units/tests/fixtures/decimal.json": _write(None, decimal_fixture()),
        crates / "copper-units/tests/fixtures/quantities.json": _write(None, quantity_fixture()),
        crates / "copper-json/tests/fixtures/python-json.json": _write(None, json_fixture()),
        crates / "copper-lang/tests/fixtures/parse.json": _write(None, parse_fixture()),
        # Not a fixture: the guide text the binary embeds, copied from the oracle
        # until cutover makes the Rust copy the source.
        crates / "copper/src/agent_guide_topics.json": _write(None, agent_guide_topics()),
    }
    for name, data in state_fixtures().items():
        result[crates / "copper-state/tests/fixtures" / name] = data
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="fail if any committed fixture differs")
    parser.add_argument("--dump-parse", type=Path, metavar="SOURCE",
                        help="print Python's state encoding of SOURCE's parse tree (to diff with the Rust test)")
    parser.add_argument("--parse-corpus", type=Path, metavar="DIR",
                        help="write a parse fixture for every .copper file under DIR to --output, for "
                             "COPPER_PARSE_FIXTURE=FILE cargo test -p copper-lang")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if args.dump_parse:
        from pcbir.parser import parse
        from pcbir.state_codec import encode
        source = args.dump_parse
        print(json.dumps(encode(parse(source.read_text(encoding="utf-8"), source.as_posix())), indent=1, sort_keys=True))
        return 0
    if args.parse_corpus:
        if not args.output:
            parser.error("--parse-corpus needs --output")
        sources = sorted(args.parse_corpus.resolve().rglob("*.copper"))
        cases = [{"source": source.as_posix(), "mutation": None,
                  **_parse_outcome(source.read_text(encoding="utf-8"), source.as_posix())} for source in sources]
        args.output.write_text(json.dumps({"schema": SCHEMA, "cases": cases}, indent=1, sort_keys=True) + "\n",
                               encoding="utf-8")
        print(f"Wrote {len(cases)} parse cases to {args.output}")
        return 0
    stale = []
    for path, data in outputs().items():
        current = path.read_bytes() if path.is_file() else None
        if current == data:
            continue
        stale.append(path.relative_to(ROOT).as_posix())
        if not args.check:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    if args.check and stale:
        print("Stale Rust fixtures; run python scripts/generate_rust_fixtures.py:\n  " + "\n  ".join(stale))
        return 1
    print(("Up to date" if not stale else "Wrote " + ", ".join(stale)) if not args.check else "Rust fixtures are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
