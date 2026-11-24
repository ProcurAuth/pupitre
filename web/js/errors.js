// The text of an error for the person (5.22): the page shows the action of a refusal and never a detail of the PDP.
// The texts are English ASD-STE100. The server gives a fixed message for each code; this module adds what the person can do.

import { clear, el, icon, notify } from "./ui.js";
import { ApiError, isAbort } from "./api.js";

const REASONS = {
  exit_code: "The playbook ended with an error.",
  render_failed: "The render playbook failed. No host got a connection.",
  invalid_values: "A stored value does not pass its check.",
  secret_unreadable: "A secret cannot be read.",
  snapshot_failed: "The snapshot cannot be written.",
  snapshot_changed: "A value changed after the check. Start a new check.",
  check_too_old: "The check is too old. Start a new check.",
  spawn_failed: "The program cannot start.",
  timeout: "The run took too long and the runner stopped it.",
  process_gone: "The runner cannot find the process of the run.",
  node_changed: "Another runner node held the run.",
  lost_exit: "The process ended and left no result.",
  host_failed: "At least one host failed.",
};

export const reasonText = (reason) => REASONS[reason] || (reason ? `Reason: ${reason}.` : "");

// The text of an error: one sentence that says what happened and, when the person can act, what to do.
export function errorText(error) {
  if (!(error instanceof ApiError)) return "The page has a defect. Reload the page.";
  const extra = error.extra || {};
  switch (error.code) {
    case "forbidden":
      return extra.action ? `You do not have the right to do this. The missing action is ${extra.action}.` : "You do not have the right to do this.";
    case "approval_cannot_wait":
      return "This action needs an approval, and the page cannot wait for an approval.";
    case "ceiling_exceeded":
      return `The run has too many hosts. The limit is ${extra.limit === undefined ? "lower" : extra.limit} hosts.`;
    case "apply_disabled":
      return "Apply and commands are off in this mode.";
    case "unauthenticated":
      return "You are not signed in, or your session ended. Reload the page.";
    case "not_found":
      return "The item does not exist.";
    case "network_error":
      return "Pupitre does not answer. Check the connection and try again.";
    case "pdp_unavailable":
      return "The access decision service does not answer. Try again later.";
    case "store_unavailable":
      return "The store does not answer. Try again later.";
    case "store_over_limit":
      return "The store holds more documents than a limit of the configuration. Delete documents, or ask the operator.";
    case "server_busy":
      return "The server is busy. Try again in a moment.";
    case "run_queue_full":
      return "The queue of runs is full. Try again in a moment.";
    case "conflict":
      return "The document changed after you read it.";
    case "already_exists":
      return "An item with this id exists.";
    case "in_use":
      return "Other items use this item. Remove the links first.";
    case "not_empty":
      return "This item holds other items. Remove them first.";
    case "check_required":
      return "The apply needs a check that passed. Start a new check.";
    case "check_used":
      return "An apply already used this check. Start a new check.";
    case "check_too_old":
      return "The check is too old. Start a new check.";
    case "run_finished":
      return "The run is already finished.";
    case "payload_too_large":
      return "The document is too big.";
    case "limit_exceeded":
      return "A limit of the store stops this write.";
    default:
      return error.message || "The request failed.";
  }
}

// A list of the items of `errors` of a 422 or a 409: path and message, with the layer when it exists.
export function errorList(items) {
  const list = el("ul", { class: "error-list" });
  for (const item of items || []) {
    const path = item.path === "" || item.path === undefined ? null : el("code", { text: item.path });
    list.append(el("li", {}, path, path ? " " : null, item.layer ? `(${item.layer}) ` : null, item.message || ""));
  }
  return list;
}

// Shows an error as a notice. An abort shows nothing.
export function notifyError(error, key = null) {
  if (isAbort(error)) return null;
  return notify({ level: "error", key, text: errorText(error) });
}

// A box with the text of an error and its list of items. It replaces the content of `box`.
export function fillErrorBox(box, error) {
  clear(box);
  box.hidden = false;
  box.append(el("p", { class: "errors__title", text: errorText(error) }));
  const items = error && error.extra ? error.extra.errors : null;
  if (Array.isArray(items) && items.length) box.append(errorList(items));
  return box;
}

// A callout (an inline notice). `tone`: info, success, warning or error.
export function callout(tone, text, ...children) {
  const glyph = tone === "error" ? "error" : tone === "warning" ? "warning" : tone === "success" ? "success" : "info";
  return el("div", { class: `callout callout--${tone}`, role: tone === "error" ? "alert" : "status" }, icon(glyph, 16), el("span", { class: "callout__text", text }), ...children);
}
