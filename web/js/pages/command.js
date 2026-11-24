// The page Launch a command (5.21, 4.35): a form from the parameters, the hosts and the confirmation.
// The page checks each parameter with the rules of 4.33 before the request. The server checks again. The page never changes a value.
// A command with check = true goes through a check, then an apply (the same flow as a unit). A command with check = false asks a confirmation
// in place and runs at once.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { errorText } from "../errors.js";
import { el, icon } from "../ui.js";
import { loadCatalog } from "../lib/catalog.js";
import { field, loadingBlock, pageHead, textInput } from "../lib/dom.js";
import { createHostbox } from "../lib/hostbox.js";
import { createRunFlow } from "../lib/runflow.js";
import { checkMessage, checkParam, paramExpected, readParam } from "../validate.js";

// The control of one parameter and a function that reads it: { value } | { absent } | { error }.
function paramControl(p) {
  const hint = `Expected: ${paramExpected(p)}.${p.required ? " Necessary." : ""}`;
  let control;
  if (p.type === "choice" || p.type === "boolean") {
    const options = p.type === "boolean" ? ["true", "false"] : p.choices;
    control = el("select", { class: "input", dataset: { testid: `param-${p.name}` } },
      el("option", { value: "", text: p.required ? "Choose a value" : "Not set", disabled: p.required, selected: true }),
      ...options.map((o) => el("option", { value: o, text: o })));
  } else {
    control = textInput({ dataset: { testid: `param-${p.name}` }, inputmode: p.type === "integer" ? "numeric" : null });
  }
  const f = field({ label: p.label, control, hint });
  const read = () => {
    if (p.type === "boolean") {
      if (control.value === "") return p.required ? { error: `expected ${paramExpected(p)}, found nothing` } : { absent: true };
      return { value: control.value === "true" };
    }
    const parsed = readParam(p, control.value);
    if (parsed.error || parsed.absent) return parsed;
    const message = checkParam(p, parsed.value);
    return message ? { error: message } : parsed;
  };
  return { field: f, read, control };
}

export async function mount(ctx, route) {
  const { main, header, session } = ctx;
  const controller = new AbortController();
  const signal = controller.signal;
  const { env } = route;
  main.append(loadingBlock());
  let catalog;
  let hostItems;
  let unitItems;
  try {
    catalog = await loadCatalog(signal);
    const envDoc = await api.getDoc(api.paths.environment(env), { signal });
    if (!envDoc) throw Object.assign(new Error("not found"), { name: "ApiError", status: 404 });
    [hostItems, unitItems] = await Promise.all([
      api.listAll(api.paths.hosts(env), {}, { signal }).then((r) => r.items),
      api.listAll(api.paths.units(env), {}, { signal }).then((r) => r.items),
    ]);
  } catch (error) {
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  const command = (catalog.commands || []).find((c) => c.id === route.command);
  if (!command) {
    ctx.pageError({ status: 404 });
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren();
  document.title = `${command.label} - Pupitre`;
  header.setCrumbs([{ text: "Environments", href: "." }, { text: env, href: pageUrl("e", env) }, { text: "Commands", href: pageUrl("e", env, "commands") }, { text: command.label }]);

  const max = session.limits.max_hosts;
  const target0 = command.target;
  const addressOf = new Map(hostItems.map((h) => [h.id, (h.document || {}).address]));
  const unitById = new Map(unitItems.map((u) => [u.id, u]));
  const choose = (text) => el("option", { value: "", text, disabled: true, selected: true });
  const unitSelect = el("select", { class: "input", dataset: { testid: "unit" } }, choose("Choose a unit"), ...unitItems.filter((u) => !(u.errors && u.errors.length)).map((u) => el("option", { value: u.id, text: u.label || u.id })));
  const hostSelect = el("select", { class: "input", dataset: { testid: "host" } }, choose("Choose a host"));
  const unitField = field({ label: "Unit", control: unitSelect });
  const hostField = field({ label: "Host", control: hostSelect });
  const hostbox = createHostbox({ hosts: [], selected: [], max, onChange: sync, emptyText: target0 === "environment" ? "This environment has no host." : "Choose a unit first." });
  const hostsField = el("div", { class: "field" }, el("span", { class: "field__label", text: "Hosts" }), hostbox.el);
  const message = textInput({ maxlength: 500, placeholder: "Message for the run (optional)", "aria-label": "Message for the run", dataset: { testid: "run-message" } });
  const messageField = field({ label: "Message", control: message });
  const params = command.params.map(paramControl);
  const paramsBox = params.length ? el("div", { class: "params" }, ...params.map((p) => p.field.el)) : null;

  // The node of the launch (3.8): its type is the target of the command.
  const target = command.target;
  function chosenNode() {
    if (target === "environment") return api.node.environment(env);
    if (target === "unit") return unitSelect.value ? api.node.unit(env, unitSelect.value) : null;
    return unitSelect.value && hostSelect.value ? api.node.host(env, unitSelect.value, hostSelect.value) : null;
  }
  function chosenHosts() {
    return target === "host" ? (hostSelect.value ? [hostSelect.value] : []) : hostbox.selected();
  }
  function setHostChoices() {
    if (target === "environment") hostbox.setHosts(hostItems.map((h) => ({ id: h.id, address: (h.document || {}).address })));
    else if (target === "unit") {
      const u = unitById.get(unitSelect.value);
      hostbox.setHosts(u ? u.hosts.map((id) => ({ id, address: addressOf.get(id) })) : [], u ? "The unit has no host." : "Choose a unit first.");
    } else {
      const u = unitById.get(unitSelect.value);
      hostSelect.replaceChildren(choose("Choose a host"), ...(u ? u.hosts.map((id) => el("option", { value: id, text: id })) : []));
    }
  }
  function readParams() {
    const out = {};
    const errors = [];
    command.params.forEach((p, i) => {
      const r = params[i].read();
      if (r.error) errors.push([p.name, r.error]);
      else if (!r.absent) out[p.name] = r.value;
    });
    return { values: out, errors };
  }
  function valid() {
    const n = chosenHosts().length;
    return !!chosenNode() && n >= 1 && n <= max && !readParams().errors.length && !checkMessage(message.value);
  }
  let touched = false;
  function sync() {
    messageField.setError(checkMessage(message.value));
    const { errors } = readParams();
    command.params.forEach((p, i) => params[i].field.setError(touched ? (errors.find(([n]) => n === p.name) || [null, null])[1] : null));
    if (flow) flow.sync();
    if (runButton) runButton.disabled = !valid();
  }
  for (const control of [message, unitSelect, hostSelect, ...params.map((p) => p.control)]) {
    control.addEventListener("input", () => { touched = true; sync(); });
    control.addEventListener("change", () => { touched = true; sync(); });
  }
  unitSelect.addEventListener("change", () => { setHostChoices(); if (hostSelect.value === "") hostSelect.selectedIndex = 0; sync(); });

  const body = (extra) => {
    const out = { ...extra };
    if (message.value) out.message = message.value;
    return out;
  };
  const requestBody = (mode) => body({ kind: "command", mode, command: command.id, node: chosenNode(), hosts: chosenHosts(), params: readParams().values });

  const form = el("div", { class: "form-page" },
    ...(target === "environment" ? [] : [el("div", { class: "form-grid" }, unitField.el, ...(target === "host" ? [hostField.el] : []))]),
    ...(target === "host" ? [] : [hostsField]),
    paramsBox, messageField.el);
  const holder = el("section", { class: "page" }, pageHead({ title: command.label, id: command.id }));
  let flow = null;
  let runButton = null;

  if (command.check) {
    flow = createRunFlow({
      ctx, signal, form, valid,
      checkBody: () => requestBody("check"),
      applyBody: (checkId) => body({ kind: "command", mode: "apply", check_run: checkId }),
      allowApply: !!(session.runner && session.runner.allow_apply),
      applyLabel: "Run the command",
    });
    holder.append(flow.el);
  } else {
    runButton = el("button", { type: "button", class: "btn btn--danger", disabled: true, dataset: { testid: "run-command" } }, icon("play", 16), el("span", { class: "btn__label", text: "Run the command" }));
    const status = el("span", { class: "note-line", role: "status" });
    const confirm = el("div", { class: "callout callout--warning confirm", dataset: { testid: "run-confirm" } }, icon("warning", 16),
      el("span", { class: "callout__text", text: `${command.label} runs at once on the picked hosts. This command has no check.` }), el("div", { class: "confirm__actions" }, runButton));
    runButton.addEventListener("click", async () => {
      touched = true;
      sync();
      if (!valid()) return;
      runButton.disabled = true;
      try {
        const created = await api.request("POST", api.paths.runs(), { body: requestBody("apply"), signal });
        await ctx.navigate(pageUrl("runs", created.id));
      } catch (error) {
        if (api.isAbort(error)) return;
        status.textContent = errorText(error);
        const items = error.extra && error.extra.errors;
        if (Array.isArray(items) && items.length) status.textContent += ` ${items.map((i) => `${i.path ? `${i.path}: ` : ""}${i.message}`).join(" ")}`;
        runButton.disabled = !valid();
      }
    });
    holder.append(form, confirm, status);
  }
  main.append(holder);
  setHostChoices();
  sync();
  ctx.setGuard(null);
  return { destroy() { controller.abort(); if (flow) flow.destroy(); } };
}
