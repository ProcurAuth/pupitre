// The page Run a unit (5.21): the hosts, the check, the result of the check, then the apply. With ?kind=status the page starts a status run.
// A check never changes a host. The apply needs the check (4.24). The address keeps the check (?check=<id>), so a reload shows it again.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { errorText, notifyError } from "../errors.js";
import { el, icon } from "../ui.js";
import { loadingBlock, pageHead, textInput, field } from "../lib/dom.js";
import { createHostbox } from "../lib/hostbox.js";
import { createRunFlow } from "../lib/runflow.js";
import { checkMessage } from "../validate.js";

export async function mount(ctx, route) {
  const { main, header, session } = ctx;
  const controller = new AbortController();
  const signal = controller.signal;
  const { env, unit } = route;
  const status = route.runKind === "status";
  main.append(loadingBlock());
  let unitDoc;
  let hostItems;
  try {
    [unitDoc, hostItems] = await Promise.all([api.getDoc(api.paths.unit(env, unit), { signal }), api.listAll(api.paths.hosts(env), {}, { signal }).then((r) => r.items)]);
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  if (!unitDoc) {
    ctx.pageError({ status: 404 });
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren();
  const label = unitDoc.doc.label || unit;
  const runner = session.runner || {};
  const path = pageUrl("e", env, "units", unit, "run");
  document.title = `${status ? "Status" : "Check and apply"} ${label} - Pupitre`;
  header.setCrumbs([{ text: "Environments", href: "." }, { text: env, href: pageUrl("e", env) }, { text: label, href: pageUrl("e", env, "units", unit) }, { text: status ? "Status" : "Run" }]);

  const byId = new Map(hostItems.map((h) => [h.id, h]));
  const unitHosts = unitDoc.doc.hosts.map((id) => ({ id, address: byId.has(id) ? (byId.get(id).document || {}).address : undefined }));
  const max = session.limits.max_hosts;
  const hostbox = createHostbox({ hosts: unitHosts, selected: unitHosts.length <= max ? unitHosts.map((h) => h.id) : [], max, onChange: sync });
  const message = textInput({ maxlength: 500, placeholder: "Message for the run (optional)", "aria-label": "Message for the run", dataset: { testid: "run-message" } });
  const messageField = field({ label: "Message", control: message });
  message.addEventListener("input", sync);
  const form = el("div", { class: "form-page" }, el("div", { class: "field" }, el("span", { class: "field__label", text: "Hosts" }), hostbox.el), messageField.el);

  const valid = () => {
    const n = hostbox.selected().length;
    return n >= 1 && n <= max && !checkMessage(message.value);
  };
  function sync() {
    messageField.setError(checkMessage(message.value));
    if (flow) flow.sync();
    if (startStatus) startStatus.disabled = !valid();
  }
  const node = api.node.unit(env, unit);
  const body = (extra) => {
    const out = { ...extra };
    if (message.value) out.message = message.value;
    return out;
  };
  let flow = null;
  let startStatus = null;
  const holder = el("section", { class: "page" }, pageHead({ title: status ? "Status" : "Check and apply", id: `${label} in ${env}` }));

  if (status) {
    startStatus = el("button", { type: "button", class: "btn btn--primary", disabled: !valid(), dataset: { testid: "start-status" } }, icon("play", 16), el("span", { class: "btn__label", text: "Run the status playbook" }));
    const note = el("span", { class: "note-line", role: "status" });
    startStatus.addEventListener("click", async () => {
      startStatus.disabled = true;
      try {
        const created = await api.request("POST", api.paths.runs(), { body: body({ kind: "status", mode: "apply", node, hosts: hostbox.selected() }), signal });
        await ctx.navigate(pageUrl("runs", created.id));
      } catch (error) {
        if (api.isAbort(error)) return;
        note.textContent = errorText(error);
        startStatus.disabled = !valid();
      }
    });
    holder.append(form, el("div", { class: "toolbar-row" }, startStatus, note));
  } else {
    flow = createRunFlow({
      ctx, signal, form,
      valid,
      checkBody: () => body({ kind: "deploy", mode: "check", node, hosts: hostbox.selected() }),
      applyBody: (checkId) => body({ kind: "deploy", mode: "apply", check_run: checkId }),
      allowApply: !!runner.allow_apply,
      onCheckStarted: (id) => window.history.replaceState(null, "", `${path}?check=${encodeURIComponent(id)}`),
      onReset: () => window.history.replaceState(null, "", path),
    });
    holder.append(flow.el);
  }
  main.append(holder);
  sync();
  if (flow && route.check) await flow.resume(route.check, { kind: "deploy", node });
  return { destroy() { controller.abort(); if (flow) flow.destroy(); } };
}
