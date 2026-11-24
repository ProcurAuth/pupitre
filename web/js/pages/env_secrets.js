// The tab Secrets of an environment (5.14, 5.21): the secrets of the scope environment. The page shows the name, the fingerprint, the time and
// the subject of the last write, and never a value. The person picks the name from the secret keys of the schemas and gives a new value.

import * as api from "../api.js";
import { fillErrorBox, notifyError } from "../errors.js";
import { el } from "../ui.js";
import { loadCatalog } from "../lib/catalog.js";
import { confirmBox, field, iconButton, loadingBlock, timeEl } from "../lib/dom.js";
import { emptyBlock, listTable, nodeCell, textCell } from "../lib/list_table.js";
import { compareBytes } from "../canonical.js";
import { checkSecret } from "../validate.js";

export async function mountTab({ ctx, env, container, controller }) {
  container.append(loadingBlock());
  let catalog;
  let items;
  try {
    catalog = await loadCatalog(controller.signal);
    items = (await api.request("GET", api.paths.secrets(env), { signal: controller.signal })).items;
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error);
    return {};
  }
  const defsByName = new Map();
  for (const schema of catalog.schemas || []) for (const def of schema.keys) if (def.secret) defsByName.set(def.path, [...(defsByName.get(def.path) || []), def]);
  const names = [...defsByName.keys()].sort(compareBytes);
  const body = el("div");
  const formBox = el("div");
  container.replaceChildren(formBox, body);

  function renderList() {
    body.replaceChildren();
    if (!items.length) return body.append(emptyBlock({ title: "No secret", text: "No secret is set in this environment.", testid: "list-empty" }));
    const table = listTable({ layout: "secrets", columns: [{ title: "Name" }, { title: "Fingerprint" }, { title: "Updated", cls: "num hide-narrow" }, { title: "By", cls: "hide-narrow" }, { title: "" }], label: "Secrets" });
    table.setRows(items.map((s) => {
      const del = iconButton(`Delete the secret ${s.name}`, "trash", () => askDelete(s, del), { testid: `delete-${s.name}`, danger: true });
      return el("tr", { dataset: { testid: `secret-${s.name}` } }, textCell(s.name, "cell-mono"), textCell(s.fingerprint, "cell-mono cell-muted"), nodeCell(timeEl(s.updated_at, { age: false }), "num cell-muted hide-narrow"), textCell(s.updated_by, "cell-muted hide-narrow"), nodeCell(del, "cell-act"));
    }));
    body.append(table.el);
  }

  function askDelete(secret, anchor) {
    formBox.querySelectorAll("[data-testid=confirm-secret-delete]").forEach((n) => n.remove());
    const box = confirmBox({
      text: `Delete the secret ${secret.name}? The value cannot come back.`,
      confirmLabel: "Delete",
      testid: "confirm-secret-delete",
      onConfirm: async () => {
        try {
          await api.request("DELETE", api.paths.secret(env, null, secret.name));
          items = items.filter((x) => x.name !== secret.name);
          renderList();
        } catch (error) {
          notifyError(error);
          return false;
        }
        return true;
      },
    });
    formBox.prepend(box.el);
    box.focus();
  }

  // The form: the name (a list of the secret keys), the new value, and the button.
  const nameSelect = el("select", { class: "input", dataset: { testid: "secret-name" } }, el("option", { value: "", text: "Choose a secret key", disabled: true, selected: true }), ...names.map((n) => el("option", { value: n, text: n })));
  const valueInput = el("input", { type: "password", class: "input input--mono", autocomplete: "new-password", spellcheck: false, dataset: { testid: "secret-value" } });
  const nameField = field({ label: "Name", control: nameSelect });
  const valueField = field({ label: "New value", control: valueInput, hint: "The value is never shown again. A new value replaces the old value." });
  const set = el("button", { type: "button", class: "btn btn--primary", text: "Set the secret", disabled: true, dataset: { testid: "secret-set" } });
  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  function sync() {
    const message = valueInput.value && nameSelect.value ? checkSecret(defsByName.get(nameSelect.value), valueInput.value) : null;
    valueField.setError(message);
    set.disabled = !nameSelect.value || !valueInput.value || !!message;
  }
  nameSelect.addEventListener("change", sync);
  valueInput.addEventListener("input", sync);
  set.addEventListener("click", async () => {
    set.disabled = true;
    errors.hidden = true;
    try {
      const name = nameSelect.value;
      await api.request("PUT", api.paths.secret(env, null, name), { body: { value: valueInput.value } });
      valueInput.value = "";
      nameSelect.value = "";
      items = (await api.request("GET", api.paths.secrets(env))).items;
      renderList();
    } catch (error) {
      if (api.isAbort(error)) return;
      if (error.code === "invalid_secret") fillErrorBox(errors, error);
      else notifyError(error);
    } finally {
      sync();
    }
  });
  if (names.length) formBox.append(el("div", { class: "form-page" }, el("div", { class: "form-grid" }, nameField.el, valueField.el), errors, el("div", { class: "toolbar-row" }, set)));
  renderList();
  return { destroy() { valueInput.value = ""; } };
}
