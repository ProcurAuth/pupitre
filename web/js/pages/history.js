// The page History (5.13, 5.21): the versions of a document, newest first, and the difference of two versions.
// The page picks the two newest versions at first and shows their difference. The list loads 50 versions at a time.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { notifyError } from "../errors.js";
import { renderDiff } from "../diff_view.js";
import { localTime } from "../format.js";
import { badge, el, segmented } from "../ui.js";
import { loadingBlock, pageHead, timeEl } from "../lib/dom.js";
import { emptyBlock } from "../lib/list_table.js";

// The route of the document, the title and the crumbs for a route of the page.
function describe(route) {
  const { env, unit, host, doc } = route;
  const e = { text: env, href: pageUrl("e", env) };
  const base = [{ text: "Environments", href: "." }, e];
  switch (doc) {
    case "environment": return { path: api.paths.environment(env), what: `environment ${env}`, crumbs: [...base, { text: "History" }], back: pageUrl("e", env) };
    case "common": return { path: api.paths.common(env), what: `common values of ${env}`, crumbs: [...base, { text: "History" }], back: `${pageUrl("e", env)}?tab=common` };
    case "host": return { path: api.paths.host(env, host), what: `host ${host}`, crumbs: [...base, { text: host, href: pageUrl("e", env, "hosts", host) }, { text: "History" }], back: pageUrl("e", env, "hosts", host) };
    case "unit": return { path: api.paths.unit(env, unit), what: `definition of ${unit}`, crumbs: [...base, { text: unit, href: pageUrl("e", env, "units", unit) }, { text: "History" }], back: pageUrl("e", env, "units", unit) };
    case "values": return { path: api.paths.values(env, unit), what: `values of ${unit}`, crumbs: [...base, { text: unit, href: pageUrl("e", env, "units", unit) }, { text: "History" }], back: pageUrl("e", env, "units", unit) };
    default: return { path: api.paths.hostValues(env, unit, host), what: `values of ${unit} on ${host}`, crumbs: [...base, { text: unit, href: pageUrl("e", env, "units", unit) }, { text: host, href: pageUrl("e", env, "units", unit, "hosts", host) }, { text: "History" }], back: pageUrl("e", env, "units", unit, "hosts", host) };
  }
}

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const signal = controller.signal;
  const info = describe(route);
  document.title = `History of the ${info.what} - Pupitre`;
  header.setCrumbs(info.crumbs);
  main.append(loadingBlock());
  const PAGE = Math.min(50, ctx.session.limits.max_page_items);
  const versions = [];
  let next = null;
  const picked = [];
  let first;
  try {
    first = await api.request("GET", `${info.path}/history${api.query({ limit: PAGE })}`, { signal });
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  versions.push(...first.items);
  next = first.next || null;
  main.replaceChildren();

  const tools = [];
  if (route.doc === "environment" || route.doc === "common" || route.doc === "values" || route.doc === "unit") {
    const items = route.doc === "environment" || route.doc === "common"
      ? [{ value: "environment", label: "Environment", href: pageUrl("e", route.env, "history") }, { value: "common", label: "Common values", href: `${pageUrl("e", route.env, "history")}?doc=common` }]
      : [{ value: "values", label: "Values", href: pageUrl("e", route.env, "units", route.unit, "history") }, { value: "unit", label: "Definition", href: `${pageUrl("e", route.env, "units", route.unit, "history")}?doc=unit` }];
    const seg = segmented({ label: "Document", items: items.map((i) => ({ value: i.value, label: i.label, testid: `doc-${i.value}` })), value: route.doc, onChange: (value) => ctx.navigate(items.find((i) => i.value === value).href) });
    tools.push(seg.el);
  }
  tools.push(el("a", { class: "btn btn--tb", href: info.back, dataset: { testid: "back" } }, el("span", { class: "btn__label", text: "Back" })));

  const listBox = el("div", { class: "card card-scroll" });
  const moreBox = el("div", { class: "history__foot" });
  const diffBox = el("div", { dataset: { testid: "diff-box" } });
  const compare = el("button", { type: "button", class: "btn btn--primary", text: "Compare", disabled: true, dataset: { testid: "compare" } });
  const hint = el("span", { class: "note-line", text: "Pick two versions." });
  main.append(el("section", { class: "page" }, pageHead({ title: "History", id: info.what, tools }), el("div", { class: "history-wrap" }, listBox, moreBox, el("div", { class: "toolbar-row" }, compare, hint), diffBox)));

  function togglePick(v) {
    const at = picked.indexOf(v);
    if (at >= 0) picked.splice(at, 1);
    else {
      picked.push(v);
      if (picked.length > 2) picked.shift();
    }
    renderList();
  }

  function renderList() {
    listBox.replaceChildren();
    if (!versions.length) {
      listBox.append(emptyBlock({ title: "No version", testid: "history-empty" }));
      return;
    }
    const rows = versions.map((h) => {
      const check = el("input", { type: "checkbox", "aria-label": `Pick the version ${h.version}`, checked: picked.includes(h.version), disabled: h.kind === "delete", dataset: { testid: `pick-${h.version}` }, on: { change: () => togglePick(h.version) } });
      return el("tr", { dataset: { testid: `version-${h.version}` } },
        el("td", { class: "history__pick" }, check),
        el("td", { class: "history__version-cell", text: String(h.version) }),
        el("td", {}, badge(h.kind, h.kind === "delete" ? "error" : null)),
        el("td", { class: "history__by", text: h.saved_by, title: h.saved_by }),
        el("td", { class: "cell-muted" }, timeEl(h.saved_at, { age: false })),
        el("td", { class: "history__message", text: h.message || "-", title: h.message || null }));
    });
    listBox.append(el("table", { class: "table history__table" },
      el("thead", {}, el("tr", {}, el("th", { class: "history__pick" }, el("span", { class: "visually-hidden", text: "Pick" })), el("th", { text: "Version" }), el("th", { text: "Change" }), el("th", { text: "By" }), el("th", { text: "Time" }), el("th", { text: "Message" }))),
      el("tbody", {}, ...rows)));
    compare.disabled = picked.length !== 2;
    hint.textContent = picked.length === 2 ? `Versions ${Math.min(...picked)} and ${Math.max(...picked)}.` : "Pick two versions.";
  }

  function renderMore() {
    moreBox.replaceChildren();
    if (next) {
      const more = el("button", { type: "button", class: "btn", text: "Load older versions", dataset: { testid: "history-more" } });
      more.addEventListener("click", async () => {
        more.disabled = true;
        try {
          const page = await api.request("GET", `${info.path}/history${api.query({ limit: PAGE, before: next })}`, { signal });
          versions.push(...page.items);
          next = page.next || null;
          renderList();
          renderMore();
        } catch (error) {
          if (!api.isAbort(error)) notifyError(error);
          more.disabled = false;
        }
      });
      moreBox.append(more);
    }
  }

  async function showDiff() {
    const [older, newer] = [Math.min(...picked), Math.max(...picked)];
    diffBox.replaceChildren(loadingBlock(2));
    try {
      const [a, b] = await Promise.all([api.getText(`${info.path}/history/${older}`, { signal }), api.getText(`${info.path}/history/${newer}`, { signal })]);
      diffBox.replaceChildren(renderDiff({ text: a.text, name: `version-${older}.json` }, { text: b.text, name: `version-${newer}.json` }, { title: `Version ${older} to version ${newer}` }).el);
    } catch (error) {
      diffBox.replaceChildren();
      if (!api.isAbort(error)) notifyError(error);
    }
  }
  compare.addEventListener("click", showDiff);

  // The two newest versions that have a text are picked at first.
  for (const h of versions) if (h.kind !== "delete" && picked.length < 2) picked.push(h.version);
  renderList();
  renderMore();
  if (picked.length === 2) await showDiff();
  return { destroy: () => controller.abort() };
}
