// The tab Units of an environment (5.10, 5.21), and the form that creates a unit (?tab=units&new=1).

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { fillErrorBox, notifyError } from "../errors.js";
import { el } from "../ui.js";
import { loadCatalog, playbooksOf } from "../lib/catalog.js";
import { field, loadingBlock, pageHead, searchBox, textInput, toolButton } from "../lib/dom.js";
import { createHostbox } from "../lib/hostbox.js";
import { emptyBlock, invalidRow, linkCell, listTable, matches, textCell } from "../lib/list_table.js";
import { checkId, checkLabelText } from "../validate.js";

export async function mountTab({ ctx, env, container, create, controller }) {
  if (create) return mountCreate({ ctx, env, container, controller });
  container.append(loadingBlock());
  let items;
  try {
    items = (await api.listAll(api.paths.units(env), {}, { signal: controller.signal })).items;
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error);
    return {};
  }
  let query = "";
  const list = el("div");
  const search = searchBox({ placeholder: "Search units", onInput: (text) => { query = text; render(); }, testid: "list-search" });
  container.replaceChildren(el("div", { class: "toolbar-row" }, search.el), list);
  ctx.header.setActions(toolButton("New unit", "plus", { primary: true, collapse: true, testid: "new-unit", onClick: () => ctx.navigate(`${pageUrl("e", env)}?tab=units&new=1`) }));

  function render() {
    list.replaceChildren();
    const shown = items.filter((u) => matches(query, u.label, u.id, u.schema));
    if (!items.length) return list.append(emptyBlock({ title: "No unit", text: "A unit is one service on a list of hosts.", testid: "list-empty" }));
    if (!shown.length) return list.append(emptyBlock({ title: "No match", testid: "list-empty" }));
    const table = listTable({ layout: "units", columns: [{ title: "Name" }, { title: "Id" }, { title: "Schema" }, { title: "Hosts", cls: "num hide-narrow" }, { title: "Version", cls: "num hide-narrow" }], label: "Units" });
    table.setRows(shown.map((u) => (u.errors && u.errors.length
      ? invalidRow(u.id, u.errors, 5, `row-${u.id}`)
      : el("tr", { dataset: { testid: `row-${u.id}` } },
        linkCell(u.label || u.id, pageUrl("e", env, "units", u.id)),
        textCell(u.id, "cell-mono cell-muted"),
        textCell(u.schema, "cell-mono"),
        textCell(Array.isArray(u.hosts) ? u.hosts.length : u.hosts, "num hide-narrow"),
        textCell(u.version, "num hide-narrow")))));
    list.append(table.el);
  }
  render();
  return {};
}

// The form of a new unit: the id, the name, the schema, the playbooks and the hosts. The playbooks are picked in the catalog (CTRL-35).
// A server that runs with runner.enabled = false has no playbook in its catalog: the form then asks for the ids (2.9).
async function mountCreate({ ctx, env, container, controller }) {
  const [catalog, hostItems] = await Promise.all([loadCatalog(controller.signal), api.listAll(api.paths.hosts(env), {}, { signal: controller.signal }).then((r) => r.items)]);
  const withCatalog = Array.isArray(catalog.playbooks);
  const idInput = textInput({ maxlength: 63, dataset: { testid: "id" } });
  const labelInput = textInput({ maxlength: 64, dataset: { testid: "label" } });
  const schemaSelect = el("select", { class: "input", dataset: { testid: "schema" } }, el("option", { value: "", text: "Choose a schema", disabled: true, selected: true }), ...(catalog.schemas || []).map((s) => el("option", { value: s.id, text: s.id })));
  const pbControl = (kind, required) => {
    if (withCatalog) {
      return el("select", { class: "input", dataset: { testid: `playbook-${kind}` } },
        el("option", { value: "", text: required ? "Choose a playbook" : "None", disabled: required, selected: true }),
        ...playbooksOf(catalog, kind).map((p) => el("option", { value: p.id, text: p.id })));
    }
    return textInput({ maxlength: 63, dataset: { testid: `playbook-${kind}` } });
  };
  const deploy = pbControl("deploy", true);
  const render = pbControl("render", false);
  const status = pbControl("status", false);
  const idField = field({ label: "Id", control: idInput, hint: "The id is in the address of the unit. You cannot change it." });
  const labelField = field({ label: "Name", control: labelInput });
  const schemaField = field({ label: "Schema", control: schemaSelect, hint: "The schema gives the keys of the unit. You cannot change it later." });
  const deployField = field({ label: "Deploy playbook", control: deploy });
  const renderField = field({ label: "Render playbook", control: render, hint: "The check runs it first. It must not connect to a host." });
  const statusField = field({ label: "Status playbook", control: status });
  const hostbox = createHostbox({ hosts: hostItems.map((h) => ({ id: h.id, address: h.document && h.document.address })), selected: [], onChange: sync });
  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  const submit = el("button", { type: "button", class: "btn btn--primary", text: "Create", disabled: true, dataset: { testid: "create" } });
  const cancel = el("a", { class: "btn", href: pageUrl("e", env), text: "Cancel" });
  let touched = false;

  function document_() {
    const playbooks = { deploy: deploy.value };
    if (render.value) playbooks.render = render.value;
    if (status.value) playbooks.status = status.value;
    return { format: 1, label: labelInput.value, schema: schemaSelect.value, playbooks, hosts: hostbox.selected() };
  }
  function problems() {
    const out = [];
    const idProblem = checkId(idInput.value, "id");
    const labelProblem = checkLabelText(labelInput.value, "name");
    if (idProblem) out.push(["id", idProblem]);
    if (labelProblem) out.push(["label", labelProblem]);
    if (!schemaSelect.value) out.push(["schema", "schema: choose a schema"]);
    if (!deploy.value) out.push(["deploy", "deploy: a unit needs a deploy playbook"]);
    if (!withCatalog) {
      for (const [name, control] of [["deploy", deploy], ["render", render], ["status", status]]) {
        if (control.value && checkId(control.value, name)) out.push([name, checkId(control.value, name)]);
      }
    }
    return out;
  }
  function sync() {
    const p = problems();
    submit.disabled = p.length > 0;
    if (touched) {
      const find = (name) => (p.find(([n]) => n === name) || [null, null])[1];
      idField.setError(find("id"));
      labelField.setError(find("label"));
    }
  }
  for (const control of [idInput, labelInput, schemaSelect, deploy, render, status]) {
    control.addEventListener("input", () => { touched = true; sync(); });
    control.addEventListener("change", () => { touched = true; sync(); });
  }
  submit.addEventListener("click", async () => {
    touched = true;
    sync();
    if (problems().length) return;
    submit.disabled = true;
    errors.hidden = true;
    const id = idInput.value;
    try {
      await api.putDoc(api.paths.unit(env, id), document_(), { etag: null });
      await ctx.navigate(pageUrl("e", env, "units", id));
    } catch (error) {
      if (api.isAbort(error)) return;
      if (error.code === "already_exists") idField.setError("A unit with this id exists.");
      else if (error.code === "invalid_document" || error.code === "limit_exceeded") fillErrorBox(errors, error);
      else notifyError(error);
      sync();
    }
  });
  container.append(el("div", { class: "form-page" }, pageHead({ title: "New unit", id: env }),
    el("div", { class: "form-grid" }, idField.el, labelField.el, schemaField.el),
    el("div", { class: "form-grid" }, deployField.el, renderField.el, statusField.el),
    el("div", { class: "field" }, el("span", { class: "field__label", text: "Hosts" }), hostbox.el),
    errors, el("div", { class: "toolbar-row" }, submit, cancel)));
  ctx.setGuard(() => !!(idInput.value || labelInput.value));
  idInput.focus();
  return {};
}
