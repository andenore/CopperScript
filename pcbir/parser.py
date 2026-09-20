"""Recursive-descent parser for CopperScript v0.1."""

from __future__ import annotations

from .lexer import Token, TokenKind, tokenize
from .syntax import (
    BindingDecl,
    ComponentDecl,
    ConstraintDecl,
    CopperScriptError,
    Declaration,
    Document,
    DevicePropertyDecl,
    ConfigurationDecl,
    ImportDecl,
    InterfaceDecl,
    ModuleInstanceDecl,
    MuxDecl,
    NetDecl,
    PartPropertyDecl,
    PadDecl,
    PeripheralDecl,
    PeripheralSignalDecl,
    PowerDomainDecl,
    PowerStateDecl,
    PinDecl,
    PortDecl,
    RawQuantity,
    ResourceDecl,
    Scalar,
    SupplyDecl,
)


class Parser:
    def __init__(self, tokens: tuple[Token, ...]):
        self.tokens = tokens
        self.index = 0

    @property
    def current(self) -> Token:
        return self.tokens[self.index]

    def parse(self) -> Document:
        root = self._expect(TokenKind.IDENTIFIER, "'board', 'module', 'part', or 'device'")
        if root.text not in {"board", "module", "part", "device"}:
            self._error(
                "PAR008",
                f"expected 'board', 'module', 'part', or 'device', found {root.text!r}",
                root,
            )
        start = root.location
        name = self._name(f"{root.text} name")
        self._expect_symbol("{")
        libraries: list[str] = []
        imports: list[ImportDecl] = []
        declarations: list[Declaration] = []
        while not self._at_symbol("}"):
            if self.current.kind is TokenKind.EOF:
                self._error("PAR001", "expected '}' before end of file")
            keyword = self._name("declaration")
            if keyword == "import":
                import_location = self.tokens[self.index - 1].location
                alias = self._name("import alias")
                import_path = self._expect(TokenKind.STRING, "package import path").text
                imports.append(ImportDecl(import_location, alias, import_path))
                self._expect_symbol(";")
            elif keyword == "use":
                import_kind = self._name("'library'")
                import_name = self._expect(TokenKind.STRING, f"{import_kind} name").text
                if import_kind == "library":
                    libraries.append(import_name)
                else:
                    self._error(
                        "PAR011",
                        f"expected 'library', found {import_kind!r}",
                        self.tokens[self.index - 2],
                    )
                self._expect_symbol(";")
            elif root.text == "part" and keyword == "pin":
                declarations.append(self._pin())
            elif root.text == "part":
                property_location = self.tokens[self.index - 1].location
                self._expect_symbol("=")
                declarations.append(
                    PartPropertyDecl(property_location, keyword, self._scalar())
                )
                self._expect_symbol(";")
            elif root.text == "device" and keyword == "peripheral":
                declarations.append(self._peripheral())
            elif root.text == "device" and keyword == "pad":
                declarations.append(self._pad())
            elif root.text == "device" and keyword == "power_domain":
                declarations.append(self._power_domain())
            elif root.text == "device" and keyword == "mux":
                declarations.append(self._mux())
            elif root.text == "device" and keyword == "resource":
                resource_location = self.tokens[self.index - 1].location
                declarations.append(ResourceDecl(resource_location, self._name("resource name")))
                self._expect_symbol(";")
            elif root.text == "device":
                property_location = self.tokens[self.index - 1].location
                self._expect_symbol("=")
                declarations.append(
                    DevicePropertyDecl(property_location, keyword, self._scalar())
                )
                self._expect_symbol(";")
            elif keyword == "component":
                declarations.append(self._component())
            elif keyword == "port":
                declarations.append(self._port())
            elif keyword == "module":
                declarations.append(self._module_instance())
            elif keyword == "net":
                declarations.append(self._net())
            elif keyword == "supply":
                declarations.append(self._supply())
            elif keyword == "interface":
                declarations.append(self._interface())
            elif keyword == "constraint":
                declarations.append(self._constraint())
            elif keyword == "configure":
                declarations.append(self._configuration())
            elif keyword == "power_state":
                declarations.append(self._power_state())
            else:
                self._error("PAR002", f"unknown declaration {keyword!r}", self.tokens[self.index - 1])
        self._expect_symbol("}")
        self._expect(TokenKind.EOF, "end of file")
        return Document(
            start,
            root.text,
            name,
            tuple(libraries),
            tuple(imports),
            tuple(declarations),
        )

    def _component(self) -> ComponentDecl:
        location = self.current.location
        ref = self._name("component reference")
        self._expect_symbol(":")
        part = self._qualified_name("part name")
        if self._accept_symbol(";"):
            return ComponentDecl(location, ref, part, {})
        attributes = self._assignment_block()
        return ComponentDecl(location, ref, part, attributes)

    def _port(self) -> PortDecl:
        location = self.current.location
        name = self._name("port name")
        self._expect_symbol(":")
        pin_type = self._name("port electrical type")
        self._expect_symbol(";")
        return PortDecl(location, name, pin_type)

    def _pin(self) -> PinDecl:
        location = self.current.location
        name = self._name("pin name")
        return PinDecl(location, name, self._assignment_block())

    def _pad(self) -> PadDecl:
        location = self.current.location
        name = self._name("device pad name")
        return PadDecl(location, name, self._assignment_block())

    def _power_domain(self) -> PowerDomainDecl:
        location = self.current.location
        name = self._name("power domain name")
        return PowerDomainDecl(location, name, self._assignment_block())

    def _power_state(self) -> PowerStateDecl:
        location = self.current.location
        name = self._name("power state name")
        raw_rails = self._assignment_block()
        rails: dict[str, str] = {}
        for rail, value in raw_rails.items():
            if not isinstance(value, str):
                self._error("PAR013", "power state value must be on, off, or unknown")
            rails[rail] = value
        return PowerStateDecl(location, name, rails)

    def _module_instance(self) -> ModuleInstanceDecl:
        location = self.current.location
        ref = self._name("module instance reference")
        self._expect_symbol(":")
        module = self._qualified_name("module name")
        self._expect_symbol(";")
        return ModuleInstanceDecl(location, ref, module)

    def _peripheral(self) -> PeripheralDecl:
        location = self.current.location
        name = self._name("peripheral name")
        self._expect_symbol(":")
        kind = self._name("peripheral kind")
        self._expect_symbol("{")
        signals: list[PeripheralSignalDecl] = []
        while not self._at_symbol("}"):
            self._expect_word("signal")
            signal_location = self.current.location
            signal_name = self._name("signal name")
            self._expect_symbol(":")
            pin_type = self._name("signal electrical type")
            attributes = {} if self._accept_symbol(";") else self._assignment_block()
            signals.append(
                PeripheralSignalDecl(signal_location, signal_name, pin_type, attributes)
            )
        self._expect_symbol("}")
        return PeripheralDecl(location, name, kind, tuple(signals))

    def _mux(self) -> MuxDecl:
        location = self.current.location
        pad = self._name("device pad")
        self._expect_symbol(":")
        peripheral = self._name("peripheral name")
        self._expect_symbol(".")
        signal = self._name("peripheral signal")
        return MuxDecl(location, pad, peripheral, signal, self._assignment_block())

    def _configuration(self) -> ConfigurationDecl:
        location = self.current.location
        component = self._name("component reference")
        self._expect_symbol(".")
        peripheral = self._name("peripheral name")
        self._expect_word("as")
        name = self._name("configuration name")
        raw_signals = self._assignment_block()
        signals: dict[str, str] = {}
        for signal, value in raw_signals.items():
            if not isinstance(value, str):
                self._error("PAR012", "configured signal must name a physical pin")
            signals[signal] = value
        return ConfigurationDecl(location, component, peripheral, name, signals)

    def _net(self) -> NetDecl:
        location = self.current.location
        name = self._name("net name")
        self._expect_symbol("{")
        endpoints: list[str] = []
        while not self._at_symbol("}"):
            endpoints.append(self._endpoint())
            self._expect_symbol(";")
        self._expect_symbol("}")
        return NetDecl(location, name, tuple(endpoints))

    def _supply(self) -> SupplyDecl:
        location = self.current.location
        name = self._name("supply name")
        return SupplyDecl(location, name, self._assignment_block())

    def _interface(self) -> InterfaceDecl:
        location = self.current.location
        name = self._name("interface name")
        self._expect_symbol(":")
        kind = self._name("interface kind")
        self._expect_symbol("{")
        attributes: dict[str, Scalar] = {}
        bindings: list[BindingDecl] = []
        while not self._at_symbol("}"):
            entry_location = self.current.location
            key = self._name("interface property or 'bind'")
            if key == "bind":
                component = self._name("component reference")
                raw_signals = self._assignment_block()
                signals: dict[str, str] = {}
                for signal, value in raw_signals.items():
                    if not isinstance(value, str):
                        self._error("PAR003", "interface pin binding must be a pin name")
                    signals[signal] = value
                bindings.append(BindingDecl(entry_location, component, signals))
            else:
                self._expect_symbol("=")
                attributes[key] = self._scalar()
                self._expect_symbol(";")
        self._expect_symbol("}")
        return InterfaceDecl(location, name, kind, attributes, tuple(bindings))

    def _constraint(self) -> ConstraintDecl:
        location = self.current.location
        kind = self._name("constraint kind")
        self._expect_symbol("(")
        targets: list[str] = []
        if not self._at_symbol(")"):
            targets.append(self._reference())
            while self._accept_symbol(","):
                targets.append(self._reference())
        self._expect_symbol(")")
        parameters = self._assignment_block()
        return ConstraintDecl(location, kind, tuple(targets), parameters)

    def _assignment_block(self) -> dict[str, Scalar]:
        self._expect_symbol("{")
        attributes: dict[str, Scalar] = {}
        while not self._at_symbol("}"):
            key_token = self.current
            key = self._name("property name")
            if key in attributes:
                self._error("PAR004", f"duplicate property {key!r}", key_token)
            self._expect_symbol("=")
            attributes[key] = self._scalar()
            self._expect_symbol(";")
        self._expect_symbol("}")
        return attributes

    def _scalar(self) -> Scalar:
        if self.current.kind is TokenKind.STRING:
            return self._advance().text
        if self.current.kind is TokenKind.NUMBER:
            number = self._advance().text
            if self.current.kind is TokenKind.IDENTIFIER:
                return RawQuantity(number, self._advance().text)
            self._error("PAR005", "a number must include a unit")
        if self.current.kind is TokenKind.IDENTIFIER:
            value = self._advance().text
            if value == "true":
                return True
            if value == "false":
                return False
            if self._accept_symbol("."):
                value = f"{value}.{self._name('pin name')}"
            return value
        self._error("PAR006", "expected a string, identifier, boolean, or quantity")

    def _endpoint(self) -> str:
        component = self._name("component reference")
        self._expect_symbol(".")
        return f"{component}.{self._name('pin name')}"

    def _reference(self) -> str:
        value = self._name("target")
        if self._accept_symbol("."):
            value = f"{value}.{self._name('pin name')}"
        return value

    def _name(self, expected: str) -> str:
        if self.current.kind not in (TokenKind.IDENTIFIER, TokenKind.NUMBER):
            self._error("PAR007", f"expected {expected}, found {self._describe(self.current)}")
        return self._advance().text

    def _qualified_name(self, expected: str) -> str:
        value = self._name(expected)
        while self._accept_symbol("."):
            value = f"{value}.{self._name(expected)}"
        return value

    def _expect_word(self, word: str) -> Token:
        token = self._expect(TokenKind.IDENTIFIER, repr(word))
        if token.text != word:
            self._error("PAR008", f"expected {word!r}, found {token.text!r}", token)
        return token

    def _expect_symbol(self, symbol: str) -> Token:
        if not self._at_symbol(symbol):
            self._error("PAR009", f"expected {symbol!r}, found {self._describe(self.current)}")
        return self._advance()

    def _accept_symbol(self, symbol: str) -> bool:
        if self._at_symbol(symbol):
            self._advance()
            return True
        return False

    def _at_symbol(self, symbol: str) -> bool:
        return self.current.kind is TokenKind.SYMBOL and self.current.text == symbol

    def _expect(self, kind: TokenKind, expected: str) -> Token:
        if self.current.kind is not kind:
            self._error("PAR010", f"expected {expected}, found {self._describe(self.current)}")
        return self._advance()

    def _advance(self) -> Token:
        token = self.current
        if token.kind is not TokenKind.EOF:
            self.index += 1
        return token

    @staticmethod
    def _describe(token: Token) -> str:
        return token.kind.value if token.kind is TokenKind.EOF else repr(token.text)

    def _error(self, code: str, message: str, token: Token | None = None):
        raise CopperScriptError(code, message, (token or self.current).location)


def parse(source: str, filename: str = "<memory>") -> Document:
    return Parser(tokenize(source, filename)).parse()
