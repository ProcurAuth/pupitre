// The list of runs, newest first (5.16, 5.21): the status, the kind, the node, the count of hosts, the subject and the time.
// The list loads one page at a time; the end of the list loads the next page. While a run is not final, the page asks again every 3 seconds.

import * as api from "../api.js";
import { notifyError } from "../errors.js";
import { shortAgo, localTime } from "../format.js";
import { el } from "../ui.js";
import { emptyBlock, linkCell, listTable, nodeCell, textCell } from "./list_table.js";
import { statusDot } from "./dom.js";
import { poller } from "./poll.js";

export const RUN_STATUS = {
  queued: { dot: "wait", label: "Queued" },
  running: { dot: "run", label: "Started" },
  passed: { dot: "ok", label: "Passed" },
  failed: { dot: "error", label: "Failed" },
  canceled: { dot: "warn", label: "Canceled" },
  lost: { dot: "warn", label: "Lost" },
};
export const isFinal = (status) => ["passed", "failed", "canceled", "lost"].includes(status);

export const kindText = (run) => (run.kind === "command" ? `command ${run.command || ""}`.trim() : `${run.kind}`) + (run.mode === "check" ? ", check" : run.kind === "status" ? "" : ", apply");

export function statusCell(run) {
  const info = RUN_STATUS[run.status] || { dot: "wait", label: run.status };
  const when = run.finished_at || run.started_at || run.created_at;
  return el("td", { class: "cell-status" }, statusDot(info.dot, info.label), el("time", { datetime: when, title: `${info.label}. ${localTime(when)} (server time ${when})`, text: shortAgo(when) }));
}

// opts: { ctx, container, filters: { environment, status }, controls: [nodes], onFilterChange }
export function createRunsList({ ctx, container, filters, signal }) {
  const items = new Map();
  let next = null;
  let loading = false;
  let ended = false;
  const wrap = el("div", { class: "list-block", dataset: { layout: "runs-page" } });
  container.append(wrap);
  const sentinel = el("div", { class: "list-more", dataset: { testid: "runs-more" } });
  const observer = typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => { if (entries.some((e) => e.isIntersecting)) loadMore(); }) : null;

  function params(after) {
    return { environment: filters.environment || undefined, status: filters.status || undefined, limit: Math.min(100, ctx.session.limits.max_page_items), after: after || undefined };
  }

  function render() {
    wrap.replaceChildren();
    const rows = [...items.values()].sort((a, b) => (a.id < b.id ? 1 : -1));
    if (!rows.length) {
      wrap.append(emptyBlock({ title: filters.environment || filters.status ? "No run matches" : "No run", text: filters.environment || filters.status ? "Change the filters to see other runs." : "A run starts when a person starts a check, an apply or a command.", testid: "runs-empty" }));
      return;
    }
    const table = listTable({ layout: "runs", columns: [{ title: "Status" }, { title: "Run" }, { title: "Node" }, { title: "Hosts", cls: "num hide-narrow" }, { title: "By", cls: "hide-narrow" }, { title: "Started", cls: "num hide-narrow" }], label: "Runs" });
    table.setRows(rows.map((run) => el("tr", { dataset: { testid: `run-${run.id}`, status: run.status } },
      statusCell(run),
      linkCell(kindText(run), `runs/${encodeURIComponent(run.id)}`, { cls: "cell-title" }),
      textCell(run.node, "cell-mono cell-muted"),
      textCell(Array.isArray(run.hosts) ? run.hosts.length : run.hosts, "num hide-narrow"),
      textCell(run.subject, "hide-narrow"),
      el("td", { class: "num cell-muted hide-narrow", text: localTime(run.created_at), title: run.created_at }))));
    wrap.append(table.el);
    if (next) {
      wrap.append(sentinel);
      if (observer) {
        observer.disconnect();
        observer.observe(sentinel);
      }
    } else if (observer) observer.disconnect();
  }

  async function loadFirst() {
    try {
      const page = await api.request("GET", `${api.paths.runs()}${api.query(params(null))}`, { signal });
      items.clear();
      for (const run of page.items || []) items.set(run.id, run);
      next = page.next || null;
      ended = !next;
      render();
    } catch (error) {
      if (!api.isAbort(error)) {
        if (!items.size) ctx.pageError(error, () => loadFirst());
        else notifyError(error, "runs");
      }
    }
  }

  async function loadMore() {
    if (loading || !next || ended) return;
    loading = true;
    try {
      const page = await api.request("GET", `${api.paths.runs()}${api.query(params(next))}`, { signal });
      for (const run of page.items || []) items.set(run.id, run);
      next = page.next || null;
      render();
    } catch (error) {
      notifyError(error, "runs");
    } finally {
      loading = false;
    }
  }

  // The poll asks again for the first page and merges it. A page that was loaded later stays.
  async function refresh() {
    try {
      const page = await api.request("GET", `${api.paths.runs()}${api.query(params(null))}`, { signal });
      let changed = false;
      for (const run of page.items || []) {
        const old = items.get(run.id);
        if (!old || old.status !== run.status || old.finished_at !== run.finished_at) changed = true;
        items.set(run.id, run);
      }
      if (changed) render();
    } catch (error) {
      if (!api.isAbort(error)) notifyError(error, "runs");
    }
  }

  const poll = poller(() => {
    if ([...items.values()].some((run) => !isFinal(run.status))) return refresh();
    return undefined;
  }, 3000);

  return {
    load: loadFirst,
    setFilters(next) {
      Object.assign(filters, next);
      return loadFirst();
    },
    start: () => poll.start(),
    destroy() {
      poll.stop();
      if (observer) observer.disconnect();
    },
  };
}
