"""Bounded JSON-lines document host. No source writes or arbitrary client paths.

VS Code owns source versions, edits, undo and saving. The trusted launcher fixes
the source path/physical settings; requests contain only that document's buffer
and the shared editor operation protocol. No network, shell or HTTP listener.
"""
from __future__ import annotations

import argparse
from hashlib import sha256
import json
from pathlib import Path
import sys

from ..syntax import CopperScriptError
from .project import document_session
from .source import SourceEditError, SourceSnapshot, declaration_index

MAX_TEXT = 4 * 1024 * 1024


def compiler_fingerprint():
    """Content identity, independent of checkout location and wheel timestamp."""
    package = Path(__file__).resolve().parents[1]
    files = [(str(p.relative_to(package)).replace("\\", "/"), p.read_bytes())
             for p in package.rglob("*") if p.is_file() and p.suffix in {".py", ".html", ".js", ".css"}]
    import copperscript
    library = Path(copperscript.__file__).parent
    files += [("copperscript/" + p.name, p.read_bytes()) for p in library.glob("*.py")]
    digest = sha256()
    for name, raw in sorted(files):
        digest.update(name.encode() + b"\0" + sha256(raw).digest())
    return digest.hexdigest()


def text_edits(before, after):
    """One minimal changed span, with UTF-16 offsets for VS Code positions.

    Exact reviewed bytes are already token-patched. A single enclosing span
    keeps this one native undo step; no unchanged prefix/suffix is replaced.
    """
    start = 0
    while start < min(len(before), len(after)) and before[start] == after[start]:
        start += 1
    end_before, end_after = len(before), len(after)
    while end_before > start and end_after > start and before[end_before - 1] == after[end_after - 1]:
        end_before -= 1; end_after -= 1
    utf16 = lambda text: len(text.encode("utf-16-le")) // 2
    return [{"start": utf16(before[:start]), "end": utf16(before[:end_before]), "text": after[start:end_after]}]


class DocumentHost:
    def __init__(self, source, **settings):
        self.source = Path(source).resolve()
        self.settings = settings
        self.text = ""
        self.version = -1
        self.session = None
        self.error = None

    def close(self):
        if self.session:
            self.session.close()

    def open(self, version, text):
        if type(version) is not int or version < 0 or not isinstance(text, str) or len(text.encode()) > MAX_TEXT:
            raise ValueError("bounded text and a nonnegative integer document version required")
        if version <= self.version:
            raise ValueError("document version must advance monotonically")
        self.close()
        self.version, self.text, self.session = version, text, None
        try:
            self.session = document_session(self.source, lambda: self.text.encode("utf-8"), **self.settings)
            self.error = None
        except (ValueError, OSError, CopperScriptError) as exc:
            self.error = str(exc)
            raise
        return {"scene": self.scene()}

    def scene(self):
        if self.session is None:
            raise ValueError(self.error or "open the source document first")
        result = self.session.scene()
        result["document_version"] = self.version
        result["document_host"] = True
        result["can_source_undo"] = False
        result["can_source_redo"] = False
        result["notice"] = "Reviewed source edits use VS Code document undo and Save. Temporary placement is not persisted."
        from ..parser import parse
        from ..syntax import ComponentDecl, ModuleInstanceDecl
        document = parse(self.text, str(self.source))
        spans = [node.span for node in declaration_index(SourceSnapshot(self.text.encode())).children]
        locations = {d.ref: d.location for d in document.declarations if isinstance(d, ComponentDecl)}
        locations.update({d.ref: d.location for d in document.declarations if isinstance(d, ModuleInstanceDecl)})
        for component in result["components"]:
            location = locations.get(component["reference"].split("/")[0])
            if location:
                # Parser component locations identify the reference token,
                # while the declaration index includes its leading keyword.
                span = next(s for s in spans if s.start <= location.offset < s.end)
                offset = lambda i: len(self.text[:i].encode("utf-16-le")) // 2
                component["source_link"] = {"line": location.line, "column": location.column,
                                           "start": offset(span.start), "end": offset(span.end)}
        return result

    def request(self, request):
        if not isinstance(request, dict):
            raise ValueError("request must be an object")
        method = request.get("method")
        if method == "hello" and set(request) == {"method"}:
            return {"compiler_digest": compiler_fingerprint(), "protocol": "copperscript-editor-document/v0.1"}
        if method == "open" and set(request) == {"method", "version", "text"}:
            return self.open(request["version"], request["text"])
        if method == "scene" and set(request) == {"method"}:
            return {"scene": self.scene()}
        if method != "operation" or set(request) != {"method", "version", "operation"}:
            raise ValueError("unknown document host request")
        if request["version"] != self.version or type(request["version"]) is not int:
            raise ValueError("document changed; reload the current buffer")
        if self.session is None:
            raise ValueError(self.error or "source is invalid")
        operation = request["operation"]
        if not isinstance(operation, dict):
            raise ValueError("operation must be an object")
        action = operation.get("action")
        if action == "save_source":
            if set(operation) != {"action", "revision", "review_id"}:
                raise ValueError("unexpected source commit fields")
            self.session._check(operation["revision"])
            review = self.session.source_pending
            if review is None or operation["review_id"] != review.id:
                raise ValueError("review changed; inspect the current source diff")
            _, board = self.session.workspace.validate(review.after.raw, review.board, self.session.options)
            if board != review.board:
                raise ValueError("physical inputs changed after review")
            return {"document_edit": {"version": self.version, "source_revision": review.before.revision,
                                      "edits": text_edits(self.text, review.after.text)}}
        if action in {"undo_source", "redo_source", "reload_source"}:
            raise ValueError("use native VS Code document undo/redo/reload")
        result = self.session.operation(operation)
        if "scene" in result:
            result["scene"] = self.scene()
        return result


def serve(source, **settings):
    host = DocumentHost(source, **settings)
    try:
        while True:
            raw = sys.stdin.buffer.readline(MAX_TEXT * 3 + 1)
            if not raw:
                break
            identifier = None
            try:
                if len(raw) > MAX_TEXT * 3 or not raw.endswith(b"\n"):
                    raise ValueError("bounded JSON-line request required")
                request = json.loads(raw)
                if not isinstance(request, dict) or set(request) != {"id", "request"} or type(request["id"]) is not int:
                    raise ValueError("integer id and request required")
                identifier = request["id"]
                response = {"id": identifier, "result": host.request(request["request"])}
            except (ValueError, OSError, CopperScriptError) as exc:
                response = {"id": identifier, "error": str(exc)}
            print(json.dumps(response, allow_nan=False), flush=True)
    finally:
        host.close()


if __name__ == "__main__":
    if sys.argv[1:] == ["--fingerprint"]:
        print(compiler_fingerprint())
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("source", type=Path)
        parser.add_argument("--footprint-root", action="append", default=[], type=Path)
        parser.add_argument("--layers", type=int, choices=(2, 4, 6), default=2)
        parser.add_argument("--fab-profile", default="generic", choices=("generic", "jlcpcb-four-layer", "jlcpcb-six-layer"))
        parser.add_argument("--placement-templates", type=Path)
        parser.add_argument("--hard-macro", action="append", default=[], type=Path)
        parser.add_argument("--allow-proxy-footprints", action="store_true")
        args = parser.parse_args()
        serve(args.source, footprint_roots=args.footprint_root, layers=args.layers, fab_profile=args.fab_profile,
              templates=args.placement_templates, macros=args.hard_macro, allow_proxy=args.allow_proxy_footprints)
