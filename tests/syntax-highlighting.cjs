"use strict";
const {before, test} = require("node:test"), assert = require("node:assert/strict");
const fs = require("node:fs"), path = require("node:path"), {createRequire} = require("node:module");
const root = path.resolve(__dirname, "..");
const requireTools = createRequire(path.join(root, "integrations/vscode/tools/package.json"));
const textmate = requireTools("vscode-textmate"), oniguruma = requireTools("vscode-oniguruma");
let grammar;

before(async () => {
  const wasm = fs.readFileSync(requireTools.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const manifest = JSON.parse(fs.readFileSync(path.join(root, "integrations/vscode/package.json"), "utf8"));
  const contribution = manifest.contributes.grammars.find(entry => entry.language === "copper");
  const filename = path.join(root, "integrations/vscode", contribution.path);
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: patterns => new oniguruma.OnigScanner(patterns),
      createOnigString: source => new oniguruma.OnigString(source),
    }),
    loadGrammar: async scope => scope === contribution.scopeName
      ? textmate.parseRawGrammar(fs.readFileSync(filename, "utf8"), filename) : null,
  });
  grammar = await registry.loadGrammar(contribution.scopeName);
  assert(grammar, "Registered grammar must load in the VS Code tokenizer");
});

function tokenize(line, state = textmate.INITIAL) {
  const result = grammar.tokenizeLine(line, state);
  assert.equal(result.stoppedEarly, false, "Tokenization must finish");
  return result;
}

function expectScope(line, text, scope, state, occurrence = 0) {
  const result = tokenize(line, state);
  let offset = -1;
  for (let i = 0; i <= occurrence; i++) offset = line.indexOf(text, offset + 1);
  assert(offset >= 0, `Missing test text: ${text}`);
  const token = result.tokens.find(t => t.startIndex <= offset && t.endIndex > offset);
  assert(token?.scopes.includes(scope), `${text}: expected ${scope}, received ${token?.scopes}`);
  return result;
}

test("declarations, qualified types, properties and endpoints get theme scopes", () => {
  expectScope("board Example {", "board", "storage.type.copper");
  expectScope("board Example {", "Example", "entity.name.type.copper");
  const line = "component Ω_R1: parts.Resistor { value = 10kohm; }";
  expectScope(line, "Ω_R1", "variable.other.definition.copper");
  expectScope(line, "parts.Resistor", "entity.name.type.copper");
  expectScope(line, "value", "variable.other.property.copper");
  expectScope("net OUT { R1.1; ADC.CH0.positive; }", "R1.1", "variable.other.member.copper");
  expectScope("net OUT { R1.1; ADC.CH0.positive; }", "ADC.CH0.positive", "variable.other.member.copper");
  expectScope("constraint fixed_placement(R1) {", "fixed_placement", "entity.name.function.copper");
  expectScope('import parts "github.com/copper/parts";', "import", "keyword.control.import.copper");
  expectScope("configure U1.UART as DEBUG {", "as", "keyword.control.import.copper");
});

test("engineering units work with compact, spaced and signed fixed-point numbers", () => {
  const units = "V mV ohm kohm Mohm F uF nF pF H mH uH nH m mm um A mA uA Hz kHz MHz GHz deg".split(" ");
  for (const unit of units) {
    for (const quantity of [`12.5${unit}`, `12.5 ${unit}`, `-12.5${unit}`, `+12.5 ${unit}`]) {
      const line = `value = ${quantity};`;
      expectScope(line, "12.5", "constant.numeric.copper");
      expectScope(line, unit, "storage.type.unit.copper");
    }
  }
  expectScope("rotation = -90; external = true; enabled = false;", "90", "constant.numeric.copper");
  expectScope("external = true; enabled = false;", "true", "constant.language.boolean.copper");
  expectScope("external = true; enabled = false;", "false", "constant.language.boolean.copper");
});

test("macro definitions and bindings highlight named geometry and angle units", () => {
  expectScope("macro BuckLayout {", "macro", "storage.type.copper");
  expectScope("macro BuckLayout {", "BuckLayout", "entity.name.type.copper");
  for (const kind of ["physical_macro", "member", "track", "via", "protected_region", "plane_return", "width_contract", "edge_opening", "contact"]) {
    expectScope(`${kind} NAME {`, kind, "storage.type.copper");
    expectScope(`${kind} NAME {`, "NAME", "variable.other.definition.copper");
  }
  expectScope("physical_macro UNIT: layout.BuckLayout on PWR {", "on", "storage.type.copper");
  expectScope("rotation = 45 deg;", "deg", "storage.type.unit.copper");
});

test("keywords and units do not split longer or hyphenated identifiers", () => {
  for (const word of ["component-height", "true_value", "false-state", "mm_route", "VDD", "board2", "R123", "α_component"]) {
    expectScope(word, word, "variable.other.copper");
    const tokens = tokenize(word).tokens;
    assert.equal(tokens.length, 1, word);
    assert.equal(tokens[0].endIndex, word.length, word);
  }
  const tokens = tokenize("value = 10MHz_driver;").tokens;
  assert(tokens.every(token => !token.scopes.includes("storage.type.unit.copper")));
});

test("comments shield quotes, keywords and quantities", () => {
  for (const [line, scope] of [
    ['# component U1: MCU { value = "10mm"; }', "comment.line.number-sign.copper"],
    ['// board Example { voltage = 3.3V; }', "comment.line.double-slash.copper"],
  ]) {
    const tokens = tokenize(line).tokens;
    assert(tokens.every(token => token.scopes.includes(scope)));
    assert(tokens.every(token => !token.scopes.includes("string.quoted.double.copper")));
  }
});

test("strings shield URLs and comments, handle escapes and recover after closing", () => {
  const line = String.raw`label = "https://example/#board \"quoted\" \\ \n \t"; external = true;`;
  expectScope(line, "https://", "string.quoted.double.copper");
  expectScope(line, "#board", "string.quoted.double.copper");
  expectScope(line, String.raw`\"`, "constant.character.escape.copper");
  expectScope(line, "true", "constant.language.boolean.copper");
  expectScope(String.raw`label = "bad\q";`, String.raw`\q`, "invalid.illegal.escape.copper");
  const first = tokenize('label = "unfinished //');
  const second = '# still string"; external = false;';
  expectScope(second, "#", "string.quoted.double.copper", first.ruleStack);
  expectScope(second, "false", "constant.language.boolean.copper", first.ruleStack);
});

test("unfinished declarations and real electrical/mechanical examples tokenize", () => {
  expectScope("component U1:", "component", "storage.type.copper");
  expectScope("voltage = 3.3", "3.3", "constant.numeric.copper");
  for (const example of ["valid_board", "device_model_showcase", "mechanical_curves", "mechanical_profile_project"]) {
    const source = fs.readFileSync(path.join(root, "examples", example, "board.copper"), "utf8");
    let state = textmate.INITIAL;
    for (const line of source.split(/\r?\n/)) {
      const result = tokenize(line, state);
      state = result.ruleStack;
      assert(result.tokens.every(token => !token.scopes.includes("invalid.illegal.escape.copper")), example);
    }
    assert(state.equals(tokenize("").ruleStack), `${example}: no unterminated string scope`);
  }
});
