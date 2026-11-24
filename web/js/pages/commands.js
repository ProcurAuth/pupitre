// The page Commands (5.21): the commands of the catalog (4.32). A person launches a command by hand. The page lists all commands of
// the configuration file; the server refuses a launch that the subject cannot do and the page shows the action (5.22).

import { pageUrl } from "../config.js";
import { el } from "../ui.js";
import { loadCatalog } from "../lib/catalog.js";
import { loadingBlock, searchBox } from "../lib/dom.js";
import { emptyBlock, linkCell, listTable, matches, textCell } from "../lib/list_table.js";
import * as api from "../api.js";

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const env = route.env;
  document.title = "Commands - Pupitre";
  header.setCrumbs([{ text: "Environments", href: "." }, { text: env, href: pageUrl("e", env) }, { text: "Commands" }]);
  main.append(loadingBlock());
  let catalog;
  try {
    catalog = await loadCatalog(controller.signal);
    if (!(await api.getDoc(api.paths.environment(env), { signal: controller.signal }))) throw Object.assign(new Error("not found"), { name: "ApiError", status: 404 });
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  const commands = catalog.commands || [];
  let query = "";
  const list = el("div", { class: "list-block", dataset: { layout: "commands-page" } });
  const search = searchBox({ placeholder: "Search commands", onInput: (text) => { query = text; render(); }, testid: "list-search" });
  main.replaceChildren(el("section", { class: "page" }, el("div", { class: "toolbar-row" }, search.el), list));

  function render() {
    list.replaceChildren();
    const shown = commands.filter((c) => matches(query, c.label, c.id, c.target));
    if (!commands.length) return list.append(emptyBlock({ title: "No command", text: "The configuration file of the server lists no command.", testid: "list-empty" }));
    if (!shown.length) return list.append(emptyBlock({ title: "No match", testid: "list-empty" }));
    const table = listTable({ layout: "commands", columns: [{ title: "Name" }, { title: "Id", cls: "hide-narrow" }, { title: "Target" }, { title: "Check", cls: "hide-narrow" }], label: "Commands" });
    table.setRows(shown.map((c) => el("tr", { dataset: { testid: `command-${c.id}` } },
      linkCell(c.label, pageUrl("e", env, "commands", c.id)),
      textCell(c.id, "cell-mono cell-muted hide-narrow"),
      textCell(c.target, "cell-mono"),
      textCell(c.check ? "Check first" : "At once", "cell-muted hide-narrow"))));
    list.append(table.el);
  }
  render();
  return { destroy: () => controller.abort() };
}
