// The state and the address helpers that all modules share (5.2, 5.21, 5.23).
// The shell holds the build id in a meta element and the base path in <base href>.

function readBuild() {
  const node = typeof document === "undefined" ? null : document.querySelector('meta[name="pupitre-build"]');
  return node ? node.getAttribute("content") || "" : "";
}

export const build = readBuild();

// The answers of the session and of the catalog. main.js fills them at start.
export const app = { session: null, catalog: null };

// The URL of a static file, relative to <base href> (5.23).
export function assetUrl(path) {
  return `static/${build}/${path}`;
}

// The path of <base href>, with a final "/".
export function basePath() {
  const path = new URL(document.baseURI).pathname;
  return path.endsWith("/") ? path : `${path}/`;
}

// The path of the location relative to the base path, with no query and no fragment, and no slash at the start or at the end.
export function relativePath(pathname = window.location.pathname) {
  const base = basePath();
  const rel = pathname.startsWith(base) ? pathname.slice(base.length) : pathname.replace(/^\/+/, "");
  return rel.replace(/\/+$/, "");
}

// The address of a page, relative to <base href>. The segments are ids, so they need no escape; the function escapes them in any case.
export function pageUrl(...segments) {
  return segments.map((s) => encodeURIComponent(s)).join("/");
}
