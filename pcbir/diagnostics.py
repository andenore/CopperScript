"""Source-located, machine-readable diagnostics and the code explanation registry.

ERC works on semantic IR and never parses source. This module maps a finding's
semantic subject back to tokens in the root ``.copper`` file, the same way the
language server does, so command-line output carries ``file:line:column`` and a
machine-applicable fix when ERC offered a ``replacement``.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Iterable

from .erc import Diagnostic, Severity
from .lexer import Lexer, Token, TokenKind
from .syntax import CopperScriptError, SourceLocation

CHECK_SCHEMA = "copperscript.check/v1"

# Declaration keywords preferred when a subject names a declared object.
_DECLARATION_KEYWORDS = ("component", "module", "net", "supply", "interface", "configure", "power_state")


@dataclass(frozen=True, slots=True)
class Span:
    start: SourceLocation
    end_line: int
    end_column: int  # 1-based, exclusive

    def document(self) -> dict[str, object]:
        return {"file": self.start.filename, "line": self.start.line, "column": self.start.column,
                "end_line": self.end_line, "end_column": self.end_column}


class SourceLocator:
    """Maps semantic subjects (``U1``, ``U1.VDD``, ``PWR/U1.VDD``) to source spans.

    Only whole dotted names are matched, so ``U1.CORE`` never matches the prefix
    of ``U1.CORE.VDD``. A finding's ``origin`` (``net NAME`` or ``no_connect``)
    restricts matches to that declaration.
    """

    def __init__(self, source: str, filename: str):
        lexer = Lexer(source, filename)
        self.tokens: tuple[Token, ...] = lexer.tokenize(recover=True)
        self._names: dict[str, list[tuple[int, int]]] = {}
        self._scopes: list[str | None] = [None] * len(self.tokens)
        tokens = self.tokens
        for i, token in enumerate(tokens):
            if token.kind not in (TokenKind.IDENTIFIER, TokenKind.NUMBER):
                continue
            if i > 0 and _joiner(tokens[i - 1]) and i > 1 and _word(tokens[i - 2]):
                continue  # inside a longer dotted name
            name, end = token.text, i
            while end + 2 < len(tokens) and _joiner(tokens[end + 1]) and _word(tokens[end + 2]):
                name += tokens[end + 1].text + tokens[end + 2].text
                end += 2
            self._names.setdefault(name, []).append((i, end))
        self._mark_scopes()

    def _mark_scopes(self) -> None:
        tokens, i = self.tokens, 0
        while i < len(tokens):
            token = tokens[i]
            opens_net = (token.text == "net" and i + 2 < len(tokens) and _word(tokens[i + 1])
                         and tokens[i + 2].text == "{")
            opens_no_connect = (token.text == "no_connect" and i + 1 < len(tokens) and _word(tokens[i + 1]))
            if token.kind is TokenKind.IDENTIFIER and (opens_net or opens_no_connect) and (
                    i == 0 or tokens[i - 1].text in {"{", "}", ";"}):
                scope = f"net {tokens[i + 1].text}" if token.text == "net" and i + 1 < len(tokens) else "no_connect"
                depth, j = 0, i + 1
                while j < len(tokens) and tokens[j].kind is not TokenKind.EOF:
                    text = tokens[j].text if tokens[j].kind is TokenKind.SYMBOL else None
                    if text == "{":
                        depth += 1
                    elif text == "}":
                        depth -= 1
                        if depth <= 0:
                            break
                    elif text == ";" and depth == 0:
                        break
                    j += 1
                for k in range(i, min(j + 1, len(tokens))):
                    self._scopes[k] = scope
                i = j + 1
                continue
            i += 1

    def span(self, subject: str | None, *, code: str = "", origin: str | None = None,
             occurrence: int = 0) -> tuple[Span | None, bool]:
        """Return the span and whether it is the subject text exactly (safe to replace)."""

        if not subject:
            return None, False
        matches, exact = self._names.get(subject, []), True
        if origin and matches:
            scoped = [m for m in matches if self._scopes[m[0]] == origin]
            matches, exact = (scoped, True) if scoped else (matches, False)
        if not matches:
            owner = subject.split("/")[0].split(".")[0]
            matches, exact = self._names.get(owner, []), False
            if not matches:
                return None, False
        if not origin:
            preferred = _preferred_keyword(code)
            declared = [m for m in matches if self._preceded_by(m[0], preferred)] if preferred else []
            if not declared:
                declared = [m for m in matches if self._preceded_by(m[0], _DECLARATION_KEYWORDS)]
            matches = declared or matches
        # A duplicate is reported where it is declared again, not where it first appears.
        index = occurrence + 1 if code.startswith("DUPLICATE_") else occurrence
        start, end = matches[min(index, len(matches) - 1)]
        if index >= len(matches):
            exact = False  # more findings than occurrences: do not reuse a fix range
        first, last = self.tokens[start], self.tokens[end]
        return Span(first.location, last.location.line,
                    last.location.column + (last.end_offset - last.location.offset)), exact

    def _preceded_by(self, index: int, keywords: Iterable[str]) -> bool:
        if index == 0:
            return False
        previous = self.tokens[index - 1]
        return previous.kind is TokenKind.IDENTIFIER and previous.text in keywords


def _joiner(token: Token) -> bool:
    return token.kind is TokenKind.SYMBOL and token.text in {".", "/"}


def _word(token: Token) -> bool:
    return token.kind in (TokenKind.IDENTIFIER, TokenKind.NUMBER)


def _preferred_keyword(code: str) -> tuple[str, ...]:
    if code.startswith(("SUPPLY_", "UNKNOWN_SUPPLY")) or code in {"UNKNOWN_NET", "DUPLICATE_SUPPLY"}:
        return ("supply",)
    if code.startswith("I2C_") or code == "DUPLICATE_INTERFACE":
        return ("interface",)
    if code in {"OUTPUT_CONFLICT", "DUPLICATE_NET"}:
        return ("net",)
    if code.startswith(("MUX_", "PERIPHERAL_", "UNKNOWN_PERIPHERAL", "INVALID_MUX", "MISSING_PERIPHERAL",
                        "UNKNOWN_CONFIGURATION", "PART_HAS_NO_DEVICE", "UNKNOWN_MUX")):
        return ("configure",)
    return ()


def locate(diagnostics: Iterable[Diagnostic], source: str, filename: str) -> list[tuple[Diagnostic, Span | None]]:
    """Attach root-source locations; unresolved subjects keep ``location=None``.

    Repeated findings for the same subject and origin are assigned to successive
    occurrences, so each gets its own range.
    """

    locator = SourceLocator(source, filename)
    seen: dict[tuple[str, str | None, str | None], int] = {}
    result = []
    for diagnostic in diagnostics:
        key = (diagnostic.code, diagnostic.subject, diagnostic.origin)
        occurrence = seen.get(key, 0)
        seen[key] = occurrence + 1
        span, exact = locator.span(diagnostic.subject, code=diagnostic.code, origin=diagnostic.origin,
                                   occurrence=occurrence)
        # Only an exact, unshared subject match can carry a machine-applicable replacement.
        located = replace(diagnostic, location=span.start if span else None,
                          replacement=diagnostic.replacement if exact else None)
        result.append((located, span))
    return result


def diagnostic_document(diagnostic: Diagnostic, span: Span | None) -> dict[str, object]:
    document: dict[str, object] = {
        "severity": diagnostic.severity.value,
        "code": diagnostic.code,
        "message": diagnostic.message,
        "subject": diagnostic.subject,
        "hint": diagnostic.hint,
        "location": span.document() if span else None,
        "fix": None,
    }
    if span is not None and diagnostic.replacement is not None:
        document["fix"] = {"range": span.document(), "replacement": diagnostic.replacement,
                           "description": f"replace {diagnostic.subject!r} with {diagnostic.replacement!r}"}
    return document


def compile_error_document(error: CopperScriptError, *, root: str | None = None, source: str | None = None,
                           display: str | None = None) -> dict[str, object]:
    """A compile error in check-document form; ``root``/``display`` shorten the root file name."""

    location = error.location
    filename = display if display is not None and location.filename == root else location.filename
    end_line, end_column = location.line, location.column + 1
    token = None
    if source is not None and location.filename == root:
        token = next((t for t in Lexer(source, filename).tokenize(recover=True)
                      if t.location.offset == location.offset and t.kind is not TokenKind.EOF), None)
        if token is not None:
            end_column = location.column + (token.end_offset - token.location.offset)
    span = {"file": filename, "line": location.line, "column": location.column,
            "end_line": end_line, "end_column": end_column}
    replacement = getattr(error, "replacement", None)
    return {
        "severity": "error", "code": error.code, "message": error.message, "subject": None,
        "hint": getattr(error, "hint", None), "location": span,
        # Only a replacement whose exact token span is known is machine-applicable.
        "fix": ({"range": span, "replacement": replacement, "description": f"replace with {replacement!r}"}
                if replacement and token is not None else None),
    }


def check_document(board_name: str | None, source: str, documents: list[dict[str, object]]) -> dict[str, object]:
    errors = sum(item["severity"] == "error" for item in documents)
    warnings = sum(item["severity"] == "warning" for item in documents)
    return {"schema": CHECK_SCHEMA, "board": board_name, "source": source, "passed": errors == 0,
            "counts": {"error": errors, "warning": warnings}, "diagnostics": documents}


# --- Explanations -------------------------------------------------------------------------
# Each entry: (summary, typical fix). Kept short: an agent reads one entry at a time.

EXPLANATIONS: dict[str, tuple[str, str]] = {
    "UNKNOWN_COMPONENT": (
        "An endpoint, no_connect or binding names a component reference that is not declared.",
        "Fix the reference spelling, or declare the component. Module internals are reached through "
        "the instance's ports (INST.PORT), not INST/REF."),
    "UNKNOWN_PIN": (
        "The component's part has no pin (or device unit terminal) with this name.",
        "Use the exact pin name from the part definition: `copper query BOARD pins REF` lists them. "
        "Do not infer names from footprint pad numbers."),
    "UNKNOWN_PART": (
        "A component uses a part that no `use library` or import provides.",
        "Check the import alias and exported name with `copper library search` / `inspect`."),
    "UNKNOWN_NET": (
        "A supply or interface refers to a net that is not declared.",
        "Declare the net, or point the supply at an existing net with `net = NAME`."),
    "UNKNOWN_DEVICE": ("A part references a device definition that is not loaded.",
                       "Import the package that defines the device."),
    "UNKNOWN_DEVICE_PAD": ("A package pin bonds to a pad the device does not define.",
                           "Repair the part/device definition in its library."),
    "UNMODELED_PIN": ("The pin has no electrical profile, so its connection cannot be checked.",
                      "Add domains/directions to the pin in the part or device definition."),
    "DUPLICATE_COMPONENT": ("Two components share a reference.", "Rename one of them."),
    "DUPLICATE_NET": ("Two nets share a name.", "Merge their endpoints into one net or rename one."),
    "DUPLICATE_SUPPLY": ("Two supplies share a name.", "Remove or rename one."),
    "DUPLICATE_INTERFACE": ("Two interfaces share a name.", "Rename one."),
    "DUPLICATE_PERIPHERAL_CONFIGURATION": ("Two `configure` declarations share a name.", "Rename one."),
    "PIN_ON_MULTIPLE_NETS": (
        "One pin appears in more than one net, which would short those nets.",
        "Keep the pin in exactly one net; merge the nets if they really are one node."),
    "INTERNAL_PAD_NET_CONFLICT": (
        "Pads that the part connects internally are assigned to different nets.",
        "Put internally joined pads on the same net."),
    "OUTPUT_CONFLICT": ("More than one push-pull output drives the same net.",
                        "Split the net or make all but one driver an input/open-drain."),
    "UNKNOWN_SUPPLY_SOURCE": ("A supply's `source` does not resolve to a known pin.", "Fix the endpoint."),
    "SUPPLY_SOURCE_NOT_OUTPUT": ("A supply's `source` pin cannot output power.",
                                 "Name the regulator or connector pin that actually sources the rail."),
    "SUPPLY_SOURCE_NOT_ON_NET": ("A supply's `source` pin is not on the supply's net.",
                                 "Connect the source pin to the net or correct the source."),
    "UNSOURCED_POWER_INPUT": (
        "A power input is unconnected, or its net has no output, external supply or DC path.",
        "Connect it to a powered net, mark the rail `external = true`, or name a `source`. "
        "A no_connect does not silence this: power inputs need an explicit decision in the part."),
    "SUPPLY_VOLTAGE_HIGH": ("The supply voltage exceeds the pin's operating maximum.",
                            "Use a lower rail, a level shifter, or a part rated for the voltage."),
    "SUPPLY_VOLTAGE_LOW": ("The supply voltage is below the pin's operating minimum.", "Raise the rail or change part."),
    "SUPPLY_VOLTAGE_ABSOLUTE_HIGH": ("The supply exceeds the pin's absolute maximum rating.",
                                     "This can damage the part; change the rail or the part."),
    "SUPPLY_VOLTAGE_ABSOLUTE_LOW": ("The supply is below the pin's absolute minimum rating.",
                                    "Change the rail or the part."),
    "POWER_DOMAIN_VOLTAGE_HIGH": ("A device power domain is supplied above its maximum.", "Change the rail."),
    "POWER_DOMAIN_VOLTAGE_LOW": ("A device power domain is supplied below its minimum.", "Change the rail."),
    "VOLTAGE_LIMIT_UNRESOLVED": ("A pin voltage limit could not be evaluated (often relative to an unknown rail).",
                                 "Connect the reference pin to a declared supply."),
    "I2C_MISSING_SIGNAL": ("An I2C interface does not map SDA or SCL.", "Add `sda = NET; scl = NET;`."),
    "I2C_TOO_FEW_PARTICIPANTS": ("An I2C interface binds fewer than two components.", "Bind the host and each target."),
    "I2C_MISSING_BINDING": ("A bound component has no SDA or SCL pin binding.", "Add the pin to its `bind` block."),
    "I2C_UNKNOWN_PIN": ("An I2C binding names a pin the part does not have.", "Fix the pin name."),
    "I2C_BINDING_NOT_ON_NET": ("A bound pin is not on the mapped SDA/SCL net.", "Add the pin to that net."),
    "I2C_INCOMPATIBLE_PIN": ("A bound pin cannot act as an open-drain I2C line.", "Choose an I2C-capable pin."),
    "I2C_NO_PULLUP_SUPPLY": ("The interface does not name a pull-up supply.", "Add `pullup = RAIL;`."),
    "I2C_UNKNOWN_PULLUP_SUPPLY": ("The named pull-up supply is not declared.", "Declare it or fix the name."),
    "I2C_MISSING_PULLUP": ("SDA or SCL has no resistor to the pull-up rail.",
                           "Add one resistor from each line to the pull-up supply net."),
    "UNKNOWN_CONFIGURATION_COMPONENT": ("A `configure` names an unknown component.", "Fix the reference."),
    "PART_HAS_NO_DEVICE": ("Peripheral configuration needs a part backed by a device definition.",
                           "Use a part whose definition names a device."),
    "UNKNOWN_PERIPHERAL": ("The device has no peripheral with this name.", "Inspect the device's peripherals."),
    "MISSING_PERIPHERAL_SIGNAL": ("A required peripheral signal is not assigned a pin.", "Assign it in `configure`."),
    "UNKNOWN_PERIPHERAL_SIGNAL": ("The peripheral has no signal with this name.", "Fix the signal name."),
    "UNKNOWN_MUX_PIN": ("A configuration selects a physical pin the part does not have.", "Fix the pin name."),
    "INVALID_MUX_OPTION": ("The selected pin cannot carry that peripheral signal.", "Pick a pin with a mux option for it."),
    "MUX_SELECTOR_MISMATCH": ("Resolved mux metadata disagrees with the device definition.", "Repair the definition."),
    "MUX_INCOMPATIBLE_PIN": ("The pin's electrical profile is incompatible with the signal.", "Pick another pin."),
    "PERIPHERAL_CONFLICT": ("One peripheral instance is configured more than once.", "Keep one configuration."),
    "PIN_MUX_CONFLICT": ("One physical pin is selected by several signals.", "Give each signal its own pin."),
    "MUX_RESOURCE_CONFLICT": ("Selections need incompatible settings of a shared resource.", "Choose compatible pins."),
    "DO_NOT_CONNECT": ("The part marks this pin do-not-connect, but it is on a net.", "Remove it from the net."),
    "REQUIRED_PIN_UNCONNECTED": ("The part requires this pin to be connected.", "Connect it as the datasheet requires."),
    "REQUIRED_NET_TRAIT": ("The pin requires net traits (for example a ground) that its net lacks.",
                           "Connect it to an appropriate net."),
    "UNKNOWN_MODE_GROUP": ("A component selects a mode group the device does not define.", "Fix the name."),
    "MODE_NOT_SELECTED": ("A device mode group has neither a default nor a selection.", "Select a mode."),
    "UNKNOWN_MODE_CHOICE": ("The selected mode is not a choice of that group.", "Use one of the declared choices."),
    "INCOMPLETE_DIFFERENTIAL_PAIR": ("Only one polarity of a differential pair is connected.", "Connect both or neither."),
    "NO_CONNECT_CONNECTED": (
        "A pin is declared `no_connect` but also appears in a net.",
        "Remove the pin from the net, or delete the no_connect if the connection is intended."),
    "NO_CONNECT_REQUIRED_PIN": (
        "A pin the part requires to be connected is declared `no_connect`.",
        "Connect the pin as the datasheet requires and remove the no_connect."),
    "DUPLICATE_NO_CONNECT": ("The same pin is declared `no_connect` more than once.", "Remove the duplicate."),
    "NO_CONNECT_MODULE_PORT": (
        "A `no_connect` names a module instance port; it applies only to component pins.",
        "Connect the port in the parent (every port must be connected), or declare the spare pin "
        "`no_connect` inside the module source."),
    "UNMARKED_PIN": (
        "Ordinary pins are neither on a net nor declared `no_connect`, so an omission cannot be told "
        "apart from a decision. Power inputs, required, optional and do-not-connect pins and pins "
        "without an electrical profile are excluded because they are reported or decided elsewhere. "
        "Components inside module instances are left to the module's own source.",
        "Connect each pin, or declare it: `no_connect U1.PA0, U1.PA1 { reason = \"unused GPIO\"; }`. "
        "`copper query BOARD unconnected` lists them; `copper check --pins error` makes this blocking."),
}

_FAMILIES = {
    "PAR": "Syntax error from the parser. The location points at the offending token.",
    "LEX": "Lexical error (unterminated string, invalid character). The location points at it.",
    "CMP": "Compile/lowering error: the source parsed but a declaration is invalid. Read the message.",
    "MEC": "Mechanical declaration error.",
    "SLK": "Silkscreen declaration error.",
    "MAC": "Physical macro definition or binding error.",
}


def explain(code: str) -> dict[str, object] | None:
    code = code.strip().upper()
    if code in EXPLANATIONS:
        summary, fix = EXPLANATIONS[code]
        return {"code": code, "summary": summary, "fix": fix}
    family = code[:3]
    if family in _FAMILIES and code[3:].isdigit():
        return {"code": code, "summary": _FAMILIES[family],
                "fix": "Use the message and hint; `copper check --json` gives the exact location."}
    return None


def suggest_code(code: str) -> str | None:
    from difflib import get_close_matches
    match = get_close_matches(code.strip().upper(), list(EXPLANATIONS), n=1, cutoff=0.6)
    return match[0] if match else None


def severity_counts(diagnostics: Iterable[Diagnostic]) -> tuple[int, int]:
    items = list(diagnostics)
    return (sum(d.severity is Severity.ERROR for d in items), sum(d.severity is Severity.WARNING for d in items))
