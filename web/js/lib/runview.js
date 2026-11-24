// The view of one run (5.16, 5.17, 5.21): the facts of the record, one row for each host, the events and the output.
// The view asks again each second while the run is not final (5.17): the record, the events after the last number and the output from the
// last offset. It never holds a request open. The events and the output are plain text: the page shows each control character as a
// visible symbol (5.22, js/ctl.js) and builds every element with createElement and textContent.

import * as api from "../api.js";
import { appendText } from "../ctl.js";
import { reasonText } from "../errors.js";
import { bytesText, localTime } from "../format.js";
import { button, el, icon } from "../ui.js";
import { pageUrl } from "../config.js";
import { poller } from "./poll.js";
import { RUN_STATUS, isFinal } from "./runs_list.js";
import { statusDot, timeEl } from "./dom.js";

const OUTPUT_LIMIT = 1048576;
const RESULT_DOT = { ok: "ok", changed: "info", skipped: "wait", failed: "error", unreachable: "warn" };
const COUNTS = ["ok", "changed", "failed", "skipped", "unreachable"];

// The counts of one host from the events: `ok` counts the changed tasks too, as the stats of Ansible do.
export function tally(events) {
  const hosts = new Map();
  for (const ev of events) {
    if (ev.kind !== "host_result" || typeof ev.host !== "string") continue;
    if (!hosts.has(ev.host)) hosts.set(ev.host, { ok: 0, changed: 0, failed: 0, skipped: 0, unreachable: 0 });
    const c = hosts.get(ev.host);
    if (ev.status === "changed") {
      c.ok += 1;
      c.changed += 1;
    } else if (COUNTS.includes(ev.status)) c[ev.status] += 1;
  }
  return hosts;
}

// A node path as text, with a break point after each "/": a long path wraps at a segment and never in the middle of an id.
function nodeText(path) {
  const span = el("span", { class: "mono" });
  String(path || "").split("/").forEach((part, i) => {
    if (i) span.append("/", document.createElement("wbr"));
    span.append(part);
  });
  return span;
}

export function createRunView({ ctx, runId, signal, onFinal = () => {}, onRecord = () => {}, compact = false }) {
  const limits = ctx.session.limits;
  const root = el("div", { class: "run", dataset: { testid: "run-view", runId } });
  const facts = el("dl", { class: "facts", dataset: { testid: "run-facts" } });
  const notes = el("div", { class: "run__notes" });
  const hostBox = el("div", { class: "card-scroll", dataset: { testid: "run-hosts" } });
  const evBody = el("div", { class: "evlist", role: "log", "aria-label": "Events of the run", tabindex: "0", dataset: { testid: "run-events" } });
  const logCount = el("span", { class: "note-line", dataset: { testid: "output-length" } });
  const log = el("pre", { class: "logview logs", tabindex: "0", role: "log", "aria-label": "Output of the run", dataset: { testid: "run-output" } }, el("span", { class: "logview__empty", text: "No output yet." }));
  root.append(facts, notes, hostBox,
    el("div", { class: "run__cols" },
      el("div", { class: "run__col" }, el("div", { class: "page__section-head" }, el("h2", { class: "page__section-title", text: "Events" })), evBody),
      el("div", { class: "run__col" }, el("div", { class: "page__section-head" }, el("h2", { class: "page__section-title", text: "Output" }), logCount), log)));

  let record = null;
  const events = [];
  let lastSeq = 0;
  let eventsComplete = false;
  let offset = 0;
  let outputComplete = false;
  let firstChunk = true;
  let finalNotified = false;
  const decoder = new TextDecoder("utf-8", { fatal: false });
  const open = new Set();
  let destroyed = false;

  // --- the facts and the hosts ---------------------------------------------------------------------------------------------------------

  function factItem(name, ...content) {
    return el("div", { class: "facts__item" }, el("dt", { text: name }), el("dd", {}, ...content));
  }

  function renderFacts() {
    const r = record;
    const info = RUN_STATUS[r.status] || { dot: "wait", label: r.status };
    const reason = reasonText(r.reason);
    const link = (id) => el("a", { class: "mono", href: pageUrl("runs", id), text: id });
    const items = [
      factItem("Status", el("span", { class: "dot-line" }, statusDot(info.dot, info.label), timeEl(r.finished_at || r.started_at || r.created_at), reason ? el("span", { class: "muted", text: reason }) : null)),
      factItem("Kind", `${r.kind}, ${r.mode}${r.command ? `, ${r.command}` : ""}`),
      factItem("Node", nodeText(r.node)),
      factItem("Started by", r.subject),
      r.check_run ? factItem("Check", link(r.check_run)) : null,
      r.applied_by ? factItem("Applied by", link(r.applied_by)) : null,
      r.started_at ? factItem("Started", el("time", { datetime: r.started_at, title: r.started_at, text: localTime(r.started_at) })) : null,
      r.finished_at ? factItem("Finished", el("time", { datetime: r.finished_at, title: r.finished_at, text: localTime(r.finished_at) })) : null,
      r.exit_code !== undefined ? factItem("Exit code", String(r.exit_code)) : null,
      r.snapshot ? factItem("Snapshot", el("span", { class: "mono", text: r.snapshot.slice(0, 16), title: r.snapshot })) : null,
      r.params && Object.keys(r.params).length ? factItem("Parameters", el("span", { class: "mono", text: Object.entries(r.params).map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`).join(", ") })) : null,
      r.message ? factItem("Message", r.message) : null,
    ];
    facts.replaceChildren(...items.filter(Boolean));
    notes.replaceChildren();
    if (r.output_truncated) notes.append(note("The output is cut. It is above the bound of the runner. The runner node holds the full output."));
    if (r.events_truncated) notes.append(note("The events are cut. They are above the bound of the runner."));
  }

  function note(text) {
    return el("div", { class: "callout callout--warning" }, icon("warning", 16), el("span", { class: "callout__text", text }));
  }

  function renderHosts() {
    const r = record;
    const counts = r.stats && typeof r.stats === "object" ? new Map(Object.entries(r.stats)) : tally(events);
    const final = isFinal(r.status);
    const rows = r.hosts.map((h) => {
      const c = counts.get(h) || null;
      let dot = "wait";
      let label = "No result yet";
      if (c) {
        if (c.failed > 0) { dot = "error"; label = "Failed"; }
        else if (c.unreachable > 0) { dot = "warn"; label = "Unreachable"; }
        else if (final && r.status === "passed") { dot = "ok"; label = "Passed"; }
        else if (final) { dot = "wait"; label = "No result"; }
        else { dot = "run"; label = "Started"; }
      } else if (!final && r.status === "running") { dot = "run"; label = "Started"; }
      const cell = (name) => {
        const n = c ? c[name] || 0 : null;
        const cls = n === null ? "num zero" : n === 0 ? "num zero" : name === "failed" || name === "unreachable" ? "num bad" : name === "changed" ? "num good" : "num";
        return el("td", { class: cls, text: n === null ? "-" : String(n) });
      };
      return el("tr", { dataset: { testid: `host-${h}`, host: h } }, el("td", { class: "mono", text: h }), el("td", {}, statusDot(dot, label)), ...COUNTS.map(cell));
    });
    hostBox.replaceChildren(el("table", { class: "table hosttable", "aria-label": "Hosts of the run" },
      el("thead", {}, el("tr", {}, el("th", { text: "Host" }), el("th", {}, el("span", { class: "visually-hidden", text: "Status" })), ...COUNTS.map((n) => el("th", { class: "num", text: n })))),
      el("tbody", {}, ...rows)));
  }

  // --- the events ---------------------------------------------------------------------------------------------------------------------------

  function eventRow(ev) {
    const detailText = ev.kind === "host_result" ? [ev.message, ev.diff].filter((x) => typeof x === "string").join("\n") : "";
    const hasDetail = detailText !== "" || ev.omitted === true;
    const phase = el("span", { class: "evrow__phase", text: ev.phase || "" });
    const dot = el("span");
    const host = el("span", { class: "evrow__host" });
    const text = el("span", { class: "evrow__text" });
    let label;
    switch (ev.kind) {
      case "playbook_start": label = `PLAYBOOK ${ev.playbook || ""}${ev.check ? " (check)" : ""}`; break;
      case "play_start": label = `PLAY ${ev.play || ""}`; break;
      case "task_start": label = `TASK ${ev.task || ""}`; break;
      case "host_result":
        dot.append(statusDot(RESULT_DOT[ev.status] || "wait", ev.status || "result"));
        appendText(host, ev.host || "");
        label = `${ev.task || ""}${typeof ev.message === "string" ? ` - ${ev.message.split("\n")[0]}` : ""}`;
        break;
      case "stats": label = "Result of the playbook"; break;
      case "invalid": label = `An event that is not valid (${ev.length} bytes)`; break;
      default: label = String(ev.kind || "event");
    }
    appendText(text, label);
    const more = hasDetail ? button("Show the details", { "aria-expanded": "false", class: "btn btn--ghost btn--icon btn--sm evrow__more", dataset: { testid: `event-more-${ev.seq}` } }, "chevron-down", { iconOnly: true, variant: "ghost", size: "sm" }) : el("span");
    const row = el("div", { class: "evrow", dataset: { seq: ev.seq, kind: ev.kind } }, phase, dot, host, text, more);
    const wrap = el("div", { class: "evitem" }, row);
    if (hasDetail) {
      let detail = null;
      more.className = "btn btn--ghost btn--icon btn--sm evrow__more";
      more.addEventListener("click", () => {
        if (detail) {
          detail.remove();
          detail = null;
          row.dataset.open = "false";
          more.setAttribute("aria-expanded", "false");
          open.delete(ev.seq);
          return;
        }
        const pre = el("pre", { class: "evdetail__text" });
        appendText(pre, detailText !== "" ? detailText : "The text is above 16 KiB. The plugin did not store it.");
        detail = el("div", { class: "evdetail" }, pre);
        wrap.append(detail);
        row.dataset.open = "true";
        more.setAttribute("aria-expanded", "true");
        open.add(ev.seq);
      });
    }
    return wrap;
  }

  function appendEvents(items) {
    const stick = evBody.scrollTop + evBody.clientHeight >= evBody.scrollHeight - 24;
    const frag = document.createDocumentFragment();
    for (const ev of items) frag.append(eventRow(ev));
    evBody.append(frag);
    if (stick) evBody.scrollTop = evBody.scrollHeight;
  }

  // --- the output ---------------------------------------------------------------------------------------------------------------------------

  function appendOutput(bytes) {
    if (!bytes.byteLength) return;
    if (firstChunk) {
      log.replaceChildren();
      firstChunk = false;
    }
    const stick = log.scrollTop + log.clientHeight >= log.scrollHeight - 24;
    appendText(log, decoder.decode(bytes, { stream: true }));
    if (stick) log.scrollTop = log.scrollHeight;
  }

  // --- the poll ------------------------------------------------------------------------------------------------------------------------------

  async function fetchRecord() {
    record = await api.request("GET", api.paths.run(runId), { signal });
    renderFacts();
    onRecord(record);
  }

  async function fetchEvents() {
    for (let guard = 0; guard < 200 && !eventsComplete && !destroyed; guard += 1) {
      const page = await api.request("GET", `${api.paths.events(runId)}${api.query({ after: lastSeq, limit: limits.max_page_items })}`, { signal });
      const items = page.items || [];
      if (items.length) {
        events.push(...items);
        appendEvents(items);
        lastSeq = page.last_seq !== undefined ? page.last_seq : items[items.length - 1].seq;
      }
      if (page.complete) eventsComplete = true;
      if (items.length < limits.max_page_items) break;
    }
  }

  async function fetchOutput() {
    for (let guard = 0; guard < 64 && !outputComplete && !destroyed; guard += 1) {
      const response = await api.request("GET", `${api.paths.output(runId)}${api.query({ offset, limit: OUTPUT_LIMIT })}`, { signal, raw: true, headers: { Accept: "application/octet-stream" } });
      const length = Number(response.headers.get("X-Pupitre-Output-Length"));
      const complete = response.headers.get("X-Pupitre-Run-Complete") === "true";
      const bytes = new Uint8Array(await response.arrayBuffer());
      appendOutput(bytes);
      offset += bytes.byteLength;
      if (Number.isFinite(length)) logCount.textContent = bytesText(length);
      if (complete && offset >= length) outputComplete = true;
      if (offset >= length) break;
    }
  }

  async function tick(skipRecord = false) {
    try {
      if (!skipRecord && (!record || !isFinal(record.status))) await fetchRecord();
      await fetchEvents();
      await fetchOutput();
      if (record) renderHosts();
      // A final run keeps one more round so that the last events and the last bytes arrive.
      if (record && isFinal(record.status) && eventsComplete && outputComplete) {
        poll.stop();
        if (!finalNotified) {
          finalNotified = true;
          onFinal(record);
        }
      }
    } catch (error) {
      if (!api.isAbort(error)) {
        if (error && error.status === 404) poll.stop();
        notes.replaceChildren(el("div", { class: "callout callout--error" }, icon("error", 16), el("span", { class: "callout__text", text: "The page cannot read the run. It tries again." })));
        return;
      }
    }
    if (notes.querySelector(".callout--error")) notes.replaceChildren();
  }

  const poll = poller(() => tick(), 1000);

  return {
    el: root,
    record: () => record,
    // The first read: the record, the events and the output. It throws the error of the first record (a 404 for an unknown run).
    async start() {
      await fetchRecord();
      renderHosts();
      await tick(true);
      if (!(record && isFinal(record.status) && eventsComplete && outputComplete)) poll.start();
    },
    refresh: () => tick(),
    destroy() {
      destroyed = true;
      poll.stop();
    },
  };
}
