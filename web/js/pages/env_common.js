// The tab Common values (5.11, 5.21): the values of the layer environment. The keys are the keys of all schemas that are not secret keys (2.10).
// The person edits a draft, and the Save button of the header writes the document `common` (5.5).

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { compareBytes } from "../canonical.js";
import { el } from "../ui.js";
import { loadCatalog } from "../lib/catalog.js";
import { confirmBox, loadingBlock, searchBox, toolButton } from "../lib/dom.js";
import { emptyBlock } from "../lib/list_table.js";
import { createLayerController } from "../lib/layer_page.js";
import { createValuesEditor } from "../lib/values_editor.js";
import { notifyError } from "../errors.js";

// The rows of the editor: one for each key path of the schemas, in byte order. A path that one schema declares as a secret key is not a row (2.10).
export function commonRows(catalog) {
  const byPath = new Map();
  const secret = new Set();
  for (const schema of catalog.schemas || []) {
    for (const def of schema.keys) {
      if (def.secret) secret.add(def.path);
      if (!byPath.has(def.path)) byPath.set(def.path, []);
      byPath.get(def.path).push(def);
    }
  }
  return [...byPath.keys()].filter((p) => !secret.has(p)).sort(compareBytes).map((path) => ({ path, defs: byPath.get(path), secret: false, description: byPath.get(path)[0].description }));
}

export async function mountTab({ ctx, env, container, controller }) {
  container.append(loadingBlock());
  let catalog;
  let loaded;
  try {
    [catalog, loaded] = await Promise.all([loadCatalog(controller.signal), api.getDoc(api.paths.common(env), { signal: controller.signal })]);
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error);
    return {};
  }
  const rows = commonRows(catalog);
  container.replaceChildren();
  if (!rows.length) {
    container.append(emptyBlock({ title: "No key", text: "The schemas of the configuration have no key that is not a secret key.", testid: "list-empty" }));
    return {};
  }
  const path = api.paths.common(env);
  const errorsBox = el("div", { class: "errors", role: "alert", hidden: true });
  const conflictBox = el("div", { class: "conflict-box" });
  const save = toolButton("Save", "save", { primary: true, disabled: true, extra: "edit-save", testid: "save" });
  const editor = createValuesEditor({ rows, layer: "environment", doc: loaded ? loaded.doc : null, effective: null, onChange: () => ctrl.sync() });
  const ctrl = createLayerController({
    ctx, path, editor, saveButton: save, errorsBox, conflictBox, what: "common values",
    initial: { doc: loaded ? loaded.doc : null, etag: loaded ? loaded.etag : null },
    reload: async () => {
      const fresh = await api.getDoc(path, { signal: controller.signal });
      return { doc: fresh ? fresh.doc : null, etag: fresh ? fresh.etag : null, effective: null };
    },
  });
  const search = searchBox({ placeholder: "Search keys", onInput: (text) => editor.setFilter(text), testid: "values-search" });
  const history = el("a", { class: "btn btn--tb", href: `${pageUrl("e", env, "history")}?doc=common`, dataset: { testid: "history" } }, el("span", { class: "btn__label", text: "History" }));
  const del = toolButton("Delete the layer", "trash", { extra: "btn--ghost", testid: "delete-layer", disabled: !loaded });
  const confirmSlot = el("div");
  del.addEventListener("click", () => {
    confirmSlot.replaceChildren();
    const box = confirmBox({
      text: "Delete all common values? The environment keeps no common value, and the default values of the schemas apply.",
      confirmLabel: "Delete",
      testid: "confirm-delete-layer",
      onConfirm: async () => {
        try {
          await api.deleteDoc(path, ctrl.state().etag);
          ctrl.setBase(null, false);
          await ctrl.refresh();
          ctrl.status.textContent = "The layer is deleted.";
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
  container.append(el("div", { class: "page__section" },
    el("div", { class: "page__section-head" }, search.el, ctrl.status, el("span", { class: "spacer" }), ctrl.message, history, del),
    confirmSlot, errorsBox, conflictBox, editor.el));
  ctx.header.setActions(save);
  return { destroy() { ctx.header.setActions(); ctx.setGuard(null); } };
}
