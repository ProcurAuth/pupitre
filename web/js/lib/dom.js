// Small builders that the pages share: the head of a page, a field, a status dot, a time, a search box, a confirmation.
// Each function builds elements with createElement, textContent and setAttribute (5.22). None makes HTML from a string.

import { button, el, icon, uniqueId } from "../ui.js";
import { localTime, shortAgo } from "../format.js";

// The head of a page: the title, the id and the tools at the right.
export function pageHead({ title, id = null, tools = [] }) {
  return el("div", { class: "page__head" },
    el("h1", { class: "page__title", text: title }),
    id ? el("span", { class: "page__id", text: id }) : null,
    tools.length ? el("div", { class: "page__tools" }, ...tools) : null);
}

export function sectionHead(title, ...nodes) {
  return el("div", { class: "page__section-head" }, el("h2", { class: "page__section-title", text: title }), ...nodes);
}

export const spacer = () => el("span", { class: "spacer" });

// A status dot with an accessible name and no text (DATA-01). kind: ok, warn, error, info, wait or run.
export function statusDot(kind, label) {
  return el("span", { class: `dot dot--${kind}`, role: "img", "aria-label": label, title: label });
}

// A time in the local zone with the server time in the title (DATA-04): a short age or the local time.
export function timeEl(iso, { age = true } = {}) {
  if (!iso) return el("span", { class: "muted", text: "-" });
  return el("time", { datetime: iso, title: `${localTime(iso)} (server time ${iso})`, text: age ? shortAgo(iso) : localTime(iso) });
}

// A field: a label, a control, a hint and an error. Returns { el, control, setError(text) }.
export function field({ label, control, hint = null, id = null, wide = false }) {
  const controlId = id || control.id || uniqueId("f");
  control.id = controlId;
  const error = el("span", { class: "field__error", role: "alert", id: `${controlId}-error` });
  const node = el("div", { class: `field${wide ? " field--wide" : ""}` },
    el("label", { class: "field__label", for: controlId, text: label }),
    control,
    hint ? el("span", { class: "field__hint", id: `${controlId}-hint`, text: hint }) : null,
    error);
  return {
    el: node,
    control,
    setError(text) {
      error.textContent = text || "";
      if (text) {
        control.setAttribute("aria-invalid", "true");
        control.setAttribute("aria-describedby", error.id);
      } else {
        control.removeAttribute("aria-invalid");
        control.removeAttribute("aria-describedby");
      }
    },
  };
}

export const textInput = (attrs = {}) => el("input", { type: "text", class: "input", autocomplete: "off", spellcheck: false, ...attrs });

// A search box of the list page of Hune.
export function searchBox({ placeholder, value = "", onInput, testid = "search" }) {
  const input = el("input", { type: "search", class: "list-search__input", value, placeholder, autocomplete: "off", "aria-label": placeholder, dataset: { testid } });
  const clearButton = el("button", { type: "button", class: "list-search__clear", hidden: !value, on: { click: () => { input.value = ""; sync(); input.focus(); } } }, icon("close", 14), el("span", { text: "Clear" }));
  const box = el("div", { class: "list-search" }, el("span", { class: "list-search__icon", "aria-hidden": "true" }, icon("search", 16)), input, clearButton);
  function sync() {
    clearButton.hidden = !input.value;
    onInput(input.value);
  }
  input.addEventListener("input", sync);
  return { el: box, input };
}

// A tool button of the header (the variant tb of Hune). Its size never depends on its state (CTRL-22).
// `collapse` hides the label up to 900 px: the button is then its icon, with the label as its name and its tooltip.
export function toolButton(label, iconName, { primary = false, onClick = null, disabled = false, iconOnly = false, collapse = false, testid = null, extra = "" } = {}) {
  const ghost = extra.split(" ").includes("btn--ghost");
  const classes = ghost ? ["btn"] : ["btn", "btn--tb"];
  if (collapse) classes.push("btn--collapse");
  if (primary) classes.push("btn--tb-primary");
  if (iconOnly) classes.push("btn--icon");
  if (extra) classes.push(extra);
  return el("button", {
    type: "button",
    class: classes.join(" "),
    disabled,
    "aria-label": iconOnly ? label : null,
    "data-tooltip": iconOnly || collapse ? label : null,
    dataset: testid ? { testid } : {},
    on: onClick ? { click: onClick } : {},
  }, iconName ? icon(iconName, 18) : null, el("span", { class: iconOnly ? "visually-hidden" : "btn__label", text: label }));
}

// An icon button with a tooltip and an accessible name (CTRL-05, CTRL-06): a quiet button, flat.
export function iconButton(label, iconName, onClick, { testid = null, danger = false, disabled = false } = {}) {
  const node = button(label, { disabled, dataset: testid ? { testid } : {}, on: onClick ? { click: onClick } : {} }, iconName, { iconOnly: true, variant: "ghost" });
  if (danger) node.classList.add("btn--danger-text");
  return node;
}

// The skeleton of a page that loads. The rows keep the place of the content (no jump).
export function loadingBlock(rows = 3) {
  const box = el("div", { class: "loading", "aria-busy": "true", "aria-hidden": "true" });
  for (let i = 0; i < rows; i += 1) box.append(el("span", { class: "skeleton" }));
  return box;
}

// An in-place confirmation (a callout with two buttons). It returns { el, close }. `onConfirm` can return false to keep the callout.
// Q3 of the owner: an irreversible action asks a confirmation in place, not a popup.
export function confirmBox({ text, confirmLabel, danger = true, tone = "warning", onConfirm, onCancel = null, testid = "confirm" }) {
  const yes = el("button", { type: "button", class: `btn ${danger ? "btn--danger" : "btn--primary"}`, text: confirmLabel, dataset: { testid: `${testid}-yes` } });
  const no = el("button", { type: "button", class: "btn", text: "Cancel", dataset: { testid: `${testid}-no` } });
  const glyph = tone === "error" ? "error" : tone === "info" ? "info" : "warning";
  const node = el("div", { class: `callout callout--${tone} confirm`, role: "alertdialog", "aria-label": text, dataset: { testid } },
    icon(glyph, 16), el("span", { class: "callout__text", text }), el("div", { class: "confirm__actions" }, yes, no));
  const close = () => node.remove();
  yes.addEventListener("click", async () => {
    yes.disabled = true;
    no.disabled = true;
    let keep = false;
    try {
      keep = (await onConfirm()) === false;
    } finally {
      yes.disabled = false;
      no.disabled = false;
    }
    if (!keep) close();
  });
  no.addEventListener("click", () => {
    close();
    if (onCancel) onCancel();
  });
  return { el: node, close, focus: () => no.focus() };
}

// A host or an item list that fits one line: the ids joined with a comma.
export const idsText = (ids) => (ids && ids.length ? ids.join(", ") : "-");

// The node path of a unit page: used in the facts of a run.
export const mono = (text) => el("span", { class: "mono", text });
