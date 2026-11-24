// Copied from Hune, web/js/editor/diff.js, commit a55fff3. Edits: none.
// Line diff: the shortest edit script of Myers on exact line equality. Pure module: no DOM access.

export const MAX_EDIT_DISTANCE = 10000;
export const MAX_LINES = 100000;
const CONTEXT = 3;

// Splits a text at each LF. A final empty piece after a closing LF is not a line.
export function splitLines(text) {
  if (text === "") return [];
  const lines = String(text).split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

// The edit script as a list of { op, a, b } where op is " ", "-" or "+", a is the index in A, b the index in B.
function myers(a, b, maxD) {
  const n = a.length;
  const m = b.length;
  const max = Math.min(n + m, maxD);
  const offset = max + 1;
  let v = new Int32Array(2 * max + 3);
  const trace = [];
  for (let d = 0; d <= max; d++) {
    trace.push({ arr: v.slice(offset - d - 1, offset + d + 2), lo: -d - 1 });
    for (let k = -d; k <= d; k += 2) {
      let x;
      if (k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1])) x = v[offset + k + 1];
      else x = v[offset + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x += 1;
        y += 1;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) return backtrack(trace, a, b, d, offset);
    }
  }
  return null;
}

function backtrack(trace, a, b, dEnd, offset) {
  const ops = [];
  let x = a.length;
  let y = b.length;
  for (let d = dEnd; d > 0; d--) {
    const t = trace[d];
    const at = (kk) => t.arr[kk - t.lo];
    const k = x - y;
    const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
    const prevX = at(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x -= 1;
      y -= 1;
      ops.push({ op: " ", a: x, b: y });
    }
    if (x === prevX) {
      y -= 1;
      ops.push({ op: "+", a: x, b: y });
    } else {
      x -= 1;
      ops.push({ op: "-", a: x, b: y });
    }
  }
  while (x > 0 && y > 0) {
    x -= 1;
    y -= 1;
    ops.push({ op: " ", a: x, b: y });
  }
  return ops.reverse();
}

/**
 * Diff of two texts. Result: { tooLarge, distance, hunks: [{ header, aStart, aCount, bStart, bCount, lines }] }.
 * A line is { op, text, aNo, bNo } with line numbers from 1 (null on the side that has no such line).
 */
export function diffLines(aText, bText, { maxDistance = MAX_EDIT_DISTANCE, maxLines = MAX_LINES } = {}) {
  const a = splitLines(aText);
  const b = splitLines(bText);
  if (a.length > maxLines || b.length > maxLines) return { tooLarge: true, distance: null, hunks: [] };
  // Common prefix and suffix are cut first: real edits are small.
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const core = myers(a.slice(start, endA), b.slice(start, endB), maxDistance);
  if (core === null) return { tooLarge: true, distance: null, hunks: [] };
  const all = [];
  for (let i = 0; i < start; i++) all.push({ op: " ", text: a[i], aNo: i + 1, bNo: i + 1 });
  for (const e of core) {
    if (e.op === " ") all.push({ op: " ", text: a[start + e.a], aNo: start + e.a + 1, bNo: start + e.b + 1 });
    else if (e.op === "-") all.push({ op: "-", text: a[start + e.a], aNo: start + e.a + 1, bNo: null });
    else all.push({ op: "+", text: b[start + e.b], aNo: null, bNo: start + e.b + 1 });
  }
  for (let i = endA; i < a.length; i++) all.push({ op: " ", text: a[i], aNo: i + 1, bNo: endB + (i - endA) + 1 });
  const distance = core.filter((e) => e.op !== " ").length;
  return { tooLarge: false, distance, hunks: toHunks(all) };
}

function toHunks(all) {
  const changes = [];
  all.forEach((l, i) => {
    if (l.op !== " ") changes.push(i);
  });
  if (!changes.length) return [];
  const ranges = [];
  for (const i of changes) {
    const from = Math.max(0, i - CONTEXT);
    const to = Math.min(all.length - 1, i + CONTEXT);
    const last = ranges[ranges.length - 1];
    if (last && from <= last.to + 1) last.to = Math.max(last.to, to);
    else ranges.push({ from, to });
  }
  return ranges.map(({ from, to }) => {
    const lines = all.slice(from, to + 1);
    const aLines = lines.filter((l) => l.aNo !== null);
    const bLines = lines.filter((l) => l.bNo !== null);
    // As in a unified diff, an empty side starts at the line before.
    const aStart = aLines.length ? aLines[0].aNo : firstBefore(all, from, "aNo");
    const bStart = bLines.length ? bLines[0].bNo : firstBefore(all, from, "bNo");
    return { header: `@@ -${aStart},${aLines.length} +${bStart},${bLines.length} @@`, aStart, aCount: aLines.length, bStart, bCount: bLines.length, lines };
  });
}

function firstBefore(all, index, key) {
  for (let i = index - 1; i >= 0; i--) if (all[i][key] !== null) return all[i][key];
  return 0;
}
