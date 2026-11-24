// The canonical text of a document (2.4): JSON with 2 spaces, the members in the byte order of their names, one member or item for each
// line, a line end at the end. The page writes it to show the two versions of a conflict and to compare a draft with the stored document.
// Pure module: no DOM access. A value is an object, an array, a string, an integer, true or false.

const encoder = new TextEncoder();

// Compares two names by their UTF-8 bytes.
export function compareBytes(a, b) {
  const x = encoder.encode(a);
  const y = encoder.encode(b);
  const n = Math.min(x.length, y.length);
  for (let i = 0; i < n; i += 1) if (x[i] !== y[i]) return x[i] - y[i];
  return x.length - y.length;
}

function write(value, depth) {
  const pad = "  ".repeat(depth + 1);
  const end = "  ".repeat(depth);
  if (value === null || value === undefined) throw new TypeError("A document has no null.");
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 0) throw new TypeError("A document has integers from 0 to 2^53-1 only.");
    return String(value);
  }
  if (typeof value === "string") return quote(value);
  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    return `[\n${value.map((item) => pad + write(item, depth + 1)).join(",\n")}\n${end}]`;
  }
  const names = Object.keys(value).sort(compareBytes);
  if (names.length === 0) return "{}";
  return `{\n${names.map((name) => `${pad}${quote(name)}: ${write(value[name], depth + 1)}`).join(",\n")}\n${end}}`;
}

// The escapes of 2.4: \" \\ \b \f \n \r \t, \u00xx for the other characters below U+0020. Any other character stays as it is.
function quote(text) {
  let out = '"';
  for (const ch of text) {
    const code = ch.codePointAt(0);
    if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (ch === "\b") out += "\\b";
    else if (ch === "\f") out += "\\f";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch === "\t") out += "\\t";
    else if (code < 0x20) out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += ch;
  }
  return `${out}"`;
}

export function canonical(value) {
  return `${write(value, 0)}\n`;
}
