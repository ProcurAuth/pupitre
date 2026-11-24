// The form of the document `host` (2.8): the address, the port and the labels. The page checks each field with the rules of 2.8 before the write.
// A label of a host is an inventory group, so it cannot be the group of a schema of the catalog (2.8). The form shows a chip for each label.

import { el, icon } from "../ui.js";
import { checkHostAddress, checkHostPort, checkId, checkLabel } from "../validate.js";
import { compareBytes } from "../canonical.js";
import { field, textInput } from "./dom.js";

// opts: { doc, withId, groups: [group names of the schemas], onChange }
export function createHostForm({ doc, withId = false, groups = [], onChange = () => {} }) {
  const labels = new Set((doc && doc.labels) || []);
  const idInput = withId ? textInput({ maxlength: 63, dataset: { testid: "id" } }) : null;
  const addressInput = textInput({ value: (doc && doc.address) || "", maxlength: 253, dataset: { testid: "address" } });
  const portInput = textInput({ value: doc && doc.port !== undefined ? String(doc.port) : "", inputmode: "numeric", maxlength: 5, dataset: { testid: "port" } });
  const chipBox = el("div", { class: "chips", dataset: { testid: "labels" } });
  const labelInput = el("input", { type: "text", class: "chips__input", autocomplete: "off", spellcheck: false, "aria-label": "Add a label", placeholder: "Add a label", dataset: { testid: "label-input" } });
  const idField = idInput ? field({ label: "Id", control: idInput, hint: "The id is in the address of the host. You cannot change it." }) : null;
  const addressField = field({ label: "Address", control: addressInput, hint: "A DNS name or an IP address." });
  const portField = field({ label: "Port", control: portInput, hint: "Empty: the SSH client uses its own port." });
  const labelsField = field({ label: "Labels", control: labelInput, hint: "Each label is an inventory group. Press Enter to add a label." });
  // The chips box holds the input: the field label points to the input.
  labelsField.el.replaceChild(chipBox, labelInput);
  chipBox.append(labelInput);
  let touched = { id: false, address: false, port: false };

  function renderChips() {
    chipBox.replaceChildren();
    for (const label of [...labels].sort(compareBytes)) {
      chipBox.append(el("span", { class: "chip", dataset: { testid: `label-${label}` } }, el("span", { text: label }),
        el("button", { type: "button", class: "chip__remove", "aria-label": `Remove the label ${label}`, on: { click: () => { labels.delete(label); renderChips(); changed(); } } }, icon("close", 12))));
    }
    chipBox.append(labelInput);
  }

  function addLabel() {
    const text = labelInput.value.trim();
    if (!text) return true;
    const problem = checkLabel(text) || (groups.includes(text) ? `The label ${JSON.stringify(text)} is the group of a schema.` : null) || (labels.size >= 16 && !labels.has(text) ? "A host has 16 labels at most." : null);
    if (problem) {
      labelsField.setError(problem);
      return false;
    }
    labels.add(text);
    labelInput.value = "";
    labelsField.setError(null);
    renderChips();
    changed();
    return true;
  }

  labelInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault();
      addLabel();
    } else if (event.key === "Backspace" && labelInput.value === "" && labels.size) {
      const last = [...labels].sort(compareBytes).pop();
      labels.delete(last);
      renderChips();
      changed();
    }
  });
  labelInput.addEventListener("blur", () => { if (labelInput.value) addLabel(); });

  function problems() {
    const out = {};
    if (idInput) out.id = checkId(idInput.value, "id");
    out.address = checkHostAddress(addressInput.value);
    out.port = checkHostPort(portInput.value);
    return out;
  }

  function show() {
    const p = problems();
    if (idField) idField.setError(touched.id ? p.id : null);
    addressField.setError(touched.address ? p.address : null);
    portField.setError(touched.port ? p.port : null);
  }

  function changed() {
    show();
    onChange();
  }

  if (idInput) idInput.addEventListener("input", () => { touched.id = true; changed(); });
  addressInput.addEventListener("input", () => { touched.address = true; changed(); });
  portInput.addEventListener("input", () => { touched.port = true; changed(); });
  renderChips();

  return {
    el: el("div", { class: "form-grid" }, ...(idField ? [idField.el] : []), addressField.el, portField.el, labelsField.el),
    id: () => (idInput ? idInput.value : null),
    valid: () => !Object.values(problems()).some(Boolean) && !labelInput.value,
    // The document of the form (2.8): the labels in byte order.
    getDoc() {
      const out = { format: 1, address: addressInput.value, labels: [...labels].sort(compareBytes) };
      if (portInput.value !== "") out.port = Number(portInput.value);
      return out;
    },
    // Shows each problem, also the fields that the person did not touch.
    showAll() {
      touched = { id: true, address: true, port: true };
      show();
    },
    setServerErrors(items) {
      const rest = [];
      for (const item of items || []) {
        if (item.path === "address") addressField.setError(item.message);
        else if (item.path === "port") portField.setError(item.message);
        else if (item.path === "labels") labelsField.setError(item.message);
        else if (item.path === "id" && idField) idField.setError(item.message);
        else rest.push(item);
      }
      return rest;
    },
    setIdError: (text) => idField && idField.setError(text),
    focus: () => (idInput || addressInput).focus(),
  };
}
