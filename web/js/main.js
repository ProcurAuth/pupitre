// The entry of the page: the sprite, the header, the session, the router (5.21 to 5.23).
// Written for Pupitre on the model of web/js/main.js of Hune (commit a55fff3): the same shell, the same first paint, the same links.

import * as api from "./api.js";
import * as busy from "./busy.js";
import { app, assetUrl, basePath, relativePath } from "./config.js";
import { errorText } from "./errors.js";
import { parseRoute } from "./routes.js";
import { themeControl } from "./theme.js";
import { SPRITE_PREFIX, clear, el, icon, notFoundIllustration, openDialog } from "./ui.js";

const main = document.getElementById("app-main");
const headerEl = document.getElementById("app-header");

let current = null; // { destroy() }
let routeToken = 0;
let revealed = false;
let guardFn = null; // returns true while the page holds changes that are not saved

// --- header ---------------------------------------------------------------------------------------------------------------------

const brandSlot = el("div", { class: "header-brand" });
const crumbs = el("nav", { class: "crumbs", "aria-label": "Location" });
const actionsRow = el("div", { class: "header-actions__row", dataset: { testid: "header-actions" } });
const actionsSlot = el("div", { class: "header-actions" }, actionsRow);
const linkSlot = el("div", { class: "header-links" });
const subjectSlot = el("span", { class: "header-subject", hidden: true, dataset: { testid: "subject" } });
const themeSlot = el("div", { class: "header-theme" });

function buildHeader() {
  clear(headerEl);
  headerEl.append(brandSlot, crumbs, actionsSlot, linkSlot, subjectSlot, themeSlot);
  themeSlot.append(themeControl());
  trackHeaderHeight();
}

// --header-h is the real height of the header: the sticky parts of a page stick under it.
function trackHeaderHeight() {
  const root = document.documentElement;
  const measure = () => root.style.setProperty("--header-h", `${headerEl.offsetHeight}px`);
  measure();
  if (typeof ResizeObserver === "function") new ResizeObserver(measure).observe(headerEl);
  window.addEventListener("resize", measure);
}

// The name and the version show after the session answers (5.23: no product name before).
function fillBrand(session) {
  clear(brandSlot);
  const version = el("span", { id: "brand-version", class: "brand__version", text: session.version, dataset: { testid: "app-version" } });
  brandSlot.append(el("a", { class: "brand", href: ".", "aria-label": "Pupitre, the environments", "aria-describedby": "brand-version" }, el("span", { class: "brand__text" }, el("span", { class: "brand__name", text: "Pupitre" }), version)));
  clear(linkSlot);
  if (session.runner && session.runner.enabled) linkSlot.append(el("a", { class: "header-link", href: "runs", dataset: { testid: "link-runs" }, text: "Runs" }));
  // The subject is "-" in the mode open: the page shows nothing then.
  if (session.subject && session.subject !== "-") {
    subjectSlot.hidden = false;
    subjectSlot.textContent = session.subject;
    subjectSlot.setAttribute("title", `Signed in as ${session.subject}`);
  }
}

const header = {
  // parts: [{ text, href }]. The last part is the current page.
  setCrumbs(parts) {
    clear(crumbs);
    for (const part of parts) {
      crumbs.append(el("span", { class: "crumbs__sep", "aria-hidden": "true" }, icon("chevron-right", 12)));
      if (part.href !== undefined && part.href !== null) crumbs.append(el("a", { class: "crumbs__item crumbs__item--link", href: part.href, title: part.text }, el("span", { class: "crumbs__text", text: part.text })));
      else crumbs.append(el("span", { class: "crumbs__item crumbs__item--current", title: part.text, "aria-current": "page" }, el("span", { class: "crumbs__text", text: part.text })));
    }
  },
  setActions(...nodes) {
    clear(actionsRow);
    for (const node of nodes) if (node) actionsRow.append(node);
  },
  clear() {
    clear(crumbs);
    clear(actionsRow);
  },
};

// --- the first paint ---------------------------------------------------------------------------------------------------------------

// The shell holds its first paint (the attribute data-pupitre-hold, css/base.css) until the first view is complete.
function reveal() {
  if (revealed) return;
  revealed = true;
  document.documentElement.removeAttribute("data-pupitre-hold");
}

// The symbols of the sprite go in the page as one hidden svg, with the ids prefixed (ui.js icon()), so an icon never shows late.
async function loadSprite() {
  try {
    const answer = await fetch(assetUrl("icons.svg"), { credentials: "same-origin" });
    if (!answer.ok) return;
    const doc = new DOMParser().parseFromString(await answer.text(), "image/svg+xml");
    if (doc.documentElement.nodeName !== "svg") return;
    const sprite = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    sprite.setAttribute("aria-hidden", "true");
    sprite.setAttribute("focusable", "false");
    sprite.setAttribute("width", "0");
    sprite.setAttribute("height", "0");
    sprite.setAttribute("id", "pupitre-sprite");
    sprite.style.position = "absolute";
    for (const symbol of doc.querySelectorAll("symbol")) {
      const node = document.importNode(symbol, true);
      node.setAttribute("id", SPRITE_PREFIX + symbol.getAttribute("id"));
      sprite.append(node);
    }
    document.body.prepend(sprite);
  } catch (error) {
    console.error(error);
  }
}
const spriteReady = loadSprite();

// --- full pages -----------------------------------------------------------------------------------------------------------------------

// A full page: an icon or a picture, a title, a text and the actions. `icon` is the name of an icon of the sprite.
export function fullPage({ title = null, text, hint = null, code = null, icon: iconName = "info", tone = null, buttons = [], links = [], testid = "full-page" }) {
  clear(main);
  main.classList.add("app-main--full");
  const picture = iconName === "compass";
  const box = el("section", { class: `full-page${tone ? ` full-page--${tone}` : ""}${picture ? " full-page--picture" : ""}`, dataset: { testid } });
  if (picture) box.append(notFoundIllustration());
  else if (iconName) box.append(el("div", { class: "full-page__icon" }, icon(iconName, 22)));
  if (title) box.append(el("h1", { text: title }));
  box.append(el("p", { class: `full-page__text${title ? "" : " full-page__text--lead"}`, text }));
  if (hint) box.append(el("p", { class: "full-page__hint", text: hint }));
  if (code) box.append(el("p", { class: "full-page__code", text: code }));
  const row = el("div", { class: "full-page__actions" });
  buttons.forEach((b, index) => row.append(el("button", { type: "button", class: `btn${index === 0 ? " btn--primary" : ""}`, text: b.label, on: { click: b.onClick } })));
  links.forEach((l, index) => row.append(el("a", { class: `btn${buttons.length === 0 && index === 0 ? " btn--primary" : ""}`, href: l.href, text: l.label })));
  if (row.childNodes.length) box.append(row);
  main.append(box);
  return box;
}

function unauthorizedPage() {
  stopCurrent();
  header.clear();
  fullPage({ icon: "lock", tone: "warning", text: "You are not signed in, or your session ended. Reload the page.", hint: "Pupitre has no sign-in page. The sign-in service of your site does the login.", buttons: [{ label: "Reload", onClick: () => window.location.reload() }], testid: "unauthorized" });
  reveal();
}

function noAnswerPage(error, retry) {
  stopCurrent();
  header.clear();
  const forbidden = error && error.status === 403;
  fullPage({
    icon: forbidden ? "lock" : "plug-off",
    tone: forbidden ? "warning" : "error",
    text: forbidden ? errorText(error) : error && error.code && error.code !== "network_error" ? errorText(error) : "Pupitre does not answer.",
    code: error && error.requestId ? `Request ${error.requestId}` : null,
    buttons: [{ label: "Retry", onClick: retry }],
    testid: forbidden ? "forbidden" : "no-answer",
  });
  reveal();
}

function notFoundPage() {
  header.clear();
  document.title = "Page not found - Pupitre";
  fullPage({ icon: "compass", title: "Page not found", text: "This address does not exist.", links: [{ label: "Go to the environments", href: "." }], testid: "not-found" });
}

// An error of a page that the page cannot show in its own place.
export function pageError(error, retry) {
  if (error && error.status === 404) {
    notFoundPage();
    return;
  }
  header.clear();
  const forbidden = error && error.status === 403;
  fullPage({ icon: forbidden ? "lock" : "error", tone: forbidden ? "warning" : "error", text: errorText(error), code: error && error.requestId ? `Request ${error.requestId}` : null, buttons: retry ? [{ label: "Retry", onClick: retry }] : [], testid: "page-error" });
}

// --- router --------------------------------------------------------------------------------------------------------------------------------

function stopCurrent() {
  if (current && current.destroy) current.destroy();
  current = null;
  guardFn = null;
}

const PAGES = {
  environments: () => import("./pages/environments.js"),
  environment: () => import("./pages/environment.js"),
  unit: () => import("./pages/unit.js"),
  unit_host: () => import("./pages/unit.js"),
  host: () => import("./pages/host.js"),
  history: () => import("./pages/history.js"),
  run_unit: () => import("./pages/run_unit.js"),
  commands: () => import("./pages/commands.js"),
  command: () => import("./pages/command.js"),
  runs: () => import("./pages/runs.js"),
  run: () => import("./pages/run.js"),
};

const ctx = {
  main,
  header,
  navigate,
  pageError,
  // Runs `proceed` at once, or after the person confirms when the page holds changes that are not saved.
  confirmIfDirty(proceed) {
    if (guardFn && guardFn()) confirmLeave(() => {
      guardFn = null;
      proceed();
    });
    else proceed();
  },
  // A page tells the router when it holds changes: fn() returns true while they are not saved.
  setGuard(fn) {
    guardFn = fn;
  },
  get session() {
    return app.session;
  },
};

async function show() {
  const token = ++routeToken;
  const route = parseRoute(relativePath(), window.location.search);
  stopCurrent();
  document.documentElement.removeAttribute("data-pupitre-view");
  header.clear();
  clear(main);
  main.classList.remove("app-main--full");
  document.title = "Pupitre";
  busy.begin();
  try {
    if (route.kind === "none" || !PAGES[route.kind]) {
      notFoundPage();
    } else if (!available(route)) {
      notFoundPage();
    } else {
      const mod = await PAGES[route.kind]();
      const page = await mod.mount(ctx, route);
      if (token !== routeToken) {
        if (page && page.destroy) page.destroy();
        return;
      }
      current = page || null;
      document.documentElement.setAttribute("data-pupitre-view", route.kind);
    }
  } catch (error) {
    if (token === routeToken && !api.isAbort(error)) {
      if (error && error.name === "ApiError" && error.status === 401) return;
      if (error && error.name === "ApiError") pageError(error, () => show());
      else {
        console.error(error);
        pageError(error, () => show());
      }
    }
  } finally {
    busy.end();
    if (token === routeToken) {
      currentRouteHighlight();
      reveal();
    }
  }
}

// A page that the session turns off is not found (a feature that is off, 5.4).
function available(route) {
  const runner = app.session && app.session.runner;
  if (["runs", "run", "run_unit"].includes(route.kind)) return !!(runner && runner.enabled);
  if (["commands", "command"].includes(route.kind)) return !!(runner && runner.enabled && runner.allow_apply);
  return true;
}

// Changes the view with history.pushState and no reload. `url` is a URL object or a text.
export function navigate(url, { replace = false } = {}) {
  const target = new URL(url, document.baseURI);
  if (replace) window.history.replaceState(null, "", target);
  else window.history.pushState(null, "", target);
  return show();
}

function confirmLeave(proceed) {
  openDialog({
    title: "Leave this page?",
    testid: "leave-dialog",
    build: (body) => body.append(el("p", { text: "The page holds changes that are not saved. If you leave, the changes are lost." })),
    buttons: [{ label: "Stay", autofocus: true }, { label: "Leave", danger: true, onClick: () => proceed() }],
  });
}

function onClick(event) {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  const link = event.target instanceof Element ? event.target.closest("a[href]") : null;
  if (!link || link.target || link.hasAttribute("download")) return;
  const url = new URL(link.href, document.baseURI);
  const base = basePath();
  if (url.origin !== window.location.origin || !url.pathname.startsWith(base)) return;
  const rel = url.pathname.slice(base.length);
  if (rel.startsWith("api/") || rel.startsWith("static/") || rel.startsWith(".well-known/")) return;
  event.preventDefault();
  if (guardFn && guardFn()) confirmLeave(() => {
    guardFn = null;
    navigate(url);
  });
  else navigate(url);
}

// The skip link of the shell goes to <main>. The shell has a <base>, so the browser would open the list: this handler moves the focus.
function skipToMain(event) {
  event.preventDefault();
  main.setAttribute("tabindex", "-1");
  main.addEventListener("blur", () => main.removeAttribute("tabindex"), { once: true });
  main.focus();
}

function currentRouteHighlight() {
  const route = parseRoute(relativePath(), window.location.search);
  for (const link of linkSlot.querySelectorAll("a")) {
    if (route.kind === "runs" || route.kind === "run") link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
}

// --- start --------------------------------------------------------------------------------------------------------------------------------------

// Waits for the font Inter before the first render, for one second at most. It never throws.
function fontsReady() {
  try {
    const fonts = document.fonts;
    if (!fonts || typeof fonts.load !== "function") return Promise.resolve();
    let timer = 0;
    const loaded = Promise.all([fonts.load("400 14px Inter"), fonts.load("500 14px Inter")]).catch(() => {});
    const late = new Promise((resolve) => {
      timer = setTimeout(resolve, 1000);
    });
    return Promise.race([loaded, late]).then(() => clearTimeout(timer), () => clearTimeout(timer));
  } catch (error) {
    return Promise.resolve();
  }
}

let started = false;

async function start() {
  const fontWait = fontsReady();
  if (!started) {
    started = true;
    buildHeader();
    api.setUnauthorizedHandler(unauthorizedPage);
    const skip = document.getElementById("skip-link");
    if (skip) skip.addEventListener("click", skipToMain);
    document.addEventListener("click", onClick);
    window.addEventListener("popstate", () => show());
    window.addEventListener("beforeunload", (event) => {
      if (guardFn && guardFn()) {
        event.preventDefault();
        event.returnValue = "";
      }
    });
  }
  try {
    app.session = await api.session();
  } catch (error) {
    if (error && error.status === 401) return;
    // Without a session the page shows the error page and sends no write (5.22).
    noAnswerPage(error, () => {
      busy.begin();
      start().finally(busy.end);
    });
    return;
  }
  fillBrand(app.session);
  await Promise.all([fontWait, spriteReady]);
  await show();
}

busy.begin();
start()
  .catch((error) => console.error(error))
  .finally(() => {
    busy.end();
    reveal();
  });
