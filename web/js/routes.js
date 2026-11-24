// The routes of the pages (5.21). A path is relative to the base path. The function is pure: it takes the path and the query text.
// It gives { kind, ...params } for a path of the table of 5.21, and { kind: "none" } for any other path.
// Creation uses the query ?new=1 on the page of the list: /?new=1 (environment), /e/{e}?tab=units&new=1 (unit), /e/{e}?tab=hosts&new=1 (host).

import { ID_RE } from "./validate.js";

export const TABS = ["units", "hosts", "common", "secrets", "runs"];
const RUN_ID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NONE = { kind: "none" };

export function parseRoute(rel, search = "") {
  const q = new URLSearchParams(search);
  const known = new Set(["tab", "new", "doc", "kind", "check", "environment", "status"]);
  for (const key of q.keys()) if (!known.has(key)) return NONE;
  const create = q.get("new") === "1";
  if (rel === "") return { kind: "environments", create };
  const s = rel.split("/");
  if (s[0] === "runs") {
    if (s.length === 1) return { kind: "runs", environment: q.get("environment") || "", status: q.get("status") || "" };
    if (s.length === 2 && RUN_ID_RE.test(s[1])) return { kind: "run", run: s[1] };
    return NONE;
  }
  if (s[0] !== "e" || s.length < 2 || !ID_RE.test(s[1])) return NONE;
  const env = s[1];
  if (s.length === 2) {
    const tab = q.get("tab") || "units";
    if (!TABS.includes(tab)) return NONE;
    return { kind: "environment", env, tab, create: create && (tab === "units" || tab === "hosts") };
  }
  if (s[2] === "history" && s.length === 3) {
    const doc = q.get("doc") || "environment";
    return doc === "environment" || doc === "common" ? { kind: "history", env, doc } : NONE;
  }
  if (s[2] === "commands") {
    if (s.length === 3) return { kind: "commands", env };
    if (s.length === 4 && ID_RE.test(s[3])) return { kind: "command", env, command: s[3] };
    return NONE;
  }
  if (s[2] === "hosts" && s.length >= 4 && ID_RE.test(s[3])) {
    if (s.length === 4) return { kind: "host", env, host: s[3] };
    if (s.length === 5 && s[4] === "history") return { kind: "history", env, host: s[3], doc: "host" };
    return NONE;
  }
  if (s[2] === "units" && s.length >= 4 && ID_RE.test(s[3])) {
    const unit = s[3];
    if (s.length === 4) return { kind: "unit", env, unit };
    if (s.length === 5 && s[4] === "history") {
      const doc = q.get("doc") || "values";
      return doc === "values" || doc === "unit" ? { kind: "history", env, unit, doc } : NONE;
    }
    if (s.length === 5 && s[4] === "run") {
      const kind = q.get("kind") || "deploy";
      // The address is not trusted: a check that is not a run id is dropped, and the flow checks the record again (runflow.js).
      const check = q.get("check") || "";
      return kind === "deploy" || kind === "status" ? { kind: "run_unit", env, unit, runKind: kind, check: RUN_ID_RE.test(check) ? check : "" } : NONE;
    }
    if (s.length >= 6 && s[4] === "hosts" && ID_RE.test(s[5])) {
      if (s.length === 6) return { kind: "unit_host", env, unit, host: s[5] };
      if (s.length === 7 && s[6] === "history") return { kind: "history", env, unit, host: s[5], doc: "host_values" };
    }
  }
  return NONE;
}
