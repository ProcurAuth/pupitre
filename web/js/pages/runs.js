// The page Runs (5.21): the runs, newest first, with their status. The filters are the environment and the status.

import * as api from "../api.js";
import { el } from "../ui.js";
import { createRunsList, RUN_STATUS } from "../lib/runs_list.js";

const STATUSES = Object.keys(RUN_STATUS);

export async function mount(ctx, route) {
  const { main, header } = ctx;
  const controller = new AbortController();
  document.title = "Runs - Pupitre";
  header.setCrumbs([{ text: "Runs" }]);
  const filters = { environment: route.environment, status: route.status };
  let environments = [];
  try {
    environments = (await api.listAll(api.paths.environments(), {}, { signal: controller.signal })).items;
  } catch (error) {
    if (error && error.status !== 403) {
      if (!api.isAbort(error)) ctx.pageError(error);
      return { destroy: () => controller.abort() };
    }
  }
  const envSelect = el("select", { class: "input", "aria-label": "Environment", dataset: { testid: "filter-environment" } }, el("option", { value: "", text: "All environments" }),
    ...environments.map((e) => el("option", { value: e.id, text: e.label || e.id, selected: e.id === filters.environment })));
  const statusSelect = el("select", { class: "input", "aria-label": "Status", dataset: { testid: "filter-status" } }, el("option", { value: "", text: "All statuses" }),
    ...STATUSES.map((s) => el("option", { value: s, text: RUN_STATUS[s].label, selected: s === filters.status })));
  const body = el("div", { class: "runs-body" });
  const bar = el("div", { class: "toolbar-row" }, envSelect, statusSelect);
  main.append(el("section", { class: "page" }, bar, body));
  const list = createRunsList({ ctx, container: body, filters, signal: controller.signal });

  function changed() {
    const q = new URLSearchParams();
    if (envSelect.value) q.set("environment", envSelect.value);
    if (statusSelect.value) q.set("status", statusSelect.value);
    window.history.replaceState(null, "", `runs${q.toString() ? `?${q}` : ""}`);
    list.setFilters({ environment: envSelect.value, status: statusSelect.value });
  }
  envSelect.addEventListener("change", changed);
  statusSelect.addEventListener("change", changed);
  await list.load();
  list.start();
  return { destroy() { controller.abort(); list.destroy(); } };
}
