// The list box of hosts (5.21: the list box of Hune): the person picks hosts from the valid choices and never types an id (CTRL-35).
// It has a search, "Select all", the count of the picked hosts and, with a bound, the limit of the session (5.7).

import { compareBytes } from "../canonical.js";
import { el } from "../ui.js";
import { matches } from "./list_table.js";
import { searchBox } from "./dom.js";

// opts: { hosts: [{ id, address }], selected: [ids], max: number | null, onChange(), single: bool }
export function createHostbox({ hosts, selected = [], max = null, onChange = () => {}, single = false, label = "Hosts", emptyText = "This environment has no host." }) {
  let empty = emptyText;
  let items = hosts.slice().sort((a, b) => compareBytes(a.id, b.id));
  const picked = new Set(selected);
  let query = "";
  let disabled = false;
  const list = el("div", { class: "listbox", role: "listbox", "aria-label": label, "aria-multiselectable": single ? null : "true", dataset: { testid: "hostbox-list" } });
  const count = el("span", { class: "hostbox__count", role: "status", dataset: { testid: "hostbox-count" } });
  const search = searchBox({ placeholder: "Search hosts", onInput: (text) => { query = text; render(); }, testid: "hostbox-search" });
  const all = el("button", { type: "button", class: "btn btn--sm", text: "Select all", dataset: { testid: "hostbox-all" } });
  const none = el("button", { type: "button", class: "btn btn--sm", text: "Clear", dataset: { testid: "hostbox-none" } });
  const head = el("div", { class: "hostbox__head" }, search.el, ...(single ? [] : [all, none]), count);
  const box = el("div", { class: "hostbox", dataset: { testid: "hostbox" } }, head, list);

  const visible = () => items.filter((h) => matches(query, h.id, h.address));

  function paintCount() {
    const over = max !== null && picked.size > max;
    count.textContent = single ? `${picked.size} of ${items.length}` : `${picked.size} of ${items.length} picked${max !== null ? `, the limit is ${max}` : ""}`;
    count.classList.toggle("hostbox__count--over", over);
    box.toggleAttribute("data-over", over);
  }

  function toggle(id) {
    if (disabled) return;
    if (single) {
      picked.clear();
      picked.add(id);
    } else if (picked.has(id)) picked.delete(id);
    else picked.add(id);
    render();
    onChange();
  }

  function render() {
    list.replaceChildren();
    const rows = visible();
    if (!rows.length) list.append(el("div", { class: "listbox__empty muted", text: items.length ? "No host matches." : empty }));
    rows.forEach((h, index) => {
      const node = el("div", {
        class: "listbox__item",
        role: "option",
        tabindex: index === 0 ? 0 : -1,
        "aria-selected": String(picked.has(h.id)),
        dataset: { id: h.id, testid: `host-${h.id}` },
        on: {
          click: () => toggle(h.id),
          keydown: (event) => {
            const nodes = [...list.querySelectorAll(".listbox__item")];
            const at = nodes.indexOf(node);
            const go = (n) => { if (nodes[n]) { nodes.forEach((x) => x.setAttribute("tabindex", "-1")); nodes[n].setAttribute("tabindex", "0"); nodes[n].focus(); } };
            if (event.key === " " || event.key === "Enter") { event.preventDefault(); toggle(h.id); const again = list.querySelector(`[data-id="${CSS.escape(h.id)}"]`); if (again) { again.setAttribute("tabindex", "0"); again.focus(); } }
            else if (event.key === "ArrowDown") { event.preventDefault(); go(Math.min(at + 1, nodes.length - 1)); }
            else if (event.key === "ArrowUp") { event.preventDefault(); go(Math.max(at - 1, 0)); }
            else if (event.key === "Home") { event.preventDefault(); go(0); }
            else if (event.key === "End") { event.preventDefault(); go(nodes.length - 1); }
          },
        },
      }, single ? null : el("span", { class: "listbox__mark" }), el("span", { class: "mono", text: h.id }), h.address ? el("span", { class: "muted", text: h.address }) : null);
      list.append(node);
    });
    paintCount();
  }

  all.addEventListener("click", () => { for (const h of visible()) picked.add(h.id); render(); onChange(); });
  none.addEventListener("click", () => { picked.clear(); render(); onChange(); });
  render();

  return {
    el: box,
    selected: () => [...picked].filter((id) => items.some((h) => h.id === id)).sort(compareBytes),
    setSelected(ids) { picked.clear(); for (const id of ids) picked.add(id); render(); },
    setHosts(next, text) {
      if (text) empty = text;
      items = next.slice().sort((a, b) => compareBytes(a.id, b.id)); for (const id of [...picked]) if (!items.some((h) => h.id === id)) picked.delete(id); render(); },
    setDisabled(value) { disabled = value; box.toggleAttribute("aria-disabled", value); },
  };
}
