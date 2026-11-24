// The page Unit (5.21): the values of the unit with their layers, the hosts, the history and the run buttons.
// The same page is the page Host of a unit (/e/{e}/units/{u}/hosts/{h}): the values of the layer host and the effective values on the host.
// The person edits the values of one layer. The effective values and the layer that gives each one come from GET .../effective (5.12).

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { errorText, fillErrorBox, notifyError } from "../errors.js";
import { el } from "../ui.js";
import { loadCatalog, schemaById } from "../lib/catalog.js";
import { showConflict } from "../lib/conflict.js";
import { confirmBox, loadingBlock, pageHead, searchBox, sectionHead, spacer, toolButton } from "../lib/dom.js";
import { createHostbox } from "../lib/hostbox.js";
import { createLayerController } from "../lib/layer_page.js";
import { emptyBlock, linkCell, listTable, nodeCell, textCell } from "../lib/list_table.js";
import { createValuesEditor } from "../lib/values_editor.js";

const effectiveMap = (answer) => new Map((answer.keys || []).map((k) => [k.path, k]));

// The rows of the editor: the keys of the schema of the unit, in the order of the schema, with the definitions of the catalog.
export function unitRows(schema) {
  return schema.keys.map((def) => ({ path: def.path, defs: [def], secret: def.secret === true, description: def.description }));
}

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const { env, unit } = route;
  const host = route.kind === "unit_host" ? route.host : null;
  const layer = host ? "host" : "unit";
  main.append(loadingBlock());
  const signal = controller.signal;
  let catalog;
  let unitDoc;
  let effective;
  let layerDoc;
  try {
    catalog = await loadCatalog(signal);
    unitDoc = await api.getDoc(api.paths.unit(env, unit), { signal });
    if (!unitDoc) throw Object.assign(new Error("not found"), { name: "ApiError", status: 404 });
    if (host && !unitDoc.doc.hosts.includes(host)) throw Object.assign(new Error("not found"), { name: "ApiError", status: 404 });
    const effectivePath = `${api.paths.effective(env, unit)}${api.query({ host })}`;
    const layerPath = host ? api.paths.hostValues(env, unit, host) : api.paths.values(env, unit);
    [effective, layerDoc] = await Promise.all([api.request("GET", effectivePath, { signal }), api.getDoc(layerPath, { signal })]);
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  const schema = schemaById(catalog, unitDoc.doc.schema);
  if (!schema) {
    ctx.pageError({ status: 500, code: "internal_error", message: "The schema of the unit is not in the catalog." });
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren();
  const label = unitDoc.doc.label || unit;
  const runner = ctx.session.runner || {};
  const unitHref = pageUrl("e", env, "units", unit);
  document.title = `${host || label} - Pupitre`;
  header.setCrumbs([{ text: "Environments", href: "." }, { text: env, href: pageUrl("e", env) }, host ? { text: label, href: unitHref } : null, { text: host || label }].filter(Boolean));

  const valuesPath = host ? api.paths.hostValues(env, unit, host) : api.paths.values(env, unit);
  const errorsBox = el("div", { class: "errors", role: "alert", hidden: true });
  const conflictBox = el("div", { class: "conflict-box" });
  const save = toolButton("Save", "save", { primary: true, disabled: true, extra: "edit-save", testid: "save" });
  let eff = effectiveMap(effective);

  async function reloadLayer() {
    const effectivePath = `${api.paths.effective(env, unit)}${api.query({ host })}`;
    const [answer, fresh] = await Promise.all([api.request("GET", effectivePath, { signal }), api.getDoc(valuesPath, { signal })]);
    eff = effectiveMap(answer);
    return { doc: fresh ? fresh.doc : null, etag: fresh ? fresh.etag : null, effective: eff };
  }

  const secretOps = {
    canSet: !host,
    async set(path, value) {
      try {
        await api.request("PUT", api.paths.secret(env, unit, path), { body: { value } });
        const answer = await api.request("GET", `${api.paths.effective(env, unit)}`, { signal });
        eff = effectiveMap(answer);
        editor.setEffective(eff);
      } catch (error) {
        error.userMessage = error.code === "invalid_secret" && error.extra && error.extra.errors && error.extra.errors[0] ? error.extra.errors[0].message : errorText(error);
        throw error;
      }
    },
    async remove(path) {
      try {
        await api.request("DELETE", api.paths.secret(env, unit, path), {});
        const answer = await api.request("GET", `${api.paths.effective(env, unit)}`, { signal });
        eff = effectiveMap(answer);
        editor.setEffective(eff);
      } catch (error) {
        error.userMessage = errorText(error);
        throw error;
      }
    },
  };

  const editor = createValuesEditor({ rows: unitRows(schema), layer, doc: layerDoc ? layerDoc.doc : null, effective: eff, secrets: host ? null : secretOps, onChange: () => ctrl.sync() });
  const ctrl = createLayerController({
    ctx, path: valuesPath, editor, saveButton: save, errorsBox, conflictBox, what: host ? "values of the host" : "values of the unit", reload: reloadLayer,
    initial: { doc: layerDoc ? layerDoc.doc : null, etag: layerDoc ? layerDoc.etag : null },
  });
  const search = searchBox({ placeholder: "Search keys", onInput: (text) => editor.setFilter(text), testid: "values-search" });
  const historyHref = host ? pageUrl("e", env, "units", unit, "hosts", host, "history") : pageUrl("e", env, "units", unit, "history");
  const tools = [el("a", { class: "btn btn--tb", href: historyHref, dataset: { testid: "history" } }, el("span", { class: "btn__label", text: "History" }))];
  if (runner.enabled && !host) {
    if (unitDoc.doc.playbooks.status) tools.push(el("a", { class: "btn btn--tb", href: `${pageUrl("e", env, "units", unit, "run")}?kind=status`, dataset: { testid: "status-run" } }, el("span", { class: "btn__label", text: "Status" })));
    tools.push(el("a", { class: "btn btn--tb btn--tb-primary", href: pageUrl("e", env, "units", unit, "run"), dataset: { testid: "check-apply" } }, el("span", { class: "btn__label", text: "Check and apply" })));
  }
  const delLayer = toolButton("Delete the layer", "trash", { extra: "btn--ghost", testid: "delete-layer", disabled: !layerDoc });
  const confirmSlot = el("div");
  delLayer.addEventListener("click", () => {
    confirmSlot.replaceChildren();
    const box = confirmBox({
      text: `Delete all the ${layer} values? The values of the layers below then apply.`,
      confirmLabel: "Delete",
      testid: "confirm-delete-layer",
      onConfirm: async () => {
        try {
          await api.deleteDoc(valuesPath, ctrl.state().etag);
          ctrl.setBase(null, false);
          await ctrl.refresh();
          ctrl.status.textContent = "The layer is deleted.";
          delLayer.disabled = true;
        } catch (error) {
          notifyError(error);
          return false;
        }
        return true;
      },
    });
    confirmSlot.append(box.el);
    box.focus();
  });

  const doc = unitDoc.doc;
  const fact = (name, value, mono = true) => el("div", { class: "facts__item" }, el("dt", { text: name }), el("dd", { class: mono ? "mono" : "", text: value || "-" }));
  const facts = el("dl", { class: "facts" },
    host ? el("div", { class: "facts__item" }, el("dt", { text: "Unit" }), el("dd", {}, el("a", { href: unitHref, text: label }))) : null,
    fact("Schema", doc.schema),
    host ? null : fact("Deploy", doc.playbooks.deploy),
    host ? null : fact("Render", doc.playbooks.render),
    host ? null : fact("Status", doc.playbooks.status));

  const valuesSection = el("div", { class: "page__section" },
    el("div", { class: "page__section-head" }, el("h2", { class: "page__section-title", text: host ? `Values on ${host}` : "Values" }), ctrl.status, spacer(), ctrl.message, search.el, delLayer),
    confirmSlot, errorsBox, conflictBox, editor.el);

  const sections = [pageHead({ title: host || label, id: host ? `unit ${unit}` : unit, tools }), facts, valuesSection];
  let hostsApi = null;
  if (!host) {
    hostsApi = await hostsSection();
    sections.push(hostsApi.el);
  }
  main.append(el("section", { class: "page" }, ...sections));
  header.setActions(save);

  // The hosts of the unit: a table with a link to the page of each host, and an editor of the list (5.10).
  async function hostsSection() {
    let hostItems = [];
    try {
      hostItems = (await api.listAll(api.paths.hosts(env), {}, { signal })).items;
    } catch (error) {
      if (!api.isAbort(error)) notifyError(error);
    }
    const byId = new Map(hostItems.map((h) => [h.id, h]));
    let current = unitDoc;
    const box = el("div", { class: "page__section", dataset: { testid: "hosts-section" } });
    const editSlot = el("div");
    const list = el("div");
    const edit = toolButton("Edit hosts", "edit", { testid: "edit-hosts", extra: "btn--ghost" });
    box.append(sectionHead("Hosts", spacer(), edit), editSlot, list);

    function renderHosts() {
      list.replaceChildren();
      const ids = current.doc.hosts;
      if (!ids.length) return list.append(emptyBlock({ title: "No host", text: "The unit has no host. A run needs at least one host.", testid: "hosts-empty" }));
      const table = listTable({ layout: "hosts", columns: [{ title: "Host" }, { title: "Address" }, { title: "Port", cls: "num hide-narrow" }, { title: "Labels" }, { title: "", cls: "hide-narrow" }], label: "Hosts of the unit" });
      table.setRows(ids.map((id) => {
        const d = (byId.get(id) || {}).document || {};
        return el("tr", { dataset: { testid: `host-${id}` } },
          linkCell(id, pageUrl("e", env, "units", unit, "hosts", id)),
          textCell(d.address, "cell-mono"), textCell(d.port, "num hide-narrow"),
          nodeCell(el("span", { class: "cell-tags__list" }, ...((d.labels || []).map((l) => el("span", { class: "tag", text: l })))), "cell-tags"),
          el("td", { class: "hide-narrow" }));
      }));
      list.append(table.el);
    }

    edit.addEventListener("click", () => {
      if (editSlot.childElementCount) return;
      const hostbox = createHostbox({ hosts: hostItems.map((h) => ({ id: h.id, address: h.document && h.document.address })), selected: current.doc.hosts });
      const apply = el("button", { type: "button", class: "btn btn--primary", text: "Apply", dataset: { testid: "hosts-apply" } });
      const cancel = el("button", { type: "button", class: "btn", text: "Cancel" });
      const err = el("div", { class: "errors", role: "alert", hidden: true });
      const conflict = el("div");
      cancel.addEventListener("click", () => editSlot.replaceChildren());
      apply.addEventListener("click", async () => {
        apply.disabled = true;
        err.hidden = true;
        const next = { ...current.doc, hosts: hostbox.selected() };
        try {
          const result = await api.putDoc(api.paths.unit(env, unit), next, { etag: current.etag });
          current = { doc: next, etag: result.etag };
          renderHosts();
          editSlot.replaceChildren();
        } catch (error) {
          if (api.isAbort(error)) return;
          if (error.code === "conflict") {
            await showConflict({ container: conflict, path: api.paths.unit(env, unit), mineDoc: next, what: "unit", onKeep: (etag) => { current = { doc: current.doc, etag }; }, onUseCurrent: () => ctx.navigate(window.location.href, { replace: true }) });
          } else if (error.code === "in_use" || error.code === "invalid_document") fillErrorBox(err, error);
          else notifyError(error);
        } finally {
          apply.disabled = false;
        }
      });
      editSlot.append(el("div", { class: "form-page" }, hostbox.el, err, conflict, el("div", { class: "toolbar-row" }, apply, cancel)));
    });
    renderHosts();
    return { el: box };
  }

  return { destroy() { controller.abort(); ctx.header.setActions(); } };
}
