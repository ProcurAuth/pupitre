// The page Environments (5.21): the environments that the subject sees, with a search. Creation is the same page with ?new=1.

import * as api from "../api.js";
import { fillErrorBox, notifyError } from "../errors.js";
import { clear, el } from "../ui.js";
import { field, loadingBlock, searchBox, textInput, toolButton } from "../lib/dom.js";
import { emptyBlock, linkCell, listTable, matches, nodeCell, invalidRow, textCell } from "../lib/list_table.js";
import { checkId, checkLabelText } from "../validate.js";
import { pageUrl } from "../config.js";

export async function mount(ctx, route) {
  return route.create ? mountCreate(ctx) : mountList(ctx);
}

async function mountList(ctx) {
  const { main, header } = ctx;
  const controller = new AbortController();
  document.title = "Environments - Pupitre";
  header.setCrumbs([{ text: "Environments" }]);
  const page = el("section", { class: "list-page", dataset: { layout: "environments-page" } }, loadingBlock());
  main.append(page);
  let items = [];
  let query = "";

  const search = searchBox({ placeholder: "Search environments", onInput: (text) => { query = text; render(); }, testid: "list-search" });
  const create = toolButton("New environment", "plus", { primary: true, collapse: true, testid: "new-environment", onClick: () => ctx.navigate("?new=1") });

  function render() {
    clear(page);
    const shown = items.filter((item) => matches(query, item.label, item.id));
    if (!items.length) {
      page.append(emptyBlock({ title: "No environment", text: "An environment holds hosts and units.", actions: [toolButton("New environment", "plus", { primary: true, onClick: () => ctx.navigate("?new=1") })], testid: "list-empty" }));
      return;
    }
    if (!shown.length) {
      page.append(emptyBlock({ title: "No match", text: "No environment has this text.", testid: "list-empty" }));
      return;
    }
    const table = listTable({ layout: "environments", columns: [{ title: "Name" }, { title: "Id" }, { title: "Version", cls: "num hide-narrow" }], label: "Environments" });
    table.setRows(shown.map((item) => (item.errors && item.errors.length
      ? invalidRow(item.id, item.errors, 3, `row-${item.id}`)
      : el("tr", { dataset: { testid: `row-${item.id}` } },
        linkCell(item.label || item.id, pageUrl("e", item.id)),
        textCell(item.id, "cell-mono cell-muted"),
        textCell(item.version, "num hide-narrow")))));
    page.append(table.el);
  }

  header.setActions(search.el, create);
  try {
    const result = await api.listAll(api.paths.environments(), {}, { signal: controller.signal });
    items = result.items;
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  render();
  return { destroy: () => controller.abort() };
}

function mountCreate(ctx) {
  const { main, header } = ctx;
  document.title = "New environment - Pupitre";
  header.setCrumbs([{ text: "Environments", href: "." }, { text: "New environment" }]);
  const idInput = textInput({ maxlength: 63, dataset: { testid: "id" } });
  const labelInput = textInput({ maxlength: 64, dataset: { testid: "label" } });
  const idField = field({ label: "Id", control: idInput, hint: "The id is in the address of the environment. You cannot change it." });
  const labelField = field({ label: "Name", control: labelInput });
  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  const submit = el("button", { type: "button", class: "btn btn--primary", text: "Create", disabled: true, dataset: { testid: "create" } });
  const cancel = el("a", { class: "btn", href: ".", text: "Cancel" });
  let touched = { id: false, label: false };

  function check() {
    const idError = touched.id || idInput.value ? checkId(idInput.value, "id") : null;
    const labelError = touched.label || labelInput.value ? checkLabelText(labelInput.value, "name") : null;
    idField.setError(idError);
    labelField.setError(labelError);
    submit.disabled = !!(checkId(idInput.value, "id") || checkLabelText(labelInput.value, "name"));
  }
  idInput.addEventListener("input", () => { touched.id = true; check(); });
  labelInput.addEventListener("input", () => { touched.label = true; check(); });
  submit.addEventListener("click", async () => {
    submit.disabled = true;
    errors.hidden = true;
    try {
      const id = idInput.value;
      await api.request("POST", api.paths.environments(), { body: { id, document: { format: 1, label: labelInput.value } } });
      await ctx.navigate(pageUrl("e", id));
    } catch (error) {
      if (api.isAbort(error)) return;
      if (error.code === "already_exists") idField.setError("An environment with this id exists.");
      else if (error.code === "invalid_document") fillErrorBox(errors, error);
      else notifyError(error);
      check();
    }
  });
  main.append(el("section", { class: "page" }, el("div", { class: "form-page" },
    el("div", { class: "page__head" }, el("h1", { class: "page__title", text: "New environment" })),
    el("div", { class: "form-grid" }, idField.el, labelField.el), errors, el("div", { class: "toolbar-row" }, submit, cancel))));
  ctx.setGuard(() => !!(idInput.value || labelInput.value));
  idInput.focus();
  return { destroy() {} };
}
