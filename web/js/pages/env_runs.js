// The tab Runs of an environment (5.16, 5.21): the runs of this environment, newest first.

import { el } from "../ui.js";
import { createRunsList, RUN_STATUS } from "../lib/runs_list.js";

export async function mountTab({ ctx, env, container, controller }) {
  const filters = { environment: env, status: "" };
  const statusSelect = el("select", { class: "input", "aria-label": "Status", dataset: { testid: "filter-status" } }, el("option", { value: "", text: "All statuses" }),
    ...Object.keys(RUN_STATUS).map((s) => el("option", { value: s, text: RUN_STATUS[s].label })));
  const body = el("div");
  container.append(el("div", { class: "toolbar-row" }, statusSelect), body);
  const list = createRunsList({ ctx, container: body, filters, signal: controller.signal });
  statusSelect.addEventListener("change", () => list.setFilters({ status: statusSelect.value }));
  await list.load();
  list.start();
  return { destroy: () => list.destroy() };
}
