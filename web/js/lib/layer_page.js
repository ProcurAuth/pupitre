// The save of the values of one layer (5.5, 5.22): the Save button of the header, the message of the version, the check of the message,
// the errors of a 422, the conflict of a 409. The page gives a `reload` function that reads the stored document, its etag and the
// effective values. The controller holds the etag of the document that the person reads.

import * as api from "../api.js";
import { errorText, errorList, notifyError } from "../errors.js";
import { canonical } from "../canonical.js";
import { byteLength, checkMessage } from "../validate.js";
import { el } from "../ui.js";
import { showConflict } from "./conflict.js";
import { textInput } from "./dom.js";

// opts: { ctx, path, editor, initial: { doc, etag }, reload(): { doc, etag, effective }, saveButton, conflictBox, errorsBox, what }
export function createLayerController(opts) {
  const { ctx, path, editor, saveButton, conflictBox, errorsBox, reload, what = "values" } = opts;
  let etag = opts.initial.etag;
  let exists = !!opts.initial.doc;
  let busy = false;
  const status = el("span", { class: "note-line", role: "status", dataset: { testid: "save-status" } });
  const message = textInput({ class: "input input--message", maxlength: 500, placeholder: "Message for the history (optional)", "aria-label": "Message for the history", dataset: { testid: "message" } });

  function sync() {
    const dirty = editor.isDirty();
    saveButton.disabled = !dirty || busy;
    if (dirty) status.textContent = pluralChanges(editor.changedCount());
    else if (/not saved$/.test(status.textContent)) status.textContent = "";
  }

  function pluralChanges(n) {
    return n === 1 ? "1 change not saved" : `${n} changes not saved`;
  }

  async function refresh() {
    const fresh = await reload();
    etag = fresh.etag;
    exists = !!fresh.doc;
    editor.reset(fresh.doc, fresh.effective);
    return fresh;
  }

  async function save() {
    const problem = checkMessage(message.value);
    if (problem) {
      status.textContent = problem;
      return;
    }
    // The server bounds the canonical bytes of a document (5.7, 2.42). The page says so before the write.
    const bytes = byteLength(canonical(editor.getDoc()));
    const bound = ctx.session.limits.max_document_bytes;
    if (bytes > bound) {
      status.textContent = `The document has ${bytes} bytes. The limit is ${bound} bytes. Remove a value or a note.`;
      return;
    }
    busy = true;
    errorsBox.hidden = true;
    conflictBox.replaceChildren();
    sync();
    try {
      const doc = editor.getDoc();
      const result = await api.putDoc(path, doc, { etag: exists ? etag : null, message: message.value });
      exists = true;
      etag = result.etag;
      await refresh();
      message.value = "";
      status.textContent = `Saved. The version is ${result.version}.`;
    } catch (error) {
      await onError(error);
    } finally {
      busy = false;
      sync();
    }
  }

  async function onError(error) {
    if (api.isAbort(error)) return;
    if (error.code === "conflict" || error.code === "already_exists" || (error.code === "not_found" && exists)) {
      status.textContent = "";
      try {
        await showConflict({
          container: conflictBox,
          path,
          mineDoc: editor.getDoc(),
          what,
          onKeep: async (currentEtag, currentDoc) => {
            etag = currentEtag;
            exists = currentEtag !== null;
            try {
              const fresh = await reload();
              editor.rebase(currentDoc, fresh.effective);
            } catch (reloadError) {
              notifyError(reloadError);
            }
            sync();
            status.textContent = "Your edits stay. Read them again, then save.";
          },
          onUseCurrent: async () => {
            try {
              await refresh();
              status.textContent = "";
            } catch (reloadError) {
              notifyError(reloadError);
            }
            sync();
          },
        });
      } catch (inner) {
        notifyError(inner);
      }
      return;
    }
    if (error.code === "invalid_document" || error.code === "invalid_secret") {
      const rest = editor.markErrors((error.extra && error.extra.errors) || []);
      errorsBox.replaceChildren(el("p", { class: "errors__title", text: errorText(error) }), errorList(rest.length ? rest : (error.extra && error.extra.errors) || []));
      errorsBox.hidden = false;
      return;
    }
    notifyError(error);
  }

  saveButton.addEventListener("click", save);
  ctx.setGuard(() => editor.isDirty());
  sync();
  return { message, status, sync, refresh, state: () => ({ etag, exists }), setBase(nextEtag, nextExists) { etag = nextEtag; exists = nextExists; } };
}
