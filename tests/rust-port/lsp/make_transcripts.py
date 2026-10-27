"""Write the LSP session transcripts the comparison harness replays.

    python tests/rust-port/lsp/make_transcripts.py

Each transcript is JSON lines (see scripts/compare_implementations.py):
messages to ``send``, ``raw`` bytes, and ``wait`` counts of complete server
messages, so debounced diagnostics publish at fixed points. Positions are
computed here from the sources; quick-fix requests carry the diagnostics the
Python server publishes for the same text. ``lifecycle.jsonl`` and
``board.jsonl`` are written by hand.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
OUT = Path(__file__).resolve().parent
SOURCE = '''board Demo {
    use library "tiny";
    component R1: RESISTOR { value = 4.7kohm; }
    net N { R1.1; }
}'''
UNSAVED = "{root_uri}/build/lsp-unsaved.copper"


def position(text, needle, advance=0, nth=0):
    offset = -1
    for _ in range(nth + 1):
        offset = text.index(needle, offset + 1)
    offset += advance
    line = text.count("\n", 0, offset)
    start = text.rfind("\n", 0, offset) + 1
    return {"line": line, "character": len(text[start:offset].encode("utf-16-le")) // 2}


def published(text):
    """The diagnostics the Python server publishes for an unsaved import-free `text`."""
    from pcbir.lsp.analysis import Workspace
    uri = (ROOT / "build" / "lsp-unsaved.copper").as_uri()
    workspace = Workspace()
    workspace.update(uri, 1, text)
    return workspace.analysis(uri).diagnostics()


class Session:
    def __init__(self, comment):
        self.lines = [f"# {line}" for line in comment.splitlines()]
        self.expected = 0
        self.next_id = 0

    def send(self, message):
        self.lines.append(json.dumps({"send": message}, ensure_ascii=False))

    def request(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        self.expected += 1

    def notify(self, method, params, replies=0):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})
        self.expected += replies

    def wait(self, published=1):
        self.expected += published
        self.lines.append(json.dumps({"wait": self.expected}))

    def raw(self, text):
        self.lines.append(json.dumps({"raw": text}, ensure_ascii=False))

    def start(self):
        self.request("initialize", {"capabilities": {}, "processId": None, "rootUri": "{root_uri}"})
        self.notify("initialized", {})

    def stop(self):
        self.request("shutdown")
        self.notify("exit", {})

    def open(self, uri, text, version=1, published=1):
        self.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "copper",
                                                              "version": version, "text": text}})
        if published:
            self.wait(published)

    def change(self, uri, version, changes, published=1, replies=0):
        self.notify("textDocument/didChange", {"textDocument": {"uri": uri, "version": version},
                                               "contentChanges": changes}, replies)
        if published:
            self.wait(published)

    def at(self, method, uri, where):
        self.request(f"textDocument/{method}", {"textDocument": {"uri": uri}, "position": where})

    def symbols(self, uri):
        self.request("textDocument/documentSymbol", {"textDocument": {"uri": uri}})

    def actions(self, uri, diagnostics):
        self.request("textDocument/codeAction", {
            "textDocument": {"uri": uri},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "context": {"diagnostics": diagnostics}})

    def write(self, name):
        (OUT / name).write_text("\n".join(self.lines) + "\n", encoding="utf-8")


def unsaved():
    s = Session("An unsaved board: symbols, hovers, definitions and completions while it is edited.")
    s.start()
    s.open(UNSAVED, SOURCE)
    s.symbols(UNSAVED)
    for needle, advance in (("R1.1", 0), ("R1.1", 3), ("N {", 0), ("tiny", 1), ("board", 2), ("Demo", 1),
                            ("4.7kohm", 2), ("RESISTOR", 3), ("use", 1)):
        s.at("hover", UNSAVED, position(SOURCE, needle, advance))
        s.at("definition", UNSAVED, position(SOURCE, needle, advance))
    for needle, advance in (("R1.1", 3), ("tiny", 2), ("    net", 0), ("component", 4), ("value", 2),
                            ("{ R1", 1), ("}", 1)):
        s.at("completion", UNSAVED, position(SOURCE, needle, advance))
    broken = SOURCE.replace("R1.1;", "R1.;").replace("value =", "val =")
    s.change(UNSAVED, 2, [{"text": broken}])
    s.at("completion", UNSAVED, position(broken, "R1.;", 3))
    s.at("completion", UNSAVED, position(broken, "val =", 3))
    s.at("hover", UNSAVED, position(broken, "R1.;", 1))
    commented = broken.replace("net N", "// comment R1.\n    # note R1\n    net N")
    s.change(UNSAVED, 3, [{"text": commented}])
    s.at("completion", UNSAVED, position(commented, "comment R1.", 11))
    s.at("completion", UNSAVED, position(commented, "note R1", 7))
    s.at("hover", UNSAVED, position(commented, "N {", 0))
    nested = commented + "\n"
    nested = nested.replace("component R1", "silkscreen {\n        text T { \n    }\n    component R1")
    s.change(UNSAVED, 4, [{"text": nested}])
    s.at("completion", UNSAVED, position(nested, "text T { ", 9))
    s.at("completion", UNSAVED, position(nested, "silkscreen {", 12))
    s.symbols(UNSAVED)
    second = "{root_uri}/build/lsp-second.copper"
    other = SOURCE.replace("Demo", "Second").replace("R1", "R9")
    s.notify("textDocument/didOpen", {"textDocument": {"uri": second, "languageId": "copper", "version": 7,
                                                       "text": other}})
    s.wait(2)  # One debounced run publishes both open documents.
    s.notify("textDocument/didSave", {"textDocument": {"uri": UNSAVED}})
    s.wait(2)
    s.notify("workspace/didChangeWatchedFiles", {"changes": []})
    s.wait(2)
    s.notify("textDocument/didClose", {"textDocument": {"uri": second}}, replies=1)
    s.wait(1)
    s.symbols(second)
    s.notify("textDocument/didClose", {"textDocument": {"uri": UNSAVED}}, replies=1)
    s.stop()
    s.write("unsaved.jsonl")


def edits():
    s = Session("Unicode and CRLF text, incremental edits, stale versions and every rejected update.")
    s.start()
    text = "// 😀 ü\r\n" + SOURCE.replace("\n", "\r\n")
    s.open(UNSAVED, text)
    s.at("hover", UNSAVED, position(text, "R1.1", 1))
    s.at("hover", UNSAVED, {"line": 0, "character": 5})
    s.at("hover", UNSAVED, {"line": 0, "character": 4})
    s.at("hover", UNSAVED, {"line": 0, "character": 99})
    s.at("hover", UNSAVED, {"line": 99, "character": 0})
    s.at("hover", UNSAVED, {"line": -1, "character": 0})
    s.at("hover", UNSAVED, {"line": 0, "character": -1})
    s.at("hover", UNSAVED, {"line": 1.0, "character": 0})
    s.at("hover", UNSAVED, {"line": True, "character": 0})
    s.at("hover", UNSAVED, {"line": 0})
    s.at("hover", UNSAVED, [0, 0])
    s.at("hover", UNSAVED, "0:0")
    s.at("completion", UNSAVED, None)
    s.request("textDocument/hover", {"textDocument": {"uri": UNSAVED}})
    start, end = position(text, "4.7"), position(text, "4.7", 3)
    s.change(UNSAVED, 2, [{"range": {"start": start, "end": end}, "text": "10"},
                          {"range": {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 7}},
                           "text": "x"}])
    s.at("hover", UNSAVED, position(text, "R1:", 0))
    s.symbols(UNSAVED)
    s.change(UNSAVED, 2, [{"text": "stale"}], published=0)
    s.change(UNSAVED, 1, [{"text": "older"}], published=0)
    bad = [
        [{"range": {"start": {"line": 999, "character": 0}, "end": {"line": 999, "character": 0}}, "text": ""}],
        [{"range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 4}}, "text": "x"}],
        [{"range": {"start": {"line": 1, "character": 3}, "end": {"line": 1, "character": 1}}, "text": "x"}],
        [{"range": {"start": {"line": 1, "character": 0}}, "text": "x"}],
        [{"range": None, "text": "x"}],
        [{"text": 5}],
        [{}],
        ["text"],
        {"text": "x"},
        "text",
        5,
        True,
    ]
    for version, changes in enumerate(bad, start=3):
        s.change(UNSAVED, version, changes, published=0, replies=1)
    s.notify("textDocument/didChange", {"textDocument": {"uri": UNSAVED, "version": 20}, "contentChanges": None},
             replies=1)
    s.notify("textDocument/didChange", {"textDocument": {"uri": UNSAVED, "version": 21}}, replies=1)
    s.notify("textDocument/didChange", {"textDocument": {"uri": UNSAVED}, "contentChanges": []}, replies=1)
    s.at("hover", UNSAVED, position(text, "R1.1", 1))
    s.change(UNSAVED, 30, [{"text": SOURCE}, {"range": {"start": {"line": 4, "character": 1},
                                                        "end": {"line": 4, "character": 1}}, "text": "\n"}])
    s.symbols(UNSAVED)
    never = "{root_uri}/build/lsp-never-opened.copper"
    s.change(never, 1, [{"text": SOURCE}], published=0, replies=1)
    # (uri, version, text, error replies, documents then published).
    opens = [("{root_uri}/build/notes.txt", 1, SOURCE, 1, 0), ("{root_uri}/build/upper.COPPER", 1, SOURCE, 0, 2),
             (UNSAVED, 1.5, SOURCE, 1, 0), (UNSAVED, True, SOURCE, 1, 0), (UNSAVED, "40", SOURCE, 1, 0),
             (UNSAVED, 41, 7, 1, 0), (UNSAVED, 42, None, 1, 0), ("file:///", 1, SOURCE, 1, 0),
             ("file:relative.copper", 1, SOURCE, 0, 3), ("untitled:Untitled-1", 1, SOURCE, 1, 0),
             ("{root_uri}/build/a%20b.copper?x=1", 1, SOURCE, 1, 0),
             ("{root_uri}/build/a%20b.copper#frag", 1, SOURCE, 1, 0)]
    for uri, version, body, replies, published in opens:
        s.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "version": version, "text": body}}, replies)
        if published:
            s.wait(published)
    s.notify("textDocument/didOpen", {"textDocument": {"uri": UNSAVED, "version": 50}}, replies=1)
    s.notify("textDocument/didOpen", {"textDocument": {"version": 50, "text": SOURCE}}, replies=1)
    encoded = "{root_uri}/build/lsp%20encoded%2Bname.copper"
    s.open(encoded, SOURCE, published=4)
    s.at("definition", encoded, position(SOURCE, "R1.1", 1))
    s.notify("textDocument/didClose", {"textDocument": {"uri": encoded}}, replies=1)
    s.wait(3)
    s.notify("textDocument/didClose", {"textDocument": {"uri": never}}, replies=1)
    s.wait(3)
    s.notify("textDocument/didClose", {"textDocument": {}}, replies=1)
    s.notify("textDocument/didClose", {"textDocument": {"uri": "http://x/y.copper"}}, replies=1)
    s.stop()
    s.write("edits.jsonl")


def quickfixes():
    s = Session("Located diagnostics, quick fixes and their withholding when stale or unparseable.")
    s.start()
    typo = SOURCE.replace("net N { R1.1; }", "net N { R1.1; R1.22; }")
    s.open(UNSAVED, typo)
    findings = published(typo)
    s.actions(UNSAVED, findings)
    s.actions(UNSAVED, findings[:1])
    unknown = next(d for d in findings if d["code"] == "UNKNOWN_PIN")
    s.actions(UNSAVED, [{**unknown, "data": {**unknown["data"], "fix": {**unknown["data"]["fix"], "title": ""}}}])
    s.actions(UNSAVED, [{**unknown, "data": {**unknown["data"], "fix": {**unknown["data"]["fix"], "title": 7}}}])
    s.actions(UNSAVED, [{"data": {"fix": {"newText": "x"}}}])
    s.actions(UNSAVED, [{"data": {"fix": {"newText": "x", "range": None}}}])
    s.actions(UNSAVED, [{"data": {"fix": {"newText": "x", "range": "r", "oldText": None}}}])
    s.actions(UNSAVED, [{"data": {"fix": {"newText": 5, "range": unknown["range"], "oldText": "R1.22"}}}])
    s.actions(UNSAVED, [{"data": {"no_connect": ["R1.1A", "R1.2"]}}])
    s.actions(UNSAVED, [{"data": {"no_connect": []}}, {"data": {"no_connect": "R1.2"}}, {"data": [1]},
                        {"data": None}, 3, None, [1]])
    s.actions(UNSAVED, {"a": 1})
    s.actions(UNSAVED, "diagnostics")
    s.actions(UNSAVED, 5)
    s.actions(UNSAVED, [])
    s.request("textDocument/codeAction", {"textDocument": {"uri": UNSAVED}, "context": None})
    s.request("textDocument/codeAction", {"textDocument": {"uri": UNSAVED}, "context": []})
    s.request("textDocument/codeAction", {"textDocument": {"uri": UNSAVED}, "context": ["x"]})
    s.request("textDocument/codeAction", {"textDocument": {"uri": UNSAVED}, "context": 3})
    s.request("textDocument/codeAction", {"textDocument": {"uri": UNSAVED}})
    edited = typo.replace("R1.22", "R1.99")
    s.change(UNSAVED, 2, [{"text": edited}])
    s.actions(UNSAVED, findings)
    duplicate = SOURCE.replace("net N { R1.1; }", "net N { R1.1; }\n    net N { R1.2; }")
    s.change(UNSAVED, 3, [{"text": duplicate}])
    syntax = SOURCE.replace("component R1", "componnet R1")
    s.change(UNSAVED, 4, [{"text": syntax}])
    s.actions(UNSAVED, published(syntax))
    crlf = typo.replace("\n", "\r\n")
    s.change(UNSAVED, 5, [{"text": crlf}])
    s.actions(UNSAVED, published(crlf))
    oneline = 'board D { use library "tiny"; component R1: RESISTOR { value = 100ohm; } net N { R1.1; R1.22; } }'
    s.change(UNSAVED, 6, [{"text": oneline}])
    s.actions(UNSAVED, published(oneline))
    tabs = typo.replace("    ", "\t")
    s.change(UNSAVED, 7, [{"text": tabs}])
    s.actions(UNSAVED, published(tabs))
    for broken in ("board D { @ net N {} }", "board D { component R: X;", "not a root",
                   'board D { value = "unterminated', "", "board", "board B", "board B {",
                   "board D {\n component A: X { value = 1;\n component B: X;\n}",
                   "board D { } extra", "part P { pin A { number = ; } }", "module M { port P: passive; }",
                   'board D { use library "missing"; component R1: RESISTOR; }',
                   'board D { use library "tiny"; component R1: NOPE; net N { R1.1; } }',
                   'board D { use library "tiny"; component R1: RESISTOR; net N { R1.1; } net N { R1.2; } '
                   'supply N { voltage = 3V; } }'):
        s.change(UNSAVED, s.next_id + 100, [{"text": broken}])
        s.symbols(UNSAVED)
        s.at("completion", UNSAVED, {"line": 0, "character": 0})
    s.stop()
    s.write("quickfix.jsonl")


def packages():
    board = "examples/hierarchical_board/board.copper"
    module = "examples/packages/power/buck_5v_to_3v3.copper"
    uri, module_uri = "{root_uri}/" + board, "{root_uri}/" + module
    text = (ROOT / board).read_text(encoding="utf-8")
    module_text = (ROOT / module).read_text(encoding="utf-8")
    s = Session("Imported packages: navigation into package sources, members and unsaved package buffers.")
    s.start()
    s.open(uri, text)
    s.symbols(uri)
    for needle, advance in (("power.Buck5VTo3V3", 7), ("power.Buck5VTo3V3", 2), ("stm32.STM32G0B1CBT6", 8),
                            ("sensors.BME280", 9), ("PWR.VIN", 1), ("PWR.VIN", 5), ("U1.PB7", 4), ("U2.SDA", 4),
                            ("U1.VDD", 3), ("PS1.OUT", 5), ("I2C_SDA", 2), ("SENSOR_BUS", 3), ("PB7;", 1),
                            ('"github.com/copperscript/examples/power"', 3), ("import power", 2),
                            ("power ", 1)):
        s.at("hover", uri, position(text, needle, advance))
        s.at("definition", uri, position(text, needle, advance))
    member = text.replace("        PWR.VIN;", "        PWR.;\n        U1.;\n        power.;")
    s.change(uri, 2, [{"text": member}])
    s.at("completion", uri, position(member, "PWR.;", 4))
    s.at("completion", uri, position(member, "U1.;", 3))
    s.at("completion", uri, position(member, "power.;", 6))
    s.at("completion", uri, position(member, "PWR.;", 2))
    s.at("hover", uri, position(member, "PWR.;", 1))
    s.change(uri, 3, [{"text": text}])
    edited = module_text.replace("port VOUT: power_out;", "port VOUT: power_out;\n    port EXTRA: passive;")
    s.open(module_uri, edited, published=2)
    s.at("completion", uri, position(member, "PWR.;", 4))
    s.at("hover", module_uri, position(edited, "U1.VIN", 4))
    s.at("completion", module_uri, position(edited, "port.VIN", 5))
    s.symbols(module_uri)
    broken = module_text.replace("value = 10uH;", "value = ;")
    s.change(module_uri, 2, [{"text": broken}], published=2)
    cyclic = 'module Loop { import again "github.com/copperscript/examples/power"; }'
    s.change(module_uri, 3, [{"text": cyclic}], published=2)
    s.notify("textDocument/didClose", {"textDocument": {"uri": module_uri}}, replies=1)
    s.wait(1)
    s.at("hover", uri, position(text, "power.Buck5VTo3V3", 7))
    s.at("hover", module_uri, position(module_text, "U1.VIN", 4))
    s.at("definition", module_uri, position(module_text, "U1.VIN", 4))
    s.at("completion", module_uri, position(module_text, "U1.VIN", 3))
    missing = text.replace('import power "github.com/copperscript/examples/power";',
                           'import power "github.com/copperscript/examples/powr";\n'
                           '    import other "github.com/unknown/module/x";\n    import power "./missing";')
    s.change(uri, 4, [{"text": missing}])
    s.stop()
    s.write("packages.jsonl")


def physical_macros():
    board = "examples/macro_definition/board.copper"
    layout = "examples/macro_definition/layouts/link.copper"
    uri, layout_uri = "{root_uri}/" + board, "{root_uri}/" + layout
    text = (ROOT / board).read_text(encoding="utf-8")
    layout_text = (ROOT / layout).read_text(encoding="utf-8")
    s = Session("Physical macros: contract roles, scoped targets, macro sources and their completions.")
    s.start()
    s.open(uri, text)
    s.symbols(uri)
    for needle, advance in (("layouts.TestLayout", 9), ("member chip", 8), ("= U1", 3), ("= SIGNAL", 4),
                            ("LINK", 1), ("parts.TestLand", 7)):
        s.at("hover", uri, position(text, needle, advance))
        s.at("definition", uri, position(text, needle, advance))
    blank = text.replace("        net signal = SIGNAL;", "        net signal = SIGNAL;\n        member \n        net ")
    s.change(uri, 2, [{"text": blank}])
    s.at("completion", uri, position(blank, "member \n", 7))
    s.at("completion", uri, position(blank, "net \n", 4))
    s.at("completion", uri, position(blank, "= U1", 2))
    s.at("completion", uri, position(blank, "= SIGNAL", 2))
    s.at("completion", uri, position(blank, "physical_macro", 0))
    s.open(layout_uri, layout_text, published=2)
    s.symbols(layout_uri)
    lines = layout_text.split("\n")
    for line in range(len(lines)):
        s.at("completion", layout_uri, {"line": line, "character": len(lines[line].encode("utf-16-le")) // 2})
        s.at("hover", layout_uri, {"line": line, "character": 6})
    s.change(layout_uri, 2, [{"text": layout_text.replace("source", "sourc", 1)}], published=2)
    s.notify("textDocument/didClose", {"textDocument": {"uri": layout_uri}}, replies=1)
    s.wait(1)
    vertical = "examples/full_vertical/board.copper"
    vertical_uri = "{root_uri}/" + vertical
    vertical_text = (ROOT / vertical).read_text(encoding="utf-8")
    s.open(vertical_uri, vertical_text, published=2)
    for needle, advance in (("member C_AVIN = C_MODEM_AVIN", 18), ("net EN = port.MODEM_EN", 12),
                            ("net FB = MODEM_FB", 11), ("member ANT = ANT_BT", 15), ("member C_AVIN", 9)):
        s.at("hover", vertical_uri, position(vertical_text, needle, advance))
        s.at("definition", vertical_uri, position(vertical_text, needle, advance))
    scoped = vertical_text.replace("        net VOUT = port.V3V8;", "        net VOUT = port.V3V8;\n"
                                   "        member U = \n        net VIN = \n        member \n        net ")
    s.change(vertical_uri, 2, [{"text": scoped}], published=2)
    for needle, advance in (("member U = \n", 11), ("net VIN = \n", 10), ("member \n", 7), ("net \n", 4)):
        s.at("completion", vertical_uri, position(scoped, needle, advance))
    s.stop()
    s.write("macros.jsonl")


def framing():
    """Transport failures end the server; each runs alone."""
    init = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    cases = {
        "frame-negative-length": "Content-Length: -1\r\n\r\n",
        "frame-oversized-length": "Content-Length: 999999999\r\n\r\n",
        "frame-truncated-payload": "Content-Length: 5\r\n\r\n{}",
        "frame-duplicate-header": "Content-Length: 1\r\nContent-Length: 1\r\n\r\n0",
        "frame-missing-length": "Other: yes\r\n\r\n",
        "frame-invalid-json": "Content-Length: 1\r\n\r\nx",
        "frame-invalid-header": "no colon here\r\n\r\n",
        "frame-bare-newline": "Content-Length: 2\n\r\n{}",
        "frame-oversized-header": "X-Long: " + "a" * 9000 + "\r\n\r\n",
        "frame-truncated-header": "Content-Length: 2\r\n",
        "frame-zero-length": "Content-Length: 0\r\n\r\n",
        "frame-non-ascii-header": "Contént-Length: 2\r\n\r\n{}",
        "frame-bom-payload": "Content-Length: 5\r\n\r\n﻿{}",
        "frame-nan-payload": 'Content-Length: 46\r\n\r\n{"jsonrpc":"2.0","id":NaN,"method":"shutdown"}'
                             'Content-Length: 48\r\n\r\n{"jsonrpc":"2.0","id":1e999,"method":"shutdown"}',
        "frame-headers-mixed": "content-type: application/vscode-jsonrpc; charset=utf-8\r\n"
                               f"CONTENT-LENGTH:  {len(init)} \r\n\r\n{init}",
        "frame-length-underscore": f"Content-Length: +{len(init)}\r\n\r\n{init}"
                                   f"Content-Length: 0_{len(init)}\r\n\r\n{init}",
    }
    for name, raw in cases.items():
        s = Session(f"Transport: {name.removeprefix('frame-').replace('-', ' ')}.")
        s.raw(raw)
        s.write(f"{name}.jsonl")
    s = Session("End of input without shutdown: exit status 1, pending diagnostics never publish.")
    s.start()
    s.open(UNSAVED, SOURCE, published=0)
    s.write("eof-without-shutdown.jsonl")
    s = Session("Exit without shutdown, then ignored input.")
    s.start()
    s.notify("exit", {})
    s.request("shutdown")
    s.write("exit-without-shutdown.jsonl")
    s = Session("Shutdown then end of input: exit status 0.")
    s.start()
    s.request("shutdown")
    s.write("shutdown-then-eof.jsonl")


if __name__ == "__main__":
    unsaved()
    edits()
    quickfixes()
    packages()
    physical_macros()
    framing()
