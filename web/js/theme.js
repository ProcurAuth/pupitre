// Copied from Hune, web/js/theme.js, commit a55fff3. Edits: the key of the storage is pupitre.theme; the event is pupitre-theme; the numbers of the Hune paragraphs stay in the comments.
// The theme selector of the header, on every page. It has the structure and the motion of the selector of ClickHouse Dash:
// one icon button that shows the choice (System, Dark or Light), and a menu of three icon options that hangs from it, joined to it.
// The choice is in localStorage key "pupitre.theme". boot_theme.js applies it before the first paint; this module changes it.
// A change sends the event "pupitre-theme" on document: the pages that draw on a canvas draw again.
// The menu opens and closes with a short motion (opacity and a move of 6 px, 160 ms). With prefers-reduced-motion it has none.

import { el, icon } from "./ui.js";

// The choices in the order of the menu, their labels and icons.
export const THEME_CHOICES = ["system", "dark", "light"];
const LABELS = { system: "System", dark: "Dark", light: "Light" };
const ICONS = { system: "monitor", dark: "moon", light: "sun" };
const KEY = "pupitre.theme";
// The motion of the menu: the token --dur-base of the copied tokens. It is under the 200 ms of 9.26.
export const THEME_MOTION_MS = 160;

// A stored value is a choice, else "system".
export function normalizeChoice(value) {
  return THEME_CHOICES.includes(value) ? value : "system";
}

// The theme that applies for a choice: "dark" or "light". "system" follows the preference of the system.
export function resolveTheme(choice, systemPrefersDark) {
  const value = normalizeChoice(choice);
  return value === "system" ? (systemPrefersDark ? "dark" : "light") : value;
}

// The option that a key reaches in the open menu, or -1 for a key that does not move. `at` is the index of the focus (-1 for none).
export function menuTarget(key, at, count) {
  if (count <= 0) return -1;
  if (key === "Home" || key === "PageUp") return 0;
  if (key === "End" || key === "PageDown") return count - 1;
  if (key === "ArrowDown") return at < 0 ? 0 : (at + 1) % count;
  if (key === "ArrowUp") return at < 0 ? count - 1 : (at - 1 + count) % count;
  return -1;
}

export const buttonLabel = (choice) => `Theme: ${LABELS[normalizeChoice(choice)]}`;

let memory = null;

export function readTheme() {
  try {
    return normalizeChoice(window.localStorage.getItem(KEY));
  } catch (error) {
    // Blocked storage: the default applies.
    return "system";
  }
}

// The choice of the viewer: "system", "dark" or "light". It is not the resolved theme (that is data-theme of <html>).
export function currentTheme() {
  return memory || readTheme();
}

export function setTheme(value) {
  if (!THEME_CHOICES.includes(value)) return;
  try {
    window.localStorage.setItem(KEY, value);
  } catch (error) {
    // The choice then lasts for this page only.
  }
  memory = value;
  document.dispatchEvent(new Event("pupitre-theme"));
}

const reducedMotion = () => {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch (error) {
    return false;
  }
};

let counter = 0;

// Builds the selector. Each call makes one control; the header makes one.
export function themeControl() {
  const menuId = `theme-menu-${++counter}`;
  // The button holds the three icons. The css shows the one of data-theme-choice, which boot_theme.js sets before the first paint.
  const current = el("span", { class: "theme-select__current", "aria-hidden": "true" });
  for (const value of THEME_CHOICES) {
    const svg = icon(ICONS[value], 18);
    svg.classList.add("theme-icon", `theme-icon--${value}`);
    current.append(svg);
  }
  const trigger = el("button", {
    type: "button",
    class: "btn btn--tb btn--icon theme-select__button",
    "aria-haspopup": "listbox",
    "aria-expanded": "false",
    "aria-controls": menuId,
    dataset: { testid: "theme-toggle", tooltipAlign: "end" },
  }, current);
  const options = new Map();
  const menu = el("div", { id: menuId, class: "theme-select__menu", role: "listbox", "aria-label": "Theme", tabindex: "-1", hidden: true, dataset: { testid: "theme-menu" } });
  for (const value of THEME_CHOICES) {
    const svg = icon(ICONS[value], 18);
    const option = el("button", {
      type: "button",
      class: "theme-select__option",
      role: "option",
      tabindex: "-1",
      title: LABELS[value],
      "aria-label": `${LABELS[value]} theme`,
      "aria-selected": "false",
      dataset: { value, testid: `theme-${value}` },
      on: { click: () => pick(value) },
    }, svg);
    options.set(value, option);
    menu.append(option);
  }
  const root = el("div", { class: "theme-select", dataset: { testid: "theme-select" } }, trigger, menu);
  let timer = 0;

  const isOpen = () => trigger.getAttribute("aria-expanded") === "true";
  const list = () => [...options.values()];

  function paint() {
    const choice = currentTheme();
    trigger.setAttribute("aria-label", buttonLabel(choice));
    trigger.setAttribute("data-tooltip", buttonLabel(choice));
    for (const [value, node] of options) node.setAttribute("aria-selected", String(value === choice));
  }

  function open({ focus = "menu" } = {}) {
    if (isOpen()) return;
    clearTimeout(timer);
    root.classList.remove("theme-select--closing");
    menu.hidden = false;
    trigger.setAttribute("aria-expanded", "true");
    // The closed style is laid out first, so that the open class runs the motion of the open state.
    void menu.offsetHeight;
    root.classList.add("theme-select--open");
    const nodes = list();
    if (focus === "first") nodes[0].focus();
    else if (focus === "last") nodes[nodes.length - 1].focus();
    else menu.focus({ preventScroll: true });
    document.addEventListener("pointerdown", onOutside, true);
  }

  function close({ focus = false } = {}) {
    if (!isOpen()) return;
    document.removeEventListener("pointerdown", onOutside, true);
    trigger.setAttribute("aria-expanded", "false");
    const finish = () => {
      timer = 0;
      menu.hidden = true;
      root.classList.remove("theme-select--closing", "theme-select--open");
    };
    clearTimeout(timer);
    if (reducedMotion()) {
      finish();
    } else {
      root.classList.add("theme-select--closing");
      requestAnimationFrame(() => {
        if (!isOpen()) root.classList.remove("theme-select--open");
      });
      timer = setTimeout(() => {
        if (!isOpen()) finish();
      }, THEME_MOTION_MS);
    }
    if (focus) trigger.focus({ preventScroll: true });
  }

  function pick(value) {
    setTheme(value);
    paint();
    close({ focus: true });
  }

  function onOutside(event) {
    if (!root.contains(event.target)) close();
  }

  trigger.addEventListener("click", (event) => {
    if (isOpen()) close();
    else open({ focus: event.detail > 0 ? "menu" : "first" });
  });
  trigger.addEventListener("keydown", (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (isOpen()) list()[event.key === "ArrowUp" ? list().length - 1 : 0].focus();
      else open({ focus: event.key === "ArrowUp" ? "last" : "first" });
    }
  });
  menu.addEventListener("keydown", (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === "Escape") {
      event.preventDefault();
      close({ focus: true });
    } else if (event.key === "Tab") {
      close();
    } else {
      const nodes = list();
      const target = menuTarget(event.key, nodes.indexOf(document.activeElement), nodes.length);
      if (target >= 0) {
        event.preventDefault();
        nodes[target].focus();
      }
    }
  });
  // The choice changes elsewhere (another tab, the system): the button shows it.
  document.addEventListener("pupitre-theme", paint);
  window.addEventListener("storage", (event) => {
    if (event.key === KEY) {
      memory = null;
      paint();
    }
  });
  paint();
  return root;
}
