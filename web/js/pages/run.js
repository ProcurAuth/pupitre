// The page Run (5.21): the record, one row for each host, the events, the output and the cancel button.

import * as api from "../api.js";
import { pageUrl } from "../config.js";
import { notifyError } from "../errors.js";
import { el } from "../ui.js";
import { confirmBox, loadingBlock, pageHead, toolButton } from "../lib/dom.js";
import { createRunView } from "../lib/runview.js";
import { isFinal } from "../lib/runs_list.js";

// The title of a run: what the run does, and where.
export function runTitle(r) {
  const parts = /^environment:([^/]+)(?:\/unit:([^/]+))?(?:\/host:([^/]+))?$/.exec(r.node || "") || [];
  const where = parts[3] || parts[2] || parts[1] || "";
  if (r.kind === "command") return `${r.command || "Command"} on ${where}`;
  if (r.kind === "status") return `Status of ${where}`;
  return r.mode === "check" ? `Check of ${where}` : `Apply to ${where}`;
}

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  const runId = route.run;
  document.title = `Run ${runId.slice(0, 8)} - Pupitre`;
  header.setCrumbs([{ text: "Runs", href: "runs" }, { text: runId.slice(0, 8) }]);
  main.append(loadingBlock());
  const slot = el("div");
  const head = el("div", { class: "page__head" });
  const confirmSlot = el("div");
  const cancel = toolButton("Cancel the run", "stop", { extra: "btn--danger", testid: "cancel-run" });
  cancel.hidden = true;
  let titled = false;

  const view = createRunView({
    ctx, runId, signal: controller.signal,
    onRecord: (record) => {
      if (!titled) {
        titled = true;
        head.replaceChildren(el("h1", { class: "page__title", text: runTitle(record) }), el("span", { class: "page__id", text: runId }), el("div", { class: "page__tools" }, cancel));
        document.title = `${runTitle(record)} - Pupitre`;
      }
      cancel.hidden = isFinal(record.status);
      if (isFinal(record.status)) confirmSlot.replaceChildren();
    },
  });
  cancel.addEventListener("click", () => {
    confirmSlot.replaceChildren();
    const box = confirmBox({
      text: "Cancel the run? The runner stops the playbook at once. The playbook decides what a half run leaves on the hosts.",
      confirmLabel: "Cancel the run",
      testid: "confirm-cancel",
      onConfirm: async () => {
        try {
          await api.request("POST", api.paths.cancel(runId), { signal: controller.signal });
          await view.refresh();
        } catch (error) {
          if (api.isAbort(error)) return true;
          if (error.code === "run_finished") await view.refresh();
          else notifyError(error);
        }
        return true;
      },
    });
    confirmSlot.append(box.el);
    box.focus();
  });
  try {
    await view.start();
  } catch (error) {
    view.destroy();
    if (!api.isAbort(error)) ctx.pageError(error, () => ctx.navigate(window.location.href, { replace: true }));
    return { destroy: () => controller.abort() };
  }
  main.replaceChildren(el("section", { class: "run-page page" }, head, confirmSlot, view.el));
  return { destroy() { controller.abort(); view.destroy(); } };
}
