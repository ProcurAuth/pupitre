// The conflict of a save (5.22): a `409 conflict` shows the two versions and asks the person to merge by hand.
// The panel shows the difference between the stored version (left) and the version of the person (right). The person chooses:
//   "Keep my edits on the new version": the edits stay, the base moves to the new etag, the person saves again after a read of the difference.
//   "Use the current version": the page drops the edits and reads the stored version.
// A document that was deleted in the meantime gives a panel with one choice: the edits stay, and the next save creates the document again.

import { canonical } from "../canonical.js";
import * as api from "../api.js";
import { renderDiff } from "../diff_view.js";
import { el, icon } from "../ui.js";

// `container` is the node that receives the panel. `mineDoc` is the document of the person. `path` is the route of the document.
// The callbacks: onKeep(currentEtag, currentDocument) (both null for a deleted document) and onUseCurrent(). It returns { close() }.
export async function showConflict({ container, path, mineDoc, onKeep, onUseCurrent, what = "document" }) {
  container.replaceChildren();
  let current = null;
  let gone = false;
  try {
    current = await api.getText(path);
  } catch (error) {
    if (error && error.status === 404) gone = true;
    else throw error;
  }
  const mine = canonical(mineDoc);
  const keep = el("button", { type: "button", class: "btn btn--primary", text: gone ? "Keep my edits and create the document again" : "Keep my edits on the new version", dataset: { testid: "conflict-keep" } });
  const use = el("button", { type: "button", class: "btn", text: "Use the current version", hidden: gone, dataset: { testid: "conflict-use" } });
  const lead = gone
    ? `The ${what} was deleted after you read it. Your edits are not saved.`
    : `The ${what} changed after you read it. Your edits are not saved. Read the difference, then choose.`;
  const panel = el("div", { class: "callout callout--error callout--column conflict", role: "alert", dataset: { testid: "conflict" } },
    el("div", { class: "row" }, icon("error", 16), el("span", { class: "callout__text", text: lead })),
    gone ? null : renderDiff({ text: current.text, name: "current.json" }, { text: mine, name: "mine.json" }, { title: "Current version (left) and your version (right)" }).el,
    el("div", { class: "confirm__actions" }, keep, use));
  keep.addEventListener("click", () => {
    panel.remove();
    onKeep(gone ? null : current.etag, gone ? null : JSON.parse(current.text));
  });
  use.addEventListener("click", () => {
    panel.remove();
    onUseCurrent();
  });
  container.append(panel);
  keep.focus({ preventScroll: false });
  panel.scrollIntoView({ block: "nearest" });
  return { close: () => panel.remove() };
}
