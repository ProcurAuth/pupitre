// One formatter for each kind of value, so that the same value reads the same on every page (DATA-07).
// A time is shown in the local time of the browser, on 24 hours, with the year only when it is not the current year (DATA-04).

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const p2 = (n) => String(n).padStart(2, "0");

// The local time of a server time text, for example "24 Nov 10:12:03". An empty text gives "-".
export function localTime(iso, now = Date.now()) {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "-";
  const d = new Date(ms);
  const year = d.getFullYear() === new Date(now).getFullYear() ? "" : ` ${d.getFullYear()}`;
  return `${d.getDate()} ${MONTHS[d.getMonth()]}${year} ${p2(d.getHours())}:${p2(d.getMinutes())}:${p2(d.getSeconds())}`;
}

// A short age: "now", "5m ago", "6h ago", "3d ago". A time in the future gives "now".
export function shortAgo(iso, now = Date.now()) {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "-";
  const s = Math.floor((now - ms) / 1000);
  if (s < 5) return "now";
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 365) return `${Math.floor(s / 86400)}d ago`;
  return `${Math.floor(s / (86400 * 365))}y ago`;
}

// A count of bytes: "1,842 bytes", "12.3 KiB".
export function bytesText(n) {
  if (!Number.isFinite(n)) return "-";
  if (n < 1024) return `${n.toLocaleString("en-US")} bytes`;
  const units = ["KiB", "MiB", "GiB"];
  let value = n / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[i]}`;
}

// The text of a value of a key, for a table cell. It never changes the value: a text shows as it is.
export function valueText(value) {
  if (value === undefined || value === null) return "-";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (Array.isArray(value)) return value.length ? value.join(", ") : "(empty list)";
  if (value === "") return "(empty text)";
  return String(value);
}

// The text of a node path of 3.8 for a run: "environment:production/unit:relay" is shown as it is; this function only cuts a long path.
export function shortId(id) {
  return typeof id === "string" && id.length > 13 ? id.slice(0, 8) : id;
}

export const pluralize = (n, one, many) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;
