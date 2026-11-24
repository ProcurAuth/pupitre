// The tab Hosts of an environment (5.9, 5.21), and the form that creates a host (?tab=hosts&new=1).

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { el } from "../ui.js";
import { loadingBlock, searchBox, toolButton } from "../lib/dom.js";
import { emptyBlock, invalidRow, linkCell, listTable, matches, nodeCell, textCell } from "../lib/list_table.js";

export async function mountTab({ ctx, env, container, create, controller }) {
  if (create) {
    const mod = await import("./host.js");
    return mod.mountCreate(ctx, env, container);
  }
  container.append(loadingBlock());
  let items;
  try {
    items = (await api.listAll(api.paths.hosts(env), {}, { signal: controller.signal })).items;
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error);
    return {};
  }
  let query = "";
  const list = el("div");
  const search = searchBox({ placeholder: "Search hosts", onInput: (text) => { query = text; render(); }, testid: "list-search" });
  container.replaceChildren(el("div", { class: "toolbar-row" }, search.el), list);
  ctx.header.setActions(toolButton("New host", "plus", { primary: true, collapse: true, testid: "new-host", onClick: () => ctx.navigate(`${pageUrl("e", env)}?tab=hosts&new=1`) }));

  function render() {
    list.replaceChildren();
    const doc = (h) => h.document || {};
    const shown = items.filter((h) => matches(query, h.id, doc(h).address, ...(doc(h).labels || [])));
    if (!items.length) return list.append(emptyBlock({ title: "No host", text: "A host is a machine of this environment.", testid: "list-empty" }));
    if (!shown.length) return list.append(emptyBlock({ title: "No match", testid: "list-empty" }));
    const table = listTable({ layout: "hosts", columns: [{ title: "Id" }, { title: "Address" }, { title: "Port", cls: "num hide-narrow" }, { title: "Labels" }, { title: "Version", cls: "num hide-narrow" }], label: "Hosts" });
    table.setRows(shown.map((h) => (h.errors && h.errors.length
      ? invalidRow(h.id, h.errors, 5, `row-${h.id}`)
      : el("tr", { dataset: { testid: `row-${h.id}` } },
        linkCell(h.id, pageUrl("e", env, "hosts", h.id)),
        textCell(doc(h).address, "cell-mono"),
        textCell(doc(h).port, "num hide-narrow"),
        nodeCell(el("span", { class: "cell-tags__list" }, ...((doc(h).labels || []).map((l) => el("span", { class: "tag", text: l })))), "cell-tags"),
        textCell(h.version, "num hide-narrow")))));
    list.append(table.el);
  }
  render();
  return {};
}
