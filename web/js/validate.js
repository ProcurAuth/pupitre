// The checks of the page before a write (5.22): the value kinds of 2.12, the secret values of 2.25, the ids of 2.1,
// the host document of 2.8 and the command parameters of 4.33. The server checks again. These functions never change a value:
// a wrong value gives a message with the expected type and range, and the value (the same form as the server).
// All functions are pure: no DOM access.

export const MAX_SAFE = 9007199254740991;
export const ID_RE = /^[a-z0-9][a-z0-9-]{0,62}$/;
export const LABEL_RE = /^[a-z][a-z0-9_]{0,62}$/;
export const KEY_SEGMENT_RE = /^[a-z][a-z0-9_]{0,62}$/;
export const SECRET_MIN_BYTES = 8;
export const SECRET_MAX_BYTES = 65536;

const encoder = new TextEncoder();
export const byteLength = (text) => encoder.encode(text).length;
// eslint-disable-next-line no-control-regex
const CONTROL_RE = /[\u0000-\u001f\u007f-\u009f]/u;
const hasControl = (text) => CONTROL_RE.test(text);

// The text of a JSON value in an error, as the server writes it.
export function shown(value, secret = false) {
  if (secret) return "<hidden>";
  return JSON.stringify(value);
}

// --- durations (6.4, 2.12) ---------------------------------------------------------------------------------------------

const DURATION_FACTORS = { ms: 1, s: 1000, m: 60000, h: 3600000 };

// Milliseconds of a duration text, or null when the text is not a duration of 6.4.
export function parseDuration(text) {
  const match = /^(0|[1-9][0-9]*)(ms|s|m|h)$/.exec(typeof text === "string" ? text : "");
  if (!match) return null;
  const ms = Number(match[1]) * DURATION_FACTORS[match[2]];
  return Number.isSafeInteger(ms) ? ms : null;
}

// The canonical text of a number of milliseconds: one whole number and the largest unit that gives a whole number.
export function formatDuration(ms) {
  if (ms === 0) return "0s";
  if (ms % 3600000 === 0) return `${ms / 3600000}h`;
  if (ms % 60000 === 0) return `${ms / 60000}m`;
  if (ms % 1000 === 0) return `${ms / 1000}s`;
  return `${ms}ms`;
}

// --- addresses ------------------------------------------------------------------------------------------------------------

function parseV4(text) {
  const parts = text.split(".");
  if (parts.length !== 4) return null;
  const out = [];
  for (const part of parts) {
    if (!/^(0|[1-9][0-9]{0,2})$/.test(part)) return null;
    const n = Number(part);
    if (n > 255) return null;
    out.push(n);
  }
  return out;
}

// The eight groups of an IPv6 text, or null. The text has no zone.
function parseV6(text) {
  if (!/^[0-9a-fA-F:.]+$/.test(text) || !text.includes(":")) return null;
  let tail = [];
  let head = text;
  const lastColon = text.lastIndexOf(":");
  if (text.slice(lastColon + 1).includes(".")) {
    const v4 = parseV4(text.slice(lastColon + 1));
    if (!v4) return null;
    tail = [(v4[0] << 8) | v4[1], (v4[2] << 8) | v4[3]];
    head = text.slice(0, lastColon + 1);
    if (head.endsWith("::")) {
      // a text such as "::a.b.c.d" keeps the two colons.
    } else {
      head = head.slice(0, -1);
    }
  }
  const halves = head.split("::");
  if (halves.length > 2) return null;
  const groups = (part) => (part === "" ? [] : part.split(":"));
  const left = groups(halves[0]);
  const right = halves.length === 2 ? groups(halves[1]) : [];
  const all = [...left, ...right];
  if (!all.every((g) => /^[0-9a-fA-F]{1,4}$/.test(g))) return null;
  const nums = (list) => list.map((g) => parseInt(g, 16));
  const fixed = nums(left).length + nums(right).length + tail.length;
  if (halves.length === 1) {
    if (fixed !== 8) return null;
    return [...nums(left), ...tail];
  }
  if (fixed > 7) return null;
  return [...nums(left), ...new Array(8 - fixed).fill(0), ...nums(right), ...tail];
}

// The canonical text of an IPv6 address, as the Rust standard library writes it.
function formatV6(g) {
  if (g.slice(0, 5).every((x) => x === 0) && g[5] === 0xffff) return `::ffff:${g[6] >> 8}.${g[6] & 255}.${g[7] >> 8}.${g[7] & 255}`;
  let best = { at: -1, len: 0 };
  for (let i = 0; i < 8; ) {
    if (g[i] !== 0) {
      i += 1;
      continue;
    }
    let j = i;
    while (j < 8 && g[j] === 0) j += 1;
    if (j - i > best.len) best = { at: i, len: j - i };
    i = j;
  }
  const hex = (list) => list.map((x) => x.toString(16)).join(":");
  if (best.len > 1) return `${hex(g.slice(0, best.at))}::${hex(g.slice(best.at + best.len))}`;
  return hex(g);
}

// True for an IPv4 or IPv6 address in the canonical form.
export function isCanonicalIp(text) {
  const v4 = parseV4(text);
  if (v4) return v4.join(".") === text;
  const v6 = parseV6(text);
  return !!v6 && formatV6(v6) === text;
}

// True for any IP address that parses (the rule host_or_ip accepts the forms that the Rust parser accepts).
export function isIp(text) {
  return !!parseV4(text) || !!parseV6(text);
}

// A DNS name of 1 to 253 bytes: labels of 1 to 63 characters from A-Z a-z 0-9 -, none that starts or ends with -.
export function isDnsName(text) {
  if (text.length === 0 || text.length > 253) return false;
  return text.split(".").every((label) => label.length >= 1 && label.length <= 63 && /^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$/.test(label));
}

// The canonical form of a socket address: an IPv4 address and a port, or an IPv6 address in brackets and a port.
export function isCanonicalSocketAddr(text) {
  const match = /^(?:\[([0-9a-fA-F:.]+)\]|([0-9.]+)):([0-9]{1,5})$/.exec(text);
  if (!match) return false;
  const port = Number(match[3]);
  if (port > 65535 || String(port) !== match[3]) return false;
  if (match[2] !== undefined) return isCanonicalIp(match[2]) && !!parseV4(match[2]);
  return !!parseV6(match[1]) && isCanonicalIp(match[1]);
}

// --- the value kinds ----------------------------------------------------------------------------------------------------------

function ruleHolds(rule, text) {
  switch (rule) {
    case "any":
    case undefined:
      return true;
    case "no_control":
      return !hasControl(text);
    case "printable_ascii":
      return /^[ -~]*$/.test(text);
    case "no_control_no_space":
      return !hasControl(text) && !text.includes(" ");
    case "dns_name":
      return isDnsName(text);
    case "host_or_ip":
      return isDnsName(text) || isIp(text);
    default:
      return false;
  }
}

const RULE_WORDS = {
  any: (a, b) => `${a} to ${b} bytes`,
  no_control: (a, b) => `${a} to ${b} bytes with no control character`,
  printable_ascii: (a, b) => `${a} to ${b} bytes of printable ASCII`,
  no_control_no_space: (a, b) => `${a} to ${b} bytes with no space and no control character`,
  dns_name: (a, b) => `${a} to ${b} bytes of a DNS name`,
  host_or_ip: (a, b) => `${a} to ${b} bytes of a DNS name or an IP address`,
};

// The text "type and range" of a key definition (the same words as the server).
export function expectedText(def) {
  switch (def.kind) {
    case "text": {
      const rule = def.rule || "any";
      if (rule === "dns_name") return `a DNS name of ${def.min} to ${def.max} bytes`;
      if (rule === "host_or_ip") return `a DNS name of ${def.min} to ${def.max} bytes, or an IP address`;
      return `a text of ${(RULE_WORDS[rule] || RULE_WORDS.any)(def.min, def.max)}`;
    }
    case "text_list":
      return `a list of ${def.min_items} to ${def.max_items} texts, each of ${(RULE_WORDS[def.rule || "any"] || RULE_WORDS.any)(def.item_min, def.item_max)}`;
    case "enum": {
      const quoted = (def.values || []).map((v) => JSON.stringify(v));
      if (quoted.length > 1) return `${quoted.slice(0, -1).join(", ")} or ${quoted[quoted.length - 1]}`;
      return quoted[0] || "";
    }
    case "integer":
      return `an integer from ${def.min} to ${def.max}`;
    case "size":
      return `a size in bytes from ${def.min} to ${def.max}`;
    case "duration":
      return `a duration from ${def.min} to ${def.max}`;
    case "bool":
      return "a boolean";
    case "path":
      return "an absolute path of 1 to 4096 bytes";
    case "socket_addr":
      return "an IP address and a port in canonical form";
    default:
      return "a valid value";
  }
}

// Checks a JSON value against a key definition. It returns null, or the message of the error.
export function checkValue(def, value) {
  const bad = () => `expected ${expectedText(def)}, found ${shown(value, def.secret === true)}`;
  const isInt = (v) => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
  switch (def.kind) {
    case "text": {
      if (typeof value !== "string" || value.includes("\u0000")) return bad();
      const n = byteLength(value);
      return n >= def.min && n <= def.max && ruleHolds(def.rule, value) ? null : bad();
    }
    case "text_list": {
      if (!Array.isArray(value) || value.length < def.min_items || value.length > def.max_items) return bad();
      for (const item of value) {
        if (typeof item !== "string" || item.includes("\u0000")) return bad();
        const n = byteLength(item);
        if (n < def.item_min || n > def.item_max || !ruleHolds(def.rule, item)) return bad();
      }
      return null;
    }
    case "enum":
      return typeof value === "string" && (def.values || []).includes(value) ? null : bad();
    case "integer":
    case "size":
      return isInt(value) && value >= def.min && value <= def.max ? null : bad();
    case "duration": {
      const ms = parseDuration(value);
      const lo = parseDuration(def.min);
      const hi = parseDuration(def.max);
      if (ms === null || lo === null || hi === null || ms < lo || ms > hi) return bad();
      if (formatDuration(ms) !== value) return `expected ${formatDuration(ms)}, found ${shown(value)}`;
      return null;
    }
    case "bool":
      return typeof value === "boolean" ? null : bad();
    case "path":
      return typeof value === "string" && value.startsWith("/") && byteLength(value) <= 4096 && !value.includes("\u0000") ? null : bad();
    case "socket_addr":
      return typeof value === "string" && isCanonicalSocketAddr(value) ? null : bad();
    default:
      return `the kind ${shown(def.kind)} is not known`;
  }
}

// Checks one value against all definitions of a key path (2.15): the first message of a failure, else null.
export function checkAgainstAll(defs, value) {
  for (const def of defs) {
    const message = checkValue(def, value);
    if (message) return message;
  }
  return null;
}

// Reads the text of a field into the JSON value of a kind. It never repairs the text: a text that has no valid form gives
// { error } with the expected type and the text. A text that has the form gives { value }, and checkValue then checks the range.
export function readField(def, text) {
  switch (def.kind) {
    case "integer":
    case "size": {
      if (!/^(0|[1-9][0-9]*)$/.test(text)) return { error: `expected ${expectedText(def)}, found ${shown(text)}` };
      const n = Number(text);
      if (!Number.isSafeInteger(n)) return { error: `expected ${expectedText(def)}, found ${shown(text)}` };
      return { value: n };
    }
    case "bool":
      return text === "true" ? { value: true } : text === "false" ? { value: false } : { error: `expected ${expectedText(def)}, found ${shown(text)}` };
    case "text_list": {
      if (text === "") return { value: [] };
      return { value: text.split("\n") };
    }
    default:
      return { value: text };
  }
}

// The text of a field for a JSON value: the inverse of readField.
export function fieldText(def, value) {
  if (value === undefined || value === null) return "";
  if (def.kind === "text_list") return Array.isArray(value) ? value.join("\n") : "";
  return String(value);
}

// --- the secret values (2.24, 2.25) --------------------------------------------------------------------------------------------

export function checkSecret(defs, value) {
  const n = byteLength(value);
  if (n < SECRET_MIN_BYTES || n > SECRET_MAX_BYTES) return `expected a text of ${SECRET_MIN_BYTES} to ${SECRET_MAX_BYTES} bytes`;
  if (value.includes("\u0000")) return "expected a text with no NUL character";
  for (const def of defs) {
    const message = checkValue({ ...def, secret: true }, value);
    if (message) return message;
  }
  return null;
}

// --- ids, hosts ---------------------------------------------------------------------------------------------------------------------

export function checkId(text, what) {
  return ID_RE.test(text) ? null : `${what}: expected 1 to 63 characters from a-z, 0-9 and -, the first one not a -, found ${shown(text)}`;
}

export function checkLabel(text) {
  return LABEL_RE.test(text) ? null : `expected a label of 1 to 63 characters from a-z, 0-9 and _, the first one a letter, found ${shown(text)}`;
}

export function checkLabelText(text, what = "label") {
  if (text.length < 1 || [...text].length > 64 || hasControl(text)) return `${what}: expected a text of 1 to 64 characters with no control character, found ${shown(text)}`;
  return null;
}

export function checkHostAddress(text) {
  if (byteLength(text) < 1 || byteLength(text) > 253) return `address: expected a DNS name or an IP address of 1 to 253 bytes, found ${shown(text)}`;
  if (isIp(text)) return isCanonicalIp(text) ? null : `address: expected an IP address in canonical form, found ${shown(text)}`;
  const labels = text.split(".");
  if (!isDnsName(text) || /^[0-9]+$/.test(labels[labels.length - 1])) return `address: expected a DNS name whose last label is not all digits, or an IP address, found ${shown(text)}`;
  return null;
}

export function checkHostPort(text) {
  if (text === "") return null;
  if (!/^(0|[1-9][0-9]*)$/.test(text)) return `port: expected an integer from 1 to 65535, found ${shown(text)}`;
  const n = Number(text);
  return n >= 1 && n <= 65535 ? null : `port: expected an integer from 1 to 65535, found ${shown(text)}`;
}

// --- the parameters of a command (4.33) ---------------------------------------------------------------------------------------------

// The text "type and range" of a parameter block.
export function paramExpected(p) {
  switch (p.type) {
    case "text":
      return `a text of ${p.min} to ${p.max} characters with no control character`;
    case "integer":
      return `an integer from ${p.min} to ${p.max}`;
    case "boolean":
      return "a boolean";
    case "duration":
      return `a duration from ${p.min_duration} to ${p.max_duration}`;
    case "choice":
      return (p.choices || []).map((c) => JSON.stringify(c)).join(", ");
    default:
      return "a valid value";
  }
}

// Reads the text of a field into the request value of a parameter. An empty text of a field that is not required is absent (null).
export function readParam(p, text) {
  if (p.type === "boolean") return { value: text === "true" };
  if (text === "") return p.required ? { error: `expected ${paramExpected(p)}, found ""` } : { absent: true };
  if (p.type === "integer") {
    if (!/^(0|[1-9][0-9]*)$/.test(text) || !Number.isSafeInteger(Number(text))) return { error: `expected ${paramExpected(p)}, found ${shown(text)}` };
    return { value: Number(text) };
  }
  return { value: text };
}

// Checks a request value of a parameter. It returns null or the message.
export function checkParam(p, value) {
  const bad = () => `expected ${paramExpected(p)}, found ${shown(value)}`;
  switch (p.type) {
    case "text": {
      if (typeof value !== "string" || hasControl(value) || value.includes("\u0000")) return bad();
      const n = [...value].length;
      return n >= p.min && n <= p.max ? null : bad();
    }
    case "integer":
      return typeof value === "number" && Number.isSafeInteger(value) && value >= p.min && value <= p.max ? null : bad();
    case "boolean":
      return typeof value === "boolean" ? null : bad();
    case "duration": {
      const ms = parseDuration(value);
      const lo = parseDuration(p.min_duration);
      const hi = parseDuration(p.max_duration);
      if (ms === null || lo === null || hi === null || ms < lo || ms > hi) return bad();
      return formatDuration(ms) === value ? null : `expected ${formatDuration(ms)}, found ${shown(value)}`;
    }
    case "choice":
      return typeof value === "string" && (p.choices || []).includes(value) ? null : bad();
    default:
      return bad();
  }
}

// --- the message of a version (2.18) ------------------------------------------------------------------------------------------------

export function checkMessage(text) {
  if ([...text].length > 500) return `message: expected a text of 0 to 500 characters, found ${[...text].length} characters`;
  // A line end is allowed in a message; any other control character is not.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/u.test(text)) return "message: expected a text with no control character except a line end";
  return null;
}

export function checkNote(text) {
  const n = [...text].length;
  if (n < 1 || n > 500) return `expected a note of 1 to 500 characters, found ${n} characters`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/u.test(text)) return "expected a note with no control character except a line end";
  return null;
}
