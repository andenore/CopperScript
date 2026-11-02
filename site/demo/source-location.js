// Rust diagnostics count Unicode code points; textarea selections use UTF-16.
export function sourceOffset(source, line, column) {
  const lines = source.split("\n");
  const index = Math.max(0, Math.min(line - 1, lines.length - 1));
  const before = lines.slice(0, index).reduce((total, text) => total + text.length + 1, 0);
  return before + Array.from(lines[index]).slice(0, Math.max(0, column - 1)).join("").length;
}
