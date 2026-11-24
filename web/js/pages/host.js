// The page Host (5.21): the address, the port and the labels of a host of one environment. The same form creates a host (?tab=hosts&new=1).

import * as api from "../api.js";
import { fillErrorBox, notifyError } from "../errors.js";
import { canonical } from "../canonical.js";
import { el } from "../ui.js";
import { loadCatalog, groupsOf } from "../lib/catalog.js";
import { showConflict } from "../lib/conflict.js";
import { confirmBox, loadingBlock, pageHead, toolButton } from "../lib/dom.js";
import { createHostForm } from "../lib/host_form.js";
import { pageUrl } from "../config.js";

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const { env, host } = route;
  document.title = `${host} - Pupitre`;
  header.setCrumbs([{ text: "Environments", href: "." }, { text: env, href: pageUrl("e", env) }, { text: "Hosts", href: `${pageUrl("e", env)}?tab=hosts` }, { text: host }]);
  main.append(loadingBlock());
  let loaded;
  let catalog;
  let units;
  try {
    [loaded, catalog, units] = await Promise.all([
      api.getDoc(api.paths.host(env, host), { signal: controller.signal }),
      loadCatalog(controller.signal),
      api.listAll(api.paths.units(env), {}, { signal: controller.signal }).then((r) => r.items).catch(() => []),
    ]);
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  if (!loaded) {
    ctx.pageError({ status: 404 });
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren();
  let etag = loaded.etag;
  const path = api.paths.host(env, host);
  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  const conflictBox = el("div", { class: "conflict-box" });
  const status = el("span", { class: "note-line", role: "status" });
  const form = createHostForm({ doc: loaded.doc, groups: groupsOf(catalog), onChange: sync });
  const save = toolButton("Save", "save", { primary: true, disabled: true, extra: "edit-save", testid: "save" });
  const confirmSlot = el("div");
  let base = canonical(loaded.doc);

  function dirty() {
    return canonical(form.getDoc()) !== base;
  }
  function sync() {
    save.disabled = !dirty() || !form.valid();
  }
  const used = units.filter((u) => Array.isArray(u.hosts) && u.hosts.includes(host));
  const facts = el("dl", { class: "facts" }, el("div", { class: "facts__item" }, el("dt", { text: "Units" }),
    el("dd", {}, ...(used.length ? used.flatMap((u, i) => [i ? ", " : null, el("a", { href: pageUrl("e", env, "units", u.id), text: u.label || u.id })]) : ["-"]))));
  const del = toolButton("Delete host", "trash", { testid: "delete", extra: "btn--ghost" });
  del.addEventListener("click", () => {
    confirmSlot.replaceChildren();
    const box = confirmBox({
      text: `Delete the host ${host}? Only a host that no unit holds can go.`,
      confirmLabel: "Delete",
      testid: "confirm-delete",
      onConfirm: async () => {
        try {
          await api.deleteDoc(path, etag);
          await ctx.navigate(`${pageUrl("e", env)}?tab=hosts`);
        } catch (error) {
          if (api.isAbort(error)) return true;
          if (error.code === "in_use") fillErrorBox(errors, error);
          else notifyError(error);
          return false;
        }
        return true;
      },
    });
    confirmSlot.append(box.el);
    box.focus();
  });
  main.append(el("section", { class: "page" }, el("div", { class: "form-page" },
    pageHead({ title: host, id: "host", tools: [el("a", { class: "btn btn--tb", href: `${pageUrl("e", env, "hosts", host, "history")}`, dataset: { testid: "history" } }, el("span", { class: "btn__label", text: "History" })), del] }),
    confirmSlot, facts, form.el, status, errors, conflictBox)));
  header.setActions(save);

  save.addEventListener("click", async () => {
    form.showAll();
    if (!form.valid()) return;
    save.disabled = true;
    errors.hidden = true;
    conflictBox.replaceChildren();
    try {
      const result = await api.putDoc(path, form.getDoc(), { etag });
      etag = result.etag;
      const fresh = await api.getDoc(path, { signal: controller.signal });
      base = canonical(fresh.doc);
      status.textContent = `Saved. The version is ${result.version}.`;
    } catch (error) {
      if (api.isAbort(error)) return;
      if (error.code === "conflict" || (error.code === "not_found")) {
        await showConflict({
          container: conflictBox, path, mineDoc: form.getDoc(), what: "host",
          onKeep: (currentEtag) => { etag = currentEtag; if (etag === null) notifyError(error); sync(); },
          onUseCurrent: () => ctx.navigate(window.location.href, { replace: true }),
        });
      } else if (error.code === "invalid_document") {
        const rest = form.setServerErrors((error.extra && error.extra.errors) || []);
        if (rest.length) fillErrorBox(errors, { ...error, extra: { errors: rest } });
      } else notifyError(error);
    } finally {
      sync();
    }
  });
  ctx.setGuard(dirty);
  sync();
  return { destroy: () => controller.abort() };
}

// The page that creates a host: the id and the fields. It writes with If-None-Match: * (5.5).
export async function mountCreate(ctx, env, container) {
  const controller = new AbortController();
  const catalog = await loadCatalog(controller.signal);
  const errors = el("div", { class: "errors", role: "alert", hidden: true });
  const submit = el("button", { type: "button", class: "btn btn--primary", text: "Create", disabled: true, dataset: { testid: "create" } });
  const cancel = el("a", { class: "btn", href: `${pageUrl("e", env)}?tab=hosts`, text: "Cancel" });
  const form = createHostForm({ doc: null, withId: true, groups: groupsOf(catalog), onChange: () => { submit.disabled = !form.valid(); } });
  submit.addEventListener("click", async () => {
    form.showAll();
    if (!form.valid()) return;
    submit.disabled = true;
    errors.hidden = true;
    const id = form.id();
    try {
      await api.putDoc(api.paths.host(env, id), form.getDoc(), { etag: null });
      await ctx.navigate(pageUrl("e", env, "hosts", id));
    } catch (error) {
      if (api.isAbort(error)) return;
      if (error.code === "already_exists") form.setIdError("A host with this id exists.");
      else if (error.code === "invalid_document") {
        const rest = form.setServerErrors((error.extra && error.extra.errors) || []);
        if (rest.length) fillErrorBox(errors, { ...error, extra: { errors: rest } });
      } else notifyError(error);
      submit.disabled = !form.valid();
    }
  });
  container.append(el("div", { class: "form-page" }, pageHead({ title: "New host", id: env }), form.el, errors, el("div", { class: "toolbar-row" }, submit, cancel)));
  ctx.setGuard(() => !!(form.id() || form.getDoc().address));
  form.focus();
  return { destroy: () => controller.abort() };
}
