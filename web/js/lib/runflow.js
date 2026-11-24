// The flow of a check and an apply (4.3, 4.24, 5.21): the form, the check, the result of the check, then the apply.
// The apply names the check run (4.4). It takes the node, the hosts and the parameters of the check, and the server refuses it when the
// snapshot changed, when the check is old or when another apply used the check (4.24). The page shows the confirmation in place, in a callout.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { errorText, notifyError, reasonText } from "../errors.js";
import { el, icon } from "../ui.js";
import { callout } from "../errors.js";
import { createRunView } from "./runview.js";

// opts: { ctx, signal, form: Node, valid(): bool, checkBody(): object, applyBody(checkId): object, allowApply: bool, resume: run id | "",
//         onCheckStarted(id), onReset(), applyLabel }
// resume(id, { kind, node }) shows the check run of the address only when its kind and its node are these.
export function createRunFlow({ ctx, signal, form, valid, checkBody, applyBody, allowApply, resume = "", onCheckStarted = () => {}, onReset = () => {}, applyLabel = "Apply" }) {
  let view = null;
  let checkId = null;
  let busy = false;
  const root = el("div", { class: "steps", dataset: { testid: "run-flow" } });
  const startButton = el("button", { type: "button", class: "btn btn--primary", disabled: true, dataset: { testid: "start-check" } }, icon("play", 16), el("span", { class: "btn__label", text: "Start check" }));
  const startNote = el("span", { class: "note-line", role: "status" });
  const formBox = el("div", { class: "step__form" }, form);
  const step1 = step(1, "Check", el("div", { class: "step__stack" }, formBox, el("div", { class: "toolbar-row" }, startButton, startNote)));
  // The result and the apply are one step, so the apply is next to the verdict. The run of the check shows below, as the details.
  const verdictSlot = el("div", { dataset: { testid: "check-result" } });
  const applySlot = el("div", { dataset: { testid: "apply-box" } });
  const waiting = el("p", { class: "note-line", role: "status", text: "The check runs. The result shows here." });
  const step2 = step(2, "Result of the check", el("div", { class: "step__stack" }, waiting, verdictSlot, applySlot));
  const detailSlot = el("div", { class: "step__details", dataset: { testid: "check-details" } });
  step2.el.hidden = true;
  root.append(step1.el, step2.el, detailSlot);

  function step(no, title, body) {
    const node = el("div", { class: "step", dataset: { state: "active", step: String(no) } },
      el("div", { class: "step__head" }, el("span", { class: "step__no", text: String(no) }), el("h2", { class: "step__title", text: title })),
      el("div", { class: "step__body" }, body));
    return { el: node, set: (state) => { node.dataset.state = state; } };
  }

  function sync() {
    startButton.disabled = busy || !!checkId || !valid();
  }

  function lock(on) {
    if (on) formBox.setAttribute("inert", "");
    else formBox.removeAttribute("inert");
    formBox.classList.toggle("step__form--locked", on);
  }

  function showRun(record) {
    checkId = record.id;
    lock(true);
    step1.set("done");
    step2.el.hidden = false;
    step2.set("active");
    waiting.hidden = false;
    verdictSlot.replaceChildren();
    applySlot.replaceChildren();
    detailSlot.replaceChildren();
    view = createRunView({ ctx, runId: record.id, signal, onFinal: (r) => finished(r) });
    detailSlot.append(view.el);
    return view.start().catch((error) => {
      if (!api.isAbort(error)) notifyError(error);
    });
  }

  function newCheckButton() {
    const again = el("button", { type: "button", class: "btn", text: "Start a new check", dataset: { testid: "new-check" } });
    again.addEventListener("click", reset);
    return again;
  }

  function finished(record) {
    waiting.hidden = true;
    if (record.status === "passed") {
      step2.set("done");
      verdictSlot.replaceChildren(callout("success", "The check passed. Read the events and the output below, then apply."));
      showApply(record);
      return;
    }
    verdictSlot.replaceChildren(callout("error", `The check did not pass.${record.reason ? ` ${reasonText(record.reason)}` : ""}`, newCheckButton()));
  }

  function showApply(record) {
    applySlot.replaceChildren();
    if (!allowApply) {
      applySlot.append(callout("info", "Apply is off in this mode. The check shows what an apply would do."));
      return;
    }
    const apply = el("button", { type: "button", class: "btn btn--danger", dataset: { testid: "apply" } }, icon("play", 16), el("span", { class: "btn__label", text: applyLabel }));
    const status = el("span", { class: "note-line", role: "status" });
    const box = el("div", { class: "callout callout--warning confirm", dataset: { testid: "apply-confirm" } },
      icon("warning", 16),
      el("span", { class: "callout__text", text: `Apply the check to ${record.hosts.join(", ")}. The apply stops if a value changes after the check, or if the check is too old.` }),
      el("div", { class: "confirm__actions" }, apply, newCheckButton()));
    applySlot.append(box, status);
    apply.addEventListener("click", async () => {
      apply.disabled = true;
      status.textContent = "";
      try {
        const created = await api.request("POST", api.paths.runs(), { body: applyBody(record.id), signal });
        await ctx.navigate(pageUrl("runs", created.id));
      } catch (error) {
        if (api.isAbort(error)) return;
        if (["check_too_old", "check_used", "check_required"].includes(error.code)) status.textContent = errorText(error);
        else notifyError(error);
        apply.disabled = false;
      }
    });
  }

  function reset() {
    if (view) view.destroy();
    view = null;
    checkId = null;
    verdictSlot.replaceChildren();
    applySlot.replaceChildren();
    detailSlot.replaceChildren();
    step2.el.hidden = true;
    step1.set("active");
    lock(false);
    startNote.textContent = "";
    onReset();
    sync();
  }

  startButton.addEventListener("click", async () => {
    if (!valid()) return;
    busy = true;
    sync();
    startNote.textContent = "";
    try {
      const record = await api.request("POST", api.paths.runs(), { body: checkBody(), signal });
      onCheckStarted(record.id);
      await showRun(record);
    } catch (error) {
      if (!api.isAbort(error)) {
        startNote.textContent = errorText(error);
        const items = error.extra && error.extra.errors;
        if (Array.isArray(items) && items.length) startNote.textContent += ` ${items.map((i) => `${i.path ? `${i.path}: ` : ""}${i.message}`).join(" ")}`;
      }
    } finally {
      busy = false;
      sync();
    }
  });

  return {
    el: root,
    sync,
    // Shows the run of a check that the address names (the page was reloaded). The address is not trusted: the record must be a check run
    // of the kind and the node of this page, else the page starts clean. A link can name any run, and the apply names the check it shows.
    async resume(id, expect) {
      try {
        const record = await api.request("GET", api.paths.run(id), { signal });
        if (record.mode === "check" && record.kind === expect.kind && record.node === expect.node) await showRun(record);
        else onReset();
      } catch (error) {
        if (!api.isAbort(error)) onReset();
      }
    },
    destroy() {
      if (view) view.destroy();
    },
  };
}
