// The editor of the values of one layer (5.21, 2.13, 2.14): a table with the key, the value that applies, the layer that gives it and the
// actions. The person edits the values of ONE layer (environment, unit or host). The edits are a draft. The header saves the draft as one
// document (5.5). The page checks each value with the rules of 2.12 before the write (5.22). It never changes a value to make it fit.
// A secret key shows the fingerprint and never the value (2.14, 5.14). It has its own action: set, then delete.

import { canonical } from "../canonical.js";
import { badge, el } from "../ui.js";
import { valueText } from "../format.js";
import { checkAgainstAll, checkNote, checkSecret, expectedText, fieldText, readField } from "../validate.js";
import { field, iconButton, confirmBox } from "./dom.js";

export const LAYERS = ["default", "environment", "unit", "host"];
// The mask of a secret: a secret is never shown (5.14).
const MASK = "\u2022".repeat(8);
const layerIndex = (name) => LAYERS.indexOf(name);

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const has = (object, key) => Object.prototype.hasOwnProperty.call(object, key);

// The document of a draft (2.10): the notes of the keys that have a value only.
export function documentOf(state) {
  const doc = { format: 1, values: { ...state.values } };
  const notes = {};
  for (const [path, text] of Object.entries(state.notes)) if (has(state.values, path)) notes[path] = text;
  if (Object.keys(notes).length) doc.notes = notes;
  return doc;
}

function copyState(doc) {
  return { values: { ...((doc && doc.values) || {}) }, notes: { ...((doc && doc.notes) || {}) } };
}

// rows: [{ path, defs: [definition of the key], secret, description }]. layer: "environment", "unit" or "host".
// doc: the stored document of the layer (null when it does not exist). effective: Map path -> member of the list of 5.12, or null.
// secrets: null, or { canSet, set(path, value), remove(path) } with async functions that write and then call setEffective().
export function createValuesEditor({ rows, layer, doc, effective = null, canEdit = true, secrets = null, onChange = () => {} }) {
  let base = copyState(doc);
  let draft = copyState(doc);
  let eff = effective;
  let filterText = "";
  let editing = null; // the path of the row with an open panel
  const errors = new Map(); // path -> message of the server
  const nodes = new Map(); // path -> { tr, panel }
  const tbody = el("tbody", { dataset: { testid: "values-body" } });
  const table = el("table", { class: "vtable", "aria-label": `Values of the layer ${layer}`, dataset: { testid: "values" } },
    el("thead", {}, el("tr", {}, el("th", { text: "Key" }), el("th", { text: "Value" }), el("th", { text: "Layer" }), el("th", { class: "visually-hidden", text: "Actions" }))),
    tbody);

  const isOwn = (path) => has(draft.values, path);
  const isChanged = (path) => isOwn(path) !== has(base.values, path) || (isOwn(path) && (!same(draft.values[path], base.values[path]) || draft.notes[path] !== base.notes[path]));

  // The value and the layer that apply to a key after the edits of the draft.
  function applied(row) {
    const info = eff ? eff.get(row.path) : null;
    if (!row.secret && isOwn(row.path)) return { layer, value: draft.values[row.path], own: true };
    if (!info) return { layer: null, value: undefined, own: false };
    const own = layerIndex(layer);
    // The highest layer that gives a value, below the edited layer (the edited layer has no value in the draft).
    let found = null;
    for (const name of LAYERS) {
      if (!info.layers || info.layers[name] === undefined) continue;
      if (name === layer && !row.secret) continue;
      if (layerIndex(name) > own && own >= 0) continue;
      found = name;
    }
    if (!found) return { layer: null, value: undefined, own: false };
    const raw = info.layers[found];
    return { layer: found, value: row.secret ? undefined : raw, fingerprint: row.secret && raw ? raw.fingerprint : null, own: found === layer };
  }

  function layerBadge(name) {
    return name ? el("span", { class: `badge layer--${name}`, text: name }) : null;
  }

  function buildRow(row) {
    const view = applied(row);
    const info = eff ? eff.get(row.path) : null;
    const changed = !row.secret && isChanged(row.path);
    const message = errors.get(row.path) || (info && info.error ? `${info.error.layer ? `(${info.error.layer}) ` : ""}${info.error.message}` : null);
    const tr = el("tr", { dataset: { path: row.path, testid: `row-${row.path}`, ...(changed ? { draft: isOwn(row.path) ? "changed" : "removed" } : {}), ...(message ? { state: "error" } : {}) } });
    tr.append(el("td", { class: "v-key" }, el("span", { class: "v-key__path", title: row.description ? `${row.path}: ${row.description}` : row.path, text: row.path }), badge(row.defs[0].kind)));
    let valueCell;
    if (row.secret) {
      valueCell = el("td", { class: `v-val v-val--secret${view.layer ? "" : " v-val--none"}` }, view.layer ? MASK : "-", view.fingerprint ? el("span", { class: "muted", text: ` ${view.fingerprint}`, title: "Fingerprint of the secret" }) : null);
    } else {
      const text = view.value === undefined ? "-" : valueText(view.value);
      valueCell = el("td", { class: `v-val${view.value === undefined ? " v-val--none" : ""}`, title: text.length > 40 ? text : null, text });
    }
    if (message) valueCell.append(el("div", { class: "field__error", text: message }));
    tr.append(valueCell);
    tr.append(el("td", { class: "v-layer" }, layerBadge(view.layer), changed ? el("span", { class: "visually-hidden", text: "changed" }) : null));
    const actions = el("td", { class: "v-act" });
    if (canEdit && !row.secret) {
      if (changed) actions.append(iconButton("Revert the change", "undo", () => revert(row), { testid: `revert-${row.path}` }));
      actions.append(iconButton("Edit the value", "edit", () => openPanel(row), { testid: `edit-${row.path}` }));
      if (isOwn(row.path)) actions.append(iconButton(`Remove the ${layer} value`, "trash", () => remove(row), { testid: `remove-${row.path}` }));
    } else if (row.secret && secrets && secrets.canSet) {
      actions.append(iconButton("Set the secret", "key", () => openPanel(row), { testid: `secret-${row.path}` }));
    }
    tr.append(actions);
    return tr;
  }

  function matchesFilter(row) {
    return !filterText || row.path.includes(filterText) || editing === row.path;
  }

  function refreshRow(row) {
    const old = nodes.get(row.path);
    const tr = buildRow(row);
    tr.hidden = !matchesFilter(row);
    let panel = null;
    if (old) {
      if (old.panel) panel = old.panel;
      old.tr.replaceWith(tr);
    }
    nodes.set(row.path, { tr, panel });
    if (panel) tr.after(panel);
  }

  function closePanel(row) {
    const entry = nodes.get(row.path);
    if (entry && entry.panel) {
      entry.panel.remove();
      entry.panel = null;
    }
    if (editing === row.path) editing = null;
    refreshRow(row);
  }

  function revert(row) {
    if (has(base.values, row.path)) {
      draft.values[row.path] = base.values[row.path];
      if (has(base.notes, row.path)) draft.notes[row.path] = base.notes[row.path];
      else delete draft.notes[row.path];
    } else {
      delete draft.values[row.path];
      delete draft.notes[row.path];
    }
    errors.delete(row.path);
    refreshRow(row);
    onChange();
  }

  function remove(row) {
    delete draft.values[row.path];
    delete draft.notes[row.path];
    errors.delete(row.path);
    refreshRow(row);
    onChange();
  }

  // The panel of a value: the control of the kind, the note, the range and the layers.
  function openPanel(row) {
    if (editing && editing !== row.path) {
      const other = rows.find((r) => r.path === editing);
      if (other) closePanel(other);
    }
    const existing = nodes.get(row.path);
    if (existing && existing.panel) {
      existing.panel.querySelector("input, select, textarea")?.focus();
      return;
    }
    editing = row.path;
    const panel = row.secret ? secretPanel(row) : valuePanel(row);
    const entry = nodes.get(row.path);
    entry.panel = panel;
    entry.tr.after(panel);
    refreshRow(row);
    panel.querySelector("input, select, textarea")?.focus();
  }

  function layersLine(row) {
    const info = eff ? eff.get(row.path) : null;
    const list = el("ul", { class: "v-layers", "aria-label": "Values of the layers" });
    const def = row.defs[0];
    if (def.default !== undefined) list.append(el("li", {}, layerBadge("default"), el("span", { class: "mono", text: valueText(def.default) })));
    if (info && info.layers) {
      for (const name of LAYERS) {
        if (name === "default" || info.layers[name] === undefined) continue;
        // A secret layer holds the fingerprint, never a value (2.14).
        const shownValue = row.secret ? info.layers[name].fingerprint : valueText(info.layers[name]);
        list.append(el("li", {}, layerBadge(name), el("span", { class: "mono", text: shownValue })));
      }
    }
    return list;
  }

  function control(row, current) {
    const def = row.defs[0];
    if (def.kind === "enum" || def.kind === "bool") {
      const select = el("select", { class: "input", dataset: { testid: "value" } });
      // A key of several schemas (the common values) offers the values that pass the check of each schema (2.15).
      const options = def.kind === "bool" ? ["true", "false"] : def.values.filter((value) => !checkAgainstAll(row.defs, value));
      const currentText = current === undefined ? "" : String(current);
      if (current === undefined) select.append(el("option", { value: "", text: "Choose a value", disabled: true, selected: true }));
      for (const value of options) select.append(el("option", { value, text: value, selected: value === currentText }));
      return select;
    }
    if (def.kind === "text_list") return el("textarea", { class: "input input--mono", rows: 3, spellcheck: false, dataset: { testid: "value" }, value: fieldText(def, current) });
    return el("input", { type: "text", class: `input${def.kind === "text" || def.kind === "path" ? "" : " input--mono"}`, autocomplete: "off", spellcheck: false, value: fieldText(def, current), dataset: { testid: "value" } });
  }

  function valuePanel(row) {
    const def = row.defs[0];
    const startValue = isOwn(row.path) ? draft.values[row.path] : applied(row).value;
    const input = control(row, startValue);
    const valueField = field({ label: "Value", control: input, hint: `Expected: ${expectedText(def)}.` });
    const noteInput = el("textarea", { class: "input", rows: 2, maxlength: 2000, spellcheck: false, dataset: { testid: "note" }, value: draft.notes[row.path] || "" });
    const noteField = field({ label: "Note", control: noteInput, hint: "Why this value? The history keeps the note." });
    const done = el("button", { type: "button", class: "btn btn--primary", text: "Done", dataset: { testid: "done" } });
    const cancel = el("button", { type: "button", class: "btn", text: "Cancel", dataset: { testid: "cancel" } });
    let touched = false;

    function read() {
      const parsed = readField(def, input.value);
      if (parsed.error) return { error: parsed.error };
      const message = checkAgainstAll(row.defs, parsed.value);
      return message ? { error: message } : { value: parsed.value };
    }
    function check() {
      const result = read();
      const noteError = noteInput.value ? checkNote(noteInput.value) : null;
      valueField.setError(touched && result.error ? result.error : null);
      noteField.setError(noteError);
      done.disabled = !!(result.error || noteError);
    }
    input.addEventListener("input", () => { touched = true; check(); });
    input.addEventListener("change", () => { touched = true; check(); });
    noteInput.addEventListener("input", check);
    done.addEventListener("click", () => {
      const result = read();
      if (result.error) return;
      draft.values[row.path] = result.value;
      if (noteInput.value) draft.notes[row.path] = noteInput.value;
      else delete draft.notes[row.path];
      errors.delete(row.path);
      closePanel(row);
      onChange();
    });
    cancel.addEventListener("click", () => closePanel(row));
    const descriptionLine = el("div", { class: "v-hint field--wide", text: row.description || "" });
    const box = el("div", { class: "v-editor" }, valueField.el, noteField.el, descriptionLine, layersLine(row), el("div", { class: "v-editor__buttons" }, done, cancel));
    panelKeys(box, cancel);
    check();
    return el("tr", { class: "v-edit", dataset: { testid: `panel-${row.path}` } }, el("td", {}, box));
  }

  function secretPanel(row) {
    const def = row.defs;
    const input = el("input", { type: "password", class: "input input--mono", autocomplete: "new-password", spellcheck: false, dataset: { testid: "secret-value" } });
    const valueField = field({ label: "New value", control: input, hint: "The value is never shown again. The page shows the fingerprint only." });
    const set = el("button", { type: "button", class: "btn btn--primary", text: "Set the secret", disabled: true, dataset: { testid: "secret-set" } });
    const cancel = el("button", { type: "button", class: "btn", text: "Cancel" });
    const buttons = el("div", { class: "v-editor__buttons" }, set, cancel);
    const info = eff ? eff.get(row.path) : null;
    const ownSecret = !!(info && info.layers && info.layers[layer] !== undefined);
    const status = el("div", { class: "field__error", role: "alert" });
    input.addEventListener("input", () => {
      const message = input.value ? checkSecret(def, input.value) : null;
      valueField.setError(message);
      set.disabled = !input.value || !!message;
    });
    set.addEventListener("click", async () => {
      set.disabled = true;
      status.textContent = "";
      try {
        await secrets.set(row.path, input.value);
        input.value = "";
        closePanel(row);
      } catch (error) {
        status.textContent = (error && error.userMessage) || "The secret was not set.";
        set.disabled = false;
      }
    });
    cancel.addEventListener("click", () => {
      input.value = "";
      closePanel(row);
    });
    if (ownSecret) {
      const del = el("button", { type: "button", class: "btn btn--danger", text: "Delete the secret", dataset: { testid: "secret-delete" } });
      del.addEventListener("click", () => {
        const confirm = confirmBox({
          text: `Delete the secret ${row.path} of this ${layer}? The value cannot come back.`,
          confirmLabel: "Delete",
          testid: "confirm-secret",
          onConfirm: async () => {
            try {
              await secrets.remove(row.path);
              closePanel(row);
            } catch (error) {
              status.textContent = (error && error.userMessage) || "The secret was not deleted.";
              return false;
            }
            return true;
          },
        });
        box.append(confirm.el);
        del.disabled = true;
        confirm.el.querySelector("[data-testid=confirm-secret-no]")?.addEventListener("click", () => { del.disabled = false; });
      });
      buttons.append(del);
    }
    const box = el("div", { class: "v-editor" }, valueField.el, layersLine(row), status, buttons);
    panelKeys(box, cancel);
    return el("tr", { class: "v-edit", dataset: { testid: `panel-${row.path}` } }, el("td", {}, box));
  }

  // Escape closes the panel of the key.
  function panelKeys(box, cancel) {
    box.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        cancel.click();
      }
    });
  }

  function renderAll() {
    nodes.clear();
    const trs = [];
    for (const row of rows) {
      const tr = buildRow(row);
      tr.hidden = !matchesFilter(row);
      nodes.set(row.path, { tr, panel: null });
      trs.push(tr);
    }
    tbody.replaceChildren(...trs);
  }

  renderAll();

  return {
    el: table,
    getDoc: () => documentOf(draft),
    isDirty: () => canonical(documentOf(draft)) !== canonical(documentOf(base)),
    changedCount: () => rows.filter((row) => !row.secret && isChanged(row.path)).length,
    // The stored document and the effective values after a save or a reload. The draft is the stored document again.
    reset(nextDoc, nextEffective) {
      base = copyState(nextDoc);
      draft = copyState(nextDoc);
      if (nextEffective !== undefined) eff = nextEffective;
      errors.clear();
      editing = null;
      renderAll();
    },
    // The effective values change (a secret was set); the draft stays.
    setEffective(next) {
      eff = next;
      for (const row of rows) refreshRow(row);
    },
    // The base of the draft moves to a newer stored document, and the draft stays (the person keeps the edits of a conflict).
    rebase(nextDoc, nextEffective) {
      base = copyState(nextDoc);
      if (nextEffective !== undefined) eff = nextEffective;
      for (const row of rows) refreshRow(row);
      onChange();
    },
    setFilter(text) {
      filterText = text;
      for (const row of rows) {
        const entry = nodes.get(row.path);
        if (!entry) continue;
        entry.tr.hidden = !matchesFilter(row);
        if (entry.panel) entry.panel.hidden = entry.tr.hidden;
      }
    },
    // The errors of a 422 (5.4): one item for each key path. Other items return in the list that the function gives back.
    markErrors(items) {
      errors.clear();
      const rest = [];
      for (const item of items || []) {
        const row = rows.find((r) => r.path === item.path);
        if (row) errors.set(row.path, item.message);
        else rest.push(item);
      }
      for (const row of rows) refreshRow(row);
      return rest;
    },
    count: () => rows.length,
  };
}

