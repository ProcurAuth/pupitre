# SPDX-License-Identifier: MIT OR Apache-2.0
"""Callback plugin pupitre_events (4.20 of the Pupitre specification).

The plugin writes one JSON object on each line of the file that PUPITRE_EVENTS_FILE names.
It flushes each line. It never cuts a text: a text above 16,384 bytes is absent and the
line has "omitted" equal to true. A task with no_log gives no diff and no message.
It works with ansible-core 2.15 and later.
"""

from __future__ import annotations

import json
import os

from ansible import context
from ansible.plugins.callback import CallbackBase

DOCUMENTATION = """
    name: pupitre_events
    type: notification
    short_description: Write the events of a run for Pupitre
    description:
      - Write one JSON object on each line of the file named by PUPITRE_EVENTS_FILE.
    requirements:
      - Enable it with callbacks_enabled in ansible.cfg.
"""

EVENT_VERSION = 1
MAX_TEXT_BYTES = 16384


def _clean(text):
    """Return text that is valid UTF-8, with no lone surrogate."""
    return str(text).encode("utf-8", "replace").decode("utf-8")


def _message_text(message):
    """Return a message that is a text. A list or a dictionary becomes JSON text, as a diff does.

    The text of str() has the Python escapes. The JSON form has the escapes that the scrubber of
    Pupitre knows, with a keep of each character (4.21).
    """
    if message is None or isinstance(message, str):
        return message
    try:
        return json.dumps(message, default=str, sort_keys=True, ensure_ascii=False)
    except (TypeError, ValueError):
        return str(message)


class CallbackModule(CallbackBase):
    """Write the events of 4.20."""

    CALLBACK_VERSION = 2.0
    CALLBACK_TYPE = "notification"
    CALLBACK_NAME = "pupitre_events"
    CALLBACK_NEEDS_ENABLED = True

    def __init__(self, display=None, options=None):
        super().__init__(display=display, options=options)
        self._handle = None
        self._failed_once = False
        path = os.environ.get("PUPITRE_EVENTS_FILE")
        if not path:
            self.disabled = True
            return
        # O_APPEND: the render phase and the main phase write the same file.
        self._handle = open(path, "a", encoding="utf-8", newline="\n")

    def _write(self, event):
        if self._handle is None:
            return
        event["v"] = EVENT_VERSION
        try:
            line = json.dumps(event, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
            self._handle.write(line + "\n")
            self._handle.flush()
        except (OSError, ValueError) as error:
            if not self._failed_once:
                self._failed_once = True
                self._display.warning("pupitre_events: cannot write an event: %s" % error)

    def _texts(self, event, diff, message):
        """Add diff and message to the event, or set omitted."""
        omitted = False
        for name, text in (("diff", diff), ("message", message)):
            if text is None or text == "":
                continue
            text = _clean(text)
            if len(text.encode("utf-8")) > MAX_TEXT_BYTES:
                omitted = True
            else:
                event[name] = text
        if omitted:
            event["omitted"] = True

    def _diff_text(self, result):
        diff = result._result.get("diff")
        if not diff:
            return None
        try:
            text = self._get_diff(diff)
        except Exception:  # pylint: disable=broad-except
            # ensure_ascii=False: a non-ASCII secret keeps its bytes, so the scrubber finds it (4.21).
            text = json.dumps(diff, default=str, sort_keys=True, ensure_ascii=False)
        return text or None

    def _no_log(self, result):
        return bool(result._result.get("_ansible_no_log")) or bool(getattr(result._task, "no_log", False))

    def _host_result(self, result, status, with_message):
        event = {"kind": "host_result", "host": _clean(result._host.get_name()), "task": _clean(result._task.get_name()), "status": status}
        if not self._no_log(result):
            message = None
            if with_message:
                message = result._result.get("msg")
                if message is None and result._result.get("exception"):
                    message = "the task raised an exception"
                message = _message_text(message)
            self._texts(event, self._diff_text(result), message)
        self._write(event)

    def v2_playbook_on_start(self, playbook):
        name = os.path.basename(getattr(playbook, "_file_name", "") or "")
        check = bool(context.CLIARGS.get("check", False))
        self._write({"kind": "playbook_start", "playbook": _clean(name), "check": check})

    def v2_playbook_on_play_start(self, play):
        self._write({"kind": "play_start", "play": _clean(play.get_name().strip())})

    def v2_playbook_on_task_start(self, task, is_conditional):
        self._write({"kind": "task_start", "task": _clean(task.get_name().strip())})

    def v2_playbook_on_handler_task_start(self, task):
        self._write({"kind": "task_start", "task": _clean(task.get_name().strip())})

    def v2_runner_on_ok(self, result):
        status = "changed" if result._result.get("changed", False) else "ok"
        self._host_result(result, status, False)

    def v2_runner_on_failed(self, result, ignore_errors=False):
        self._host_result(result, "failed", True)

    def v2_runner_on_skipped(self, result):
        self._host_result(result, "skipped", False)

    def v2_runner_on_unreachable(self, result):
        self._host_result(result, "unreachable", True)

    def v2_playbook_on_stats(self, stats):
        hosts = {}
        for host in sorted(stats.processed.keys()):
            summary = stats.summarize(host)
            hosts[_clean(host)] = {
                "changed": int(summary.get("changed", 0)),
                "failed": int(summary.get("failures", 0)),
                "ignored": int(summary.get("ignored", 0)),
                "ok": int(summary.get("ok", 0)),
                "rescued": int(summary.get("rescued", 0)),
                "skipped": int(summary.get("skipped", 0)),
                "unreachable": int(summary.get("unreachable", 0)),
            }
        self._write({"kind": "stats", "hosts": hosts})
