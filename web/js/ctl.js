// The text of a run for the page (5.22). The output and the events of a run are plain text. The page shows each control character as a
// visible symbol, so a log cannot hide text or move the cursor.
//   U+0000 to U+001F  ->  U+2400 to U+241F  (the line feed U+000A stays a line end: it breaks the line, it hides and moves nothing)
//   U+007F            ->  U+2421
//   U+0080 to U+009F, the marks of the direction of the text (U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069, U+061C) and the line and
//   paragraph separators (U+2028, U+2029)  ->  the text <U+XXXX>
// Pure part: split(text) gives a list of parts. DOM part: appendText(node, text).

// A control character: the ones that the page replaces. The line feed is not in the set.
// eslint-disable-next-line no-control-regex
const SPECIAL = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f\u061c\u200e\u200f\u2028\u2029\u202a-\u202e\u2066-\u2069]/u;
const SPECIAL_G = new RegExp(SPECIAL.source, "gu");

// The visible text of one character that the page replaces.
export function symbol(ch) {
  const code = ch.codePointAt(0);
  if (code <= 0x1f) return String.fromCodePoint(0x2400 + code);
  if (code === 0x7f) return "\u2421";
  return `<U+${code.toString(16).toUpperCase().padStart(4, "0")}>`;
}

// True when the text has a character that the page replaces.
export function hasSpecial(text) {
  return SPECIAL.test(text);
}

// Splits a text in parts: { text } for plain text, { symbol, code } for a replaced character.
export function split(text) {
  const parts = [];
  let last = 0;
  for (const match of text.matchAll(SPECIAL_G)) {
    if (match.index > last) parts.push({ text: text.slice(last, match.index) });
    parts.push({ symbol: symbol(match[0]), code: match[0].codePointAt(0) });
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push({ text: text.slice(last) });
  return parts;
}

// The plain visible text: every replaced character is its symbol. Used for a short text in a table cell and for the tests.
export function visible(text) {
  return text.replace(SPECIAL_G, (ch) => symbol(ch));
}

// Adds a text to a node. The plain parts are text nodes; a replaced character is an element <i class="ctl"> with a title that names it.
export function appendText(node, text) {
  if (!SPECIAL.test(text)) {
    node.append(document.createTextNode(text));
    return;
  }
  for (const part of split(text)) {
    if (part.text !== undefined) {
      node.append(document.createTextNode(part.text));
    } else {
      const mark = document.createElement("i");
      mark.className = "ctl";
      mark.setAttribute("title", `U+${part.code.toString(16).toUpperCase().padStart(4, "0")}`);
      mark.textContent = part.symbol;
      node.append(mark);
    }
  }
}
