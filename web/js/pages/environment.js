// The page Environment (5.21): the tabs units, hosts, common values, secrets and runs. The tab is the query ?tab=. A tab changes in the
// page, with no new request for the head (DATA-24), and the address changes with it.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { fillErrorBox, notifyError } from "../errors.js";
import { el } from "../ui.js";
import { confirmBox, loadingBlock, pageHead, toolButton } from "../lib/dom.js";

const TAB_LABELS = { units: "Units", hosts: "Hosts", common: "Common values", secrets: "Secrets", runs: "Runs" };
const TAB_MODULES = {
  units: () => import("./env_units.js"),
  hosts: () => import("./env_hosts.js"),
  common: () => import("./env_common.js"),
  secrets: () => import("./env_secrets.js"),
  runs: () => import("./env_runs.js"),
};

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const env = route.env;
  main.append(loadingBlock());
  let loaded;
  try {
    loaded = await api.getDoc(api.paths.environment(env), { signal: controller.signal });
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  if (!loaded) {
    ctx.pageError({ status: 404 });
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren();
  const label = loaded.doc.label || env;
  const runner = ctx.session.runner || {};
  document.title = `${label} - Pupitre`;
  header.setCrumbs([{ text: "Environments", href: "." }, { text: label }]);

  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  const confirmSlot = el("div");
  const tools = [];
  if (runner.enabled && runner.allow_apply) tools.push(el("a", { class: "btn btn--tb", href: pageUrl("e", env, "commands"), dataset: { testid: "commands" } }, el("span", { class: "btn__label", text: "Commands" })));
  const del = toolButton("Delete environment", "trash", { extra: "btn--ghost", testid: "delete-environment" });
  tools.push(del);
  del.addEventListener("click", () => {
    confirmSlot.replaceChildren();
    const box = confirmBox({
      text: `Delete the environment ${label}? Only an empty environment can go.`,
      confirmLabel: "Delete",
      testid: "confirm-delete",
      onConfirm: async () => {
        try {
          await api.deleteDoc(api.paths.environment(env), loaded.etag);
          await ctx.navigate(".");
        } catch (error) {
          if (api.isAbort(error)) return true;
          if (error.code === "not_empty") fillErrorBox(errors, error);
          else notifyError(error);
          return false;
        }
        return true;
      },
    });
    confirmSlot.append(box.el);
    box.focus();
  });

  const tabNames = Object.keys(TAB_LABELS).filter((t) => t !== "runs" || runner.enabled);
  const tabsNav = el("nav", { class: "tabs", role: "tablist", "aria-label": "Sections of the environment" });
  const body = el("div", { class: "page__body", dataset: { testid: "tab-body" } });
  // A creation is a page of its own (CTRL-32): the form has its own head, and the head of the environment does not show.
  const page = el("section", { class: "page" }, ...(route.create ? [] : [pageHead({ title: label, id: env, tools }), confirmSlot, errors]));
  main.append(page);

  let tab = null;
  let current = null;
  let token = 0;
  const tabLinks = new Map();

  function tabHref(name) {
    return `${pageUrl("e", env)}${name === "units" ? "" : `?tab=${name}`}`;
  }

  async function showTab(name, { create = false, push = false } = {}) {
    const mine = ++token;
    if (current && current.destroy) current.destroy();
    current = null;
    ctx.setGuard(null);
    tab = name;
    for (const [n, link] of tabLinks) link.setAttribute("aria-selected", String(n === name));
    if (push) window.history.pushState(null, "", tabHref(name));
    header.setActions();
    body.replaceChildren();
    const mod = await TAB_MODULES[name]();
    if (mine !== token) return;
    const result = await mod.mountTab({ ctx, env, container: body, create, controller, envLabel: label, reload: () => showTab(name, { create }) });
    if (mine !== token) {
      if (result && result.destroy) result.destroy();
      return;
    }
    current = result || null;
  }

  if (route.create) {
    // A creation is a page of its own: no tabs, a form.
    page.append(body);
    await showTab(route.tab, { create: true });
  } else {
    for (const name of tabNames) {
      const link = el("a", { class: "tab", role: "tab", href: tabHref(name), "aria-selected": "false", dataset: { testid: `tab-${name}` }, text: TAB_LABELS[name] });
      link.addEventListener("click", (event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
        event.preventDefault();
        if (name === tab) return;
        ctx.confirmIfDirty(() => showTab(name, { push: true }));
      });
      tabLinks.set(name, link);
      tabsNav.append(link);
    }
    page.append(tabsNav, body);
    const first = tabNames.includes(route.tab) ? route.tab : "units";
    await showTab(first);
  }
  return {
    destroy() {
      controller.abort();
      token += 1;
      if (current && current.destroy) current.destroy();
    },
  };
}
