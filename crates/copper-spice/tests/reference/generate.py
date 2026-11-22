"""Reference results for the decks in decks/, produced with ngspice.

This script is run by a developer, by hand, when a deck is added or changed.
`copper` and its tests never run ngspice: the test
(`crates/copper-spice/tests/reference_comparison.rs`) only reads the CSV
files this script writes to results/.

It drives the ngspice shared library through ctypes (ngSpice_Init,
ngSpice_Circ, ngSpice_Command, ngGet_Vec_Info). By default it loads the
library KiCad 10 ships; NGSPICE_DLL names another one.

    python crates/copper-spice/tests/reference/generate.py [DECK.cir ...]

Without arguments every deck is regenerated. See README.md for the deck
directives (`*@ probe`, `*@ temps`) and the CSV layout.
"""

import ctypes
import os
import re
import sys
from ctypes import CFUNCTYPE, POINTER, Structure, c_bool, c_char_p, c_double, c_int, c_void_p

HERE = os.path.dirname(os.path.abspath(__file__))
DECKS = os.path.join(HERE, "decks")
DEFAULT_DLL = r"C:\Program Files\KiCad\10.0\bin\ngspice.dll"
# The code models KiCad ships beside it; spice2poly implements POLY sources.
DEFAULT_CODE_MODELS = r"C:\Program Files\KiCad\10.0\lib\ngspice"

ANALYSES = (".op", ".dc", ".ac", ".noise", ".tran")
# The gitignored directory of decks that include vendor models (README, vendor models).
VENDOR = os.path.join(HERE, "vendor")


class NgComplex(Structure):
    _fields_ = [("re", c_double), ("im", c_double)]


class VectorInfo(Structure):
    _fields_ = [
        ("name", c_char_p),
        ("type", c_int),
        ("flags", ctypes.c_short),
        ("real", POINTER(c_double)),
        ("complex", POINTER(NgComplex)),
        ("length", c_int),
    ]


SEND_CHAR = CFUNCTYPE(c_int, c_char_p, c_int, c_void_p)
SEND_STAT = CFUNCTYPE(c_int, c_char_p, c_int, c_void_p)
CONTROLLED_EXIT = CFUNCTYPE(c_int, c_int, c_bool, c_bool, c_int, c_void_p)
SEND_DATA = CFUNCTYPE(c_int, c_void_p, c_int, c_int, c_void_p)
SEND_INIT_DATA = CFUNCTYPE(c_int, c_void_p, c_int, c_void_p)
BG_THREAD = CFUNCTYPE(c_int, c_bool, c_int, c_void_p)


class Ngspice:
    def __init__(self, path):
        os.add_dll_directory(os.path.dirname(path))
        self.lib = ctypes.CDLL(path)
        self.output = []
        self.exited = False

        def send_char(text, _id, _user):
            self.output.append(text.decode(errors="replace"))
            return 0

        def controlled_exit(status, _unload, _quit, _id, _user):
            self.exited = True
            self.output.append(f"ngspice exit {status}")
            return 0

        # Kept on self so the callbacks outlive this call.
        self.callbacks = [
            SEND_CHAR(send_char),
            SEND_STAT(lambda *_: 0),
            CONTROLLED_EXIT(controlled_exit),
            SEND_DATA(lambda *_: 0),
            SEND_INIT_DATA(lambda *_: 0),
            BG_THREAD(lambda *_: 0),
        ]
        self.lib.ngSpice_Init(*self.callbacks, None)
        self.lib.ngSpice_Command.argtypes = [c_char_p]
        self.lib.ngSpice_Circ.argtypes = [POINTER(c_char_p)]
        self.lib.ngSpice_AllPlots.restype = POINTER(c_char_p)
        self.lib.ngSpice_AllVecs.argtypes = [c_char_p]
        self.lib.ngSpice_AllVecs.restype = POINTER(c_char_p)
        self.lib.ngGet_Vec_Info.argtypes = [c_char_p]
        self.lib.ngGet_Vec_Info.restype = POINTER(VectorInfo)

    def command(self, text):
        return self.lib.ngSpice_Command(text.encode())

    def version(self):
        self.output.clear()
        self.command("version -f")
        for line in self.output:
            m = re.search(r"\*\* (ngspice-\S+)", line)
            if m:
                return m.group(1)
        return "unknown"

    def run(self, lines):
        """Load `lines` (title first, .end last), run, return {name: [values]} of the current plot."""
        self.output.clear()
        array = (c_char_p * (len(lines) + 1))(*[line.encode() for line in lines], None)
        if self.lib.ngSpice_Circ(array) != 0:
            raise RuntimeError("ngSpice_Circ failed:\n" + "\n".join(self.output))
        self.command("run")
        failed = [o for o in self.output if re.search(r"error|singular|too small|aborted", o, re.I)]
        # Every plot of the run (a noise analysis makes two: spectrum and totals),
        # its vectors merged; the scale vectors (frequency, time) are shared.
        plots = []
        names = self.lib.ngSpice_AllPlots()
        i = 0
        while names[i]:
            plots.append(names[i].decode())
            i += 1
        vectors = {}
        for plot in plots:
            if plot == "const":
                continue
            names = self.lib.ngSpice_AllVecs(plot.encode())
            i = 0
            while names[i]:
                name = names[i].decode()
                info = self.lib.ngGet_Vec_Info(f"{plot}.{name}".encode())
                if info:
                    v = info.contents
                    if v.real:
                        values = [complex(v.real[k], 0.0) for k in range(v.length)]
                    else:
                        values = [complex(v.complex[k].re, v.complex[k].im) for k in range(v.length)]
                    vectors.setdefault(name.lower(), values)
                i += 1
        self.command("destroy all")
        self.command("remcirc")
        if failed or not vectors:
            raise RuntimeError(f"ngspice run failed (plots {plots}):\n" + "\n".join(self.output))
        return vectors


def directives(text):
    """`*@ probe`, `*@ tran-probe` (the probes of a transient, when they
    differ), `*@ temps`, `*@ ngspice-options` (options for ngspice only,
    such as `badmos3`, which copper-sim's reader rejects) and `*@ ngbehavior`
    (ngspice's compatibility mode for the deck, `psa` for a PSpice model)."""
    probes, tran_probes, temps, options = [], [], [27.0], []
    behavior = None
    for line in text.splitlines():
        m = re.match(r"\*@\s*([\w-]+)\s*(.*)", line.strip())
        if not m:
            continue
        key, rest = m.group(1).lower(), m.group(2)
        if key == "probe":
            probes += re.findall(r"[vi]\([^)]*\)", rest.lower())
        elif key == "tran-probe":
            tran_probes += re.findall(r"[vi]\([^)]*\)", rest.lower())
        elif key == "temps":
            temps = [float(t) for t in rest.split()]
        elif key == "ngspice-options":
            options.append(".options " + rest)
        elif key == "ngbehavior":
            behavior = rest.strip()
    return probes, tran_probes or probes, temps, options, behavior


def split_deck(text):
    """(title, body lines, analysis cards): continuation lines joined to their card."""
    lines = text.splitlines()
    title, body, cards = lines[0], [], []
    last_was_card = False
    for line in lines[1:]:
        s = line.strip()
        low = s.lower()
        if s.startswith("+") and last_was_card:
            cards[-1] += " " + s[1:].strip()
            continue
        last_was_card = low.split(" ")[0] in ANALYSES
        if last_was_card:
            cards.append(s)
        elif low != ".end":
            body.append(line)
    return title, body, cards


def probe_value(vectors, probe):
    kind, args = probe[0], [a.strip() for a in probe[2:-1].split(",")]
    def node(n):
        if n == "0":
            return None
        return vectors[n]
    if kind == "v":
        a = node(args[0])
        b = node(args[1]) if len(args) > 1 else None
        if b is None:
            return a
        if a is None:
            return [-x for x in b]
        return [x - y for x, y in zip(a, b)]
    return vectors[f"{args[0]}#branch"]


def scale_of(kind, vectors):
    if kind == "op":
        return [0.0]
    if kind == "dc":
        for name, v in vectors.items():
            if name.endswith("-sweep"):
                return [x.real for x in v]
        raise RuntimeError(f"no sweep vector in {sorted(vectors)}")
    if kind in ("ac", "noise"):
        return [x.real for x in vectors["frequency"]]
    return [x.real for x in vectors["time"]]


def num(x):
    """A value to ten significant digits (the comparison's bounds are 1e-5
    and wider), 0 as 0. The sweep values (x) are written exactly: the test
    runs copper-sim at them, and a sweep point on a threshold (a switch's,
    a ternary's) must be the same point."""
    return "0" if x == 0 else f"{x:.10g}"


def generate(ng, deck_path):
    text = open(deck_path, encoding="utf-8").read()
    probes, tran_probes, temps, options, behavior = directives(text)
    # The compatibility mode is read when a circuit is loaded.
    ng.command(f"set ngbehavior={behavior}" if behavior else "unset ngbehavior")
    title, body, cards = split_deck(text)
    rows = []
    for index, card in enumerate(cards):
        kind = card.split()[0][1:].lower()
        for temp in temps:
            lines = [title] + body + options + [f".temp {temp:g}", card, ".end"]
            vectors = ng.run(lines)
            x = scale_of(kind, vectors)
            if kind == "noise":
                quantities = [("onoise", vectors["onoise_spectrum"]), ("inoise", vectors["inoise_spectrum"])]
            else:
                quantities = [(p, probe_value(vectors, p)) for p in (tran_probes if kind == "tran" else probes)]
            for name, values in quantities:
                if len(values) != len(x):
                    raise RuntimeError(f"{deck_path}: {name} has {len(values)} points, the scale {len(x)}")
                field = f'"{name}"' if "," in name else name
                for xv, v in zip(x, values):
                    rows.append(f"{index}:{kind},{temp:g},{field},{xv!r},{num(v.real)},{num(v.imag)}")
    return rows


def short_path(path):
    """`path` without spaces (ngspice's codemodel command does not take quotes)."""
    if os.name != "nt" or " " not in path:
        return path
    buffer = ctypes.create_unicode_buffer(1024)
    ctypes.windll.kernel32.GetShortPathNameW(path, buffer, len(buffer))
    return buffer.value.replace("\\", "/")


def main():
    dll = os.environ.get("NGSPICE_DLL", DEFAULT_DLL)
    ng = Ngspice(dll)
    # The library finds no spinit, so the code model POLY needs is loaded here,
    # and for the vendor decks the ones ngspice's PSpice mode translates
    # LAPLACE and VSWITCH into (s_xfer in analog.cm, aswitch in xtradev.cm,
    # which needs digital.cm).
    code_models = os.environ.get("NGSPICE_CODE_MODELS", DEFAULT_CODE_MODELS)
    ng.command(f"codemodel {short_path(os.path.join(code_models, 'spice2poly.cm'))}")
    if VENDOR in os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ""):
        for cm in ("analog.cm", "digital.cm", "xtradev.cm"):
            ng.command(f"codemodel {short_path(os.path.join(code_models, cm))}")
    version = ng.version()
    decks = sys.argv[1:] or sorted(
        os.path.join(DECKS, f) for f in os.listdir(DECKS) if f.endswith(".cir")
    )
    for deck in decks:
        name = os.path.splitext(os.path.basename(deck))[0]
        rows = generate(ng, deck)
        # results/ beside the deck's decks/ directory.
        results = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(deck))), "results")
        os.makedirs(results, exist_ok=True)
        out = os.path.join(results, name + ".csv")
        with open(out, "w", encoding="utf-8", newline="\n") as f:
            f.write(f"# {version}; generated by generate.py from decks/{name}.cir\n")
            f.write("analysis,temp,quantity,x,re,im\n")
            f.write("\n".join(rows) + "\n")
        print(f"{name}: {len(rows)} values")


if __name__ == "__main__":
    main()
