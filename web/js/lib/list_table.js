// The list table of Hune (css/list.css): a head row and one grid row for each item. The columns come from the attribute data-layout of
// the container (css/pupitre.css). A row is built from cells. An item with errors shows the badge "Not valid" and a toggle for its errors.

import { badge, el, icon, uniqueId } from "../ui.js";

// A cell with a link to a page of Pupitre. The link is a real <a>, so a click with a modifier key works (the router handles the plain click).
export function linkCell(text, href, { cls = "cell-title", testid = null } = {}) {
  return el("td", { class: cls }, el("span", { class: "cell-title__inner" }, el("a", { class: "cell-title__link", href, title: text, text, dataset: testid ? { testid } : {} })));
}

export function textCell(text, cls = "") {
  return el("td", { class: cls, title: typeof text === "string" && text.length > 24 ? text : null, text: text === undefined || text === null || text === "" ? "-" : String(text) });
}

export function nodeCell(node, cls = "") {
  return el("td", { class: cls }, node);
}

// The cell of the id of an item that has errors: the badge and the toggle. The detail row shows under the title cell.
function invalidTitle(id, errors) {
  const panelId = uniqueId("errors");
  const list = el("ul", { class: "error-list" });
  for (const item of errors || []) list.append(el("li", {}, item.path ? el("code", { text: item.path }) : null, item.path ? " " : null, item.message || ""));
  const detail = el("div", { id: panelId, class: "invalid-detail", hidden: true }, list);
  const chevron = el("span", { class: "errors-toggle__chevron" }, icon("chevron-down", 12));
  const text = el("span", { class: "errors-toggle__text", text: "Show errors" });
  const toggle = el("button", {
    type: "button",
    class: "btn btn--sm btn--ghost errors-toggle",
    "aria-expanded": "false",
    "aria-controls": panelId,
    on: {
      click: () => {
        const open = toggle.getAttribute("aria-expanded") === "true";
        toggle.setAttribute("aria-expanded", String(!open));
        text.textContent = open ? "Show errors" : "Hide errors";
        detail.hidden = open;
      },
    },
  }, text, chevron);
  return el("td", { class: "cell-title" }, el("span", { class: "cell-title__inner" }, icon("error", 14), el("code", { class: "cell-title__uid", text: id }), badge("Not valid", "error"), toggle), detail);
}

// A row of an item with errors: the first cell is the toggle, the other cells stay empty so that the columns hold.
export function invalidRow(id, errors, columns, testid = null) {
  const cells = [invalidTitle(id, errors)];
  for (let i = 1; i < columns; i += 1) cells.push(el("td"));
  return el("tr", { dataset: { state: "invalid", ...(testid ? { testid } : {}) } }, ...cells);
}

// The table: `layout` names the columns (data-layout), `columns` is [{ title, cls }]. It returns { el, body, setRows(rows) }.
// A column title can be "" for an obvious column (DATA-10).
export function listTable({ layout, columns, testid = "list", label }) {
  const head = el("div", { class: "list-colhead", role: "presentation" }, ...columns.map((c) => el("span", { class: `list-colhead__cell${c.cls ? ` ${c.cls}` : ""}`, text: c.title })));
  const body = el("tbody");
  const table = el("table", { class: "list-table list-table--flat", "aria-label": label || null }, body);
  const box = el("div", { class: "list-block", dataset: { layout, testid } }, head, table);
  return {
    el: box,
    body,
    head,
    setRows(rows) {
      body.replaceChildren(...rows);
    },
    append(rows) {
      body.append(...rows);
    },
  };
}

// An empty state of the list page of Hune: an icon, a title and the actions.
export function emptyBlock({ title, text = null, actions = [], testid = "empty" }) {
  return el("div", { class: "empty", dataset: { testid } },
    el("div", { class: "empty__icon" }, icon("inbox", 20)),
    el("p", { class: "empty__title", text: title }),
    text ? el("p", { class: "empty__text", text }) : null,
    actions.length ? el("div", { class: "empty__actions" }, ...actions) : null);
}

// The case-insensitive match of a text with a search (ASCII only, as Hune does).
export const fold = (text) => String(text ?? "").replace(/[A-Z]/g, (c) => c.toLowerCase());
export const matches = (needle, ...texts) => !needle || texts.some((t) => fold(t).includes(fold(needle)));
