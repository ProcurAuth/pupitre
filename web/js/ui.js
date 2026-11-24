// Copied from Hune, web/js/ui.js, commit a55fff3. Edits: the prefix of the icon ids is pupitre-icon-; the paragraph numbers stay as comments of the origin.
// Small DOM helpers: elements, icons, buttons, notices, menus, dialogs, empty states, badges, toggles (9.6, 9.11, 9.26).
// The classes they set are in css/components.css, the catalogue of the classes.
// Text from a dashboard or a result goes in the DOM only with textContent or attribute setters (9.1).
// No function here makes HTML from a string.

const SVG_NS = "http://www.w3.org/2000/svg";
// Enumerated attributes: `false` is a value ("false"), not an absent attribute.
export const ENUMERATED_ATTRS = new Set(["spellcheck", "draggable", "contenteditable", "translate"]);
const PROPERTY_ATTRS = new Set(["value", "checked", "disabled", "hidden", "tabIndex", "open", "selected", "readOnly", "indeterminate"]);

// el("div", { class: "x", dataset: { a: "1" }, aria: { label: "y" }, on: { click: fn }, text: "t" }, child, ...)
// A child is a Node, a string, or null (skipped). A string becomes a text node.
export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === false && ENUMERATED_ATTRS.has(key.toLowerCase())) {
      node.setAttribute(key, "false");
      continue;
    }
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key === "dataset") for (const [name, v] of Object.entries(value)) node.dataset[name] = String(v);
    else if (key === "aria") for (const [name, v] of Object.entries(value)) node.setAttribute(`aria-${name}`, String(v));
    else if (key === "style") for (const [name, v] of Object.entries(value)) node.style.setProperty(name, String(v));
    else if (key === "on") for (const [name, handler] of Object.entries(value)) node.addEventListener(name, handler);
    else if (PROPERTY_ATTRS.has(key)) node[key] = value;
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  append(node, children);
  return node;
}

export function append(node, children) {
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

export function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
  return node;
}

// An icon of the sprite icons.svg. It has no text, so the caller gives a label to the control.
// main.js puts the symbols of the sprite in the page before the first paint (9.6), each id with the prefix SPRITE_PREFIX, so an icon
// is drawn with its element and never appears a moment later, as an external reference would.
export const SPRITE_PREFIX = "pupitre-icon-";
export function icon(name, size = 16) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  const use = document.createElementNS(SVG_NS, "use");
  use.setAttribute("href", `#${SPRITE_PREFIX}${name}`);
  svg.appendChild(use);
  return svg;
}

// A button with an icon and a visible or hidden label.
// options: { iconOnly, variant: "primary" | "ghost" | "danger" | "tb", size: "sm" }.
// The variant "tb" is the toolbar button of the dashboard (32 px, 14 px text, icon of 18 px, see .btn--tb in components.css).
// An icon-only button has the label as its accessible name and as its tooltip.
// An attrs.class replaces the classes of the options (older callers write "btn btn--primary" in it).
export function button(label, attrs = {}, iconName = null, { iconOnly = false, variant = null, size = null } = {}) {
  const children = [];
  if (iconName) children.push(icon(iconName, variant === "tb" ? 18 : 16));
  if (iconOnly) children.push(el("span", { class: "visually-hidden", text: label }));
  else children.push(el("span", { class: "btn__label", text: label }));
  const classes = ["btn"];
  if (variant) classes.push(`btn--${variant}`);
  if (size) classes.push(`btn--${size}`);
  if (iconOnly) classes.push("btn--icon");
  return el("button", { type: "button", class: classes.join(" "), ...(iconOnly ? { "data-tooltip": label } : {}), ...attrs }, ...children);
}

// Puts a popup under an anchor with position: fixed, inside the window, and keeps it there on scroll and resize.
// The popup must be in the document. align: "start" or "end" (the edge of the anchor that the popup follows).
// It returns a function that stops the follow. The popup keeps its own content and its own width.
export function anchorPopup(popup, anchor, { align = "start", gap = 4 } = {}) {
  popup.classList.add("popup");
  const place = () => {
    if (!popup.isConnected || !anchor.isConnected) return;
    const box = anchor.getBoundingClientRect();
    const viewport = document.documentElement.clientWidth;
    const width = popup.offsetWidth;
    const left = align === "end" ? box.right - width : box.left;
    const top = box.bottom + gap;
    popup.style.left = `${Math.round(Math.max(8, Math.min(left, viewport - width - 8)))}px`;
    popup.style.top = `${Math.round(top)}px`;
    popup.style.setProperty("--popup-avail", `${Math.round(Math.max(160, window.innerHeight - top - 8))}px`);
  };
  place();
  window.addEventListener("resize", place);
  window.addEventListener("scroll", place, true);
  return () => {
    window.removeEventListener("resize", place);
    window.removeEventListener("scroll", place, true);
  };
}

// The picture of the pages "not found" (9.8): a dashboard card with flat axes under a magnifier that holds a question mark.
// It is drawn with SVG elements (no markup text) and takes its colours from CSS variables (css .illus), so both themes work.
export function notFoundIllustration() {
  const NS = "http://www.w3.org/2000/svg";
  const node = (tag, attrs = {}, ...kids) => {
    const n = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
    n.append(...kids);
    return n;
  };
  const svg = node("svg", { class: "illus", viewBox: "0 0 300 200", width: 300, height: 199, "aria-hidden": "true", focusable: "false" });
  svg.append(
    node("path", { class: "illus__blob", d: "M44 112C30 62 68 14 132 10c56-4 112 12 128 56 14 40-6 84-62 108-50 22-140 12-154-62z" }),
    node("rect", { class: "illus__card", x: 62, y: 46, width: 138, height: 94, rx: 10 }),
    node("rect", { class: "illus__bar", x: 74, y: 58, width: 46, height: 7, rx: 3.5 }),
    node("path", { class: "illus__axis", d: "M74 78v50h114M74 103h114" }),
    node("path", { class: "illus__trace", d: "M80 120l22-14 18 6 24-26 20 12" }),
    node("circle", { class: "illus__lens-fill", cx: 190, cy: 112, r: 36 }),
    node("path", { class: "illus__handle", d: "M215 138l30 30" }),
    node("circle", { class: "illus__lens", cx: 190, cy: 112, r: 36 }),
    node("path", { class: "illus__mark", d: "M178 104a12 12 0 1 1 18 10c-4 2-6 5-6 10" }),
    node("circle", { class: "illus__dot", cx: 190, cy: 134, r: 3 }),
    node("path", { class: "illus__spark", d: "M246 52v14M239 59h14M52 150v10M47 155h10M262 120v8M258 124h8" }),
    node("circle", { class: "illus__dot illus__dot--soft", cx: 36, cy: 70, r: 3 }),
    node("circle", { class: "illus__dot illus__dot--soft", cx: 268, cy: 92, r: 2.5 }),
  );
  return svg;
}

// A small status label. tone: "ok" | "warn" | "error" | "info" | "accent" | null (neutral).
export function badge(text, tone = null) {
  return el("span", { class: `badge${tone ? ` badge--${tone}` : ""}`, text });
}

// A filter-like label that the user does not act on.
export function tag(text) {
  return el("span", { class: "tag", text });
}

// Shows a small label on hover and on keyboard focus. The control keeps its own accessible name.
export function tooltip(node, text, align = null) {
  node.setAttribute("data-tooltip", text);
  if (align) node.setAttribute("data-tooltip-align", align);
  return node;
}

// An empty state: an icon, a title, a text and some actions.
// emptyState({ icon: "inbox", title, text, actions: [node, ...], inline: false, testid })
export function emptyState({ icon: iconName = "inbox", title, text = null, actions = [], inline = false, testid = null } = {}) {
  const node = el("div", { class: `empty${inline ? " empty--inline" : ""}`, dataset: testid ? { testid } : {} });
  if (iconName) node.append(el("div", { class: "empty__icon" }, icon(iconName, 20)));
  if (title) node.append(el("p", { class: "empty__title", text: title }));
  if (text) node.append(el("p", { class: "empty__text", text }));
  if (actions.length) node.append(el("div", { class: "empty__actions" }, ...actions));
  return node;
}

// A checkbox or a toggle switch with its label. Returns { el, input }.
export function toggle({ label, checked = false, onChange = null, kind = "switch", testid = null }) {
  const input = el("input", { type: "checkbox", class: kind === "switch" ? "toggle" : null, role: kind === "switch" ? "switch" : null, checked, dataset: testid ? { testid } : {}, on: onChange ? { change: (event) => onChange(event.target.checked, event) } : {} });
  return { el: el("label", { class: "check" }, input, el("span", { text: label })), input };
}

// A segmented control: one pressed button in a group. Returns { el, set(value), value() }.
// segmented({ label, items: [{ value, label, icon, testid }], value, onChange(value) })
export function segmented({ label, items, value = null, onChange = null }) {
  let current = value;
  const nodes = new Map();
  const group = el("div", { class: "seg", role: "group", "aria-label": label || null });
  const paint = () => nodes.forEach((node, key) => node.setAttribute("aria-pressed", String(key === current)));
  for (const item of items) {
    const node = el("button", {
      type: "button",
      class: "seg__item",
      dataset: item.testid ? { testid: item.testid } : {},
      on: {
        click: () => {
          if (current === item.value) return;
          current = item.value;
          paint();
          if (onChange) onChange(current);
        },
      },
    }, item.icon ? icon(item.icon, 14) : null, item.label ? el("span", { text: item.label }) : null);
    if (!item.label) node.setAttribute("aria-label", item.title || String(item.value));
    nodes.set(item.value, node);
    group.append(node);
  }
  paint();
  return { el: group, set(next) { current = next; paint(); }, value: () => current };
}

// --- notices ----------------------------------------------------------------------------------

// notify({ level: "info" | "success" | "warning" | "error", text, key, action: { label, onClick } }).
// A notice with a key replaces an earlier notice with the same key.
export function notify({ level = "info", text, key = null, action = null, actions = null }) {
  const area = document.getElementById("app-notices");
  if (!area) return null;
  if (key) {
    for (const old of area.querySelectorAll("[data-notice-key]")) if (old.dataset.noticeKey === key) old.remove();
  }
  const close = el("button", { type: "button", class: "btn btn--ghost btn--icon btn--sm notice__close", "aria-label": "Close the notice", on: { click: () => node.remove() } }, icon("close", 14));
  const glyph = level === "error" ? "error" : level === "warning" ? "warning" : level === "success" ? "success" : "info";
  const parts = [icon(glyph, 16), el("span", { class: "notice__text", text })];
  for (const a of actions || (action ? [action] : [])) parts.push(el("button", { type: "button", class: "btn btn--sm", text: a.label, on: { click: () => a.onClick(node) } }));
  parts.push(close);
  const node = el("div", { class: `notice notice--${level}`, dataset: { testid: "notice", level, ...(key ? { noticeKey: key } : {}) } }, ...parts);
  area.appendChild(node);
  return node;
}

export function dismissNotice(key) {
  const area = document.getElementById("app-notices");
  if (!area) return;
  for (const old of area.querySelectorAll("[data-notice-key]")) if (old.dataset.noticeKey === key) old.remove();
}

// --- menus --------------------------------------------------------------------------------------

let openMenuState = null;

export function closeMenu({ restoreFocus = true } = {}) {
  if (!openMenuState) return;
  const { popup, anchor, cleanup } = openMenuState;
  openMenuState = null;
  cleanup();
  popup.remove();
  anchor.setAttribute("aria-expanded", "false");
  if (restoreFocus) anchor.focus();
}

// items: [{ label, onSelect, href, disabled, separator, icon, danger, heading }]. The anchor button opens it.
// A heading item is a small label that groups the items under it. It is not a menu item.
// An item icon is the name of an icon of the sprite. It shows at 16 px, 8 px before the label (css .menu__item).
// gap: the distance in pixels between the anchor and the menu.
export function openMenu(anchor, items, { testid = null, align = "start", gap = 8 } = {}) {
  closeMenu({ restoreFocus: false });
  const popup = el("div", { class: "menu", role: "menu", dataset: testid ? { testid } : {} });
  const entries = [];
  for (const item of items) {
    if (item.separator) {
      popup.appendChild(el("div", { class: "menu__sep", role: "separator" }));
      continue;
    }
    if (item.heading) {
      popup.appendChild(el("div", { class: "menu__label", role: "presentation", text: item.heading }));
      continue;
    }
    // A checked item is one choice of a group (the refresh interval): role menuitemradio, with a neutral fill.
    const attrs = {
      class: `menu__item${item.danger ? " menu__item--danger" : ""}${item.checked !== undefined ? " menu__item--choice" : ""}`,
      role: item.checked !== undefined ? "menuitemradio" : "menuitem",
      "aria-checked": item.checked !== undefined ? String(item.checked === true) : null,
      tabIndex: -1,
      disabled: item.disabled,
    };
    const onClick = (event) => {
      // An in-app link (with onSelect) changes the view with pushState. An external link keeps the action of the browser.
      if (item.href && item.onSelect && !item.external && !event.defaultPrevented && !(event.ctrlKey || event.metaKey || event.shiftKey)) event.preventDefault();
      closeMenu();
      if (item.onSelect) item.onSelect(event);
    };
    const glyph = item.icon ? icon(item.icon, 16) : null;
    const node = item.href
      ? el("a", { ...attrs, href: item.href, target: item.blank ? "_blank" : null, rel: item.blank ? "noopener noreferrer" : null, dataset: item.external ? { external: "true" } : {}, on: { click: onClick } }, glyph, item.label)
      : el("button", { ...attrs, type: "button", on: { click: onClick } }, glyph, item.label);
    if (item.testid) node.dataset.testid = item.testid;
    popup.appendChild(node);
    if (!item.disabled) entries.push(node);
  }
  document.body.appendChild(popup);
  const box = anchor.getBoundingClientRect();
  const width = popup.offsetWidth;
  const left = align === "end" ? box.right - width : box.left;
  popup.style.left = `${Math.max(4, Math.min(left, window.innerWidth - width - 4))}px`;
  popup.style.top = `${Math.max(4, Math.min(box.bottom + gap, window.innerHeight - popup.offsetHeight - 4))}px`;
  anchor.setAttribute("aria-haspopup", "menu");
  anchor.setAttribute("aria-expanded", "true");
  const onDocDown = (event) => {
    if (!popup.contains(event.target) && !anchor.contains(event.target)) closeMenu({ restoreFocus: false });
  };
  const onKey = (event) => {
    const at = entries.indexOf(document.activeElement);
    if (event.key === "Escape") {
      event.preventDefault();
      closeMenu();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      entries[(at + 1) % entries.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      entries[(at - 1 + entries.length) % entries.length]?.focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      entries[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      entries[entries.length - 1]?.focus();
    } else if (event.key === "Tab") {
      closeMenu({ restoreFocus: false });
    }
  };
  document.addEventListener("pointerdown", onDocDown, true);
  popup.addEventListener("keydown", onKey);
  anchor.addEventListener("keydown", onKey);
  openMenuState = {
    popup,
    anchor,
    cleanup() {
      document.removeEventListener("pointerdown", onDocDown, true);
      anchor.removeEventListener("keydown", onKey);
    },
  };
  (entries.find((node) => node.getAttribute("aria-checked") === "true") || entries[0])?.focus();
  return popup;
}

// --- dialogs ------------------------------------------------------------------------------------

// A modal dialog with the native <dialog> element: it traps the focus and closes on Escape.
// build(body, dialog) fills the body. buttons: [{ label, primary, danger, onClick(dialog) }].
// A destructive action has danger: true (a red button). onClick can return false to keep the dialog open.
// The focus returns to the element that had it before.
// `drawer: true` makes a panel at the right edge of the window (full height) in place of a centred modal; `subtitle` is a muted
// line under the title of a drawer. The structure (head, body, footer) and the test hooks are the same in both forms.
export function openDialog({ title, build, buttons = [{ label: "Close" }], testid = null, wide = false, drawer = false, subtitle = null }) {
  const previous = document.activeElement;
  const titleId = `dlg-${Math.random().toString(36).slice(2, 8)}`;
  const body = el("div", { class: "dialog__body" });
  const dialog = el("dialog", { class: `dialog${wide ? " dialog--wide" : ""}${drawer ? " dialog--drawer" : ""}`, "aria-labelledby": titleId, dataset: testid ? { testid } : {} });
  const footer = el("div", { class: "dialog__footer" });
  for (const spec of buttons) {
    footer.appendChild(
      el("button", {
        type: "button",
        class: `btn${spec.danger ? " btn--danger" : spec.primary ? " btn--primary" : ""}`,
        text: spec.label,
        dataset: spec.autofocus ? { autofocus: "1" } : {},
        on: {
          click: () => {
            if (spec.onClick && spec.onClick(dialog) === false) return;
            dialog.close();
          },
        },
      }),
    );
  }
  dialog.append(
    el("div", { class: "dialog__head" },
      el("div", { class: "dialog__titles" }, el("h2", { id: titleId, class: "dialog__title", text: title }), ...(subtitle ? [el("p", { class: "dialog__subtitle", text: subtitle })] : [])),
      el("button", { type: "button", class: "btn btn--ghost btn--icon dialog__close", "aria-label": "Close", on: { click: () => dialog.close() } }, icon("close", 16))),
    body,
    footer,
  );
  dialog.addEventListener("close", () => {
    dialog.remove();
    if (previous && previous.isConnected && typeof previous.focus === "function") previous.focus();
  });
  build(body, dialog);
  document.body.appendChild(dialog);
  dialog.showModal();
  // The focus goes to the first field. A dialog with no field puts it on the button that has autofocus, else on the main button.
  // A button that gets the focus here shows the ring even after a click (is-initial-focus), until it loses the focus.
  const field = body.querySelector("input:not([type=hidden]):not([type=file]):not([disabled]), select:not([disabled]), textarea:not([disabled])");
  const target = field || footer.querySelector("[data-autofocus]") || footer.querySelector(".btn--primary, .btn--danger");
  if (target) {
    target.focus();
    if (target.classList.contains("btn")) {
      target.classList.add("is-initial-focus");
      target.addEventListener("blur", () => target.classList.remove("is-initial-focus"), { once: true });
    }
  }
  return dialog;
}

// --- clipboard ------------------------------------------------------------------------------------

// Copies a text. It returns true on success. A failure is not an error of the page.
export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (error) {
    return false;
  }
}

// --- time texts -----------------------------------------------------------------------------------

const RELATIVE_UNITS = [
  ["year", 31536000000],
  ["month", 2592000000],
  ["day", 86400000],
  ["hour", 3600000],
  ["minute", 60000],
];

// "3 hours ago" (9.7). A time in the future or below one minute gives "just now".
export function relativeTime(iso, now = Date.now()) {
  const then = Date.parse(iso);
  if (!Number.isFinite(then)) return "";
  const diff = now - then;
  for (const [name, size] of RELATIVE_UNITS) {
    if (diff >= size) {
      const count = Math.floor(diff / size);
      return `${count} ${name}${count === 1 ? "" : "s"} ago`;
    }
  }
  return "just now";
}

// The absolute time in the browser zone, for a title attribute.
export function absoluteTime(iso) {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "";
  const d = new Date(ms);
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

// Makes a unique id for aria relations.
let idCounter = 0;
export function uniqueId(prefix = "id") {
  idCounter += 1;
  return `${prefix}-${idCounter}`;
}
