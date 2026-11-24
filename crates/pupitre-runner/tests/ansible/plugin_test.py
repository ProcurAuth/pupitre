# SPDX-License-Identifier: MIT OR Apache-2.0
"""Run the callback plugin pupitre_events with a real ansible-playbook (4.20).

Use: python3 -I plugin_test.py <plugin file> <output folder>

The script runs ansible-playbook on localhost (connection local) and on one host that cannot be
reached. It checks each event that the plugin wrote against the closed members of 4.20, and the
behavior of the plugin: no_log, a text above 16,384 bytes, the mode check, a failed task and the
file that two phases share. It writes the events files in the output folder.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

PLUGIN, OUT = sys.argv[1], sys.argv[2]

PLAYBOOK = """\
---
- name: Local tasks
  hosts: local
  gather_facts: false
  tasks:
    - name: Write a file
      ansible.builtin.copy:
        content: "hello from the test\\n"
        dest: "{work}/written.txt"
        mode: "0644"

    - name: Skipped task
      ansible.builtin.debug:
        msg: never
      when: false

    - name: Secret task
      ansible.builtin.copy:
        content: "the secret value of the test\\n"
        dest: "{work}/secret.txt"
        mode: "0600"
      no_log: true

    - name: Task that fails and is ignored
      ansible.builtin.fail:
        msg: expected failure
      ignore_errors: true

    - name: Unicode text
      ansible.builtin.debug:
        msg: "é — 日本語"

    - name: Large difference
      ansible.builtin.copy:
        content: "{big}"
        dest: "{work}/big.txt"
        mode: "0644"

    - name: Large message
      ansible.builtin.fail:
        msg: "{big}"
      ignore_errors: true

- name: Remote tasks
  hosts: remote
  gather_facts: false
  tasks:
    - name: Ping a host that is not reachable
      ansible.builtin.ping:
"""

FAILING = """\
---
- name: Failing play
  hosts: local
  gather_facts: false
  tasks:
    - name: Task that fails
      ansible.builtin.fail:
        msg: the task failed
"""

INVENTORY = """\
all:
  hosts:
    local:
      ansible_connection: local
      ansible_python_interpreter: "{python}"
    remote:
      ansible_connection: ssh
      ansible_host: 203.0.113.1
      ansible_user: nobody
      ansible_ssh_common_args: "-o ConnectTimeout=2 -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null"
"""

STATS_KEYS = {"changed", "failed", "ignored", "ok", "rescued", "skipped", "unreachable"}
KINDS = {
    "playbook_start": {"playbook", "check"},
    "play_start": {"play"},
    "task_start": {"task"},
    "host_result": {"host", "task", "status", "diff", "message", "omitted"},
    "stats": {"hosts"},
}
MAX_TEXT = 16384


def fail(text):
    print("FAILED: " + text, file=sys.stderr)
    sys.exit(1)


def check(condition, text):
    if not condition:
        fail(text)


def reject_constant(name):
    raise ValueError("a constant that 2.5 refuses: " + name)


def strict_events(path):
    """Read the events file with the rules of 4.20: closed members, `v` equal to 1."""
    events = []
    with open(path, "rb") as handle:
        data = handle.read()
    check(data.endswith(b"\n") or not data, "the file ends in the middle of a line")
    for number, raw in enumerate(data.split(b"\n")[:-1], 1):
        text = raw.decode("utf-8")
        event = json.loads(text, parse_constant=reject_constant)
        check(isinstance(event, dict), "line %d is not an object" % number)
        check(event.get("v") == 1, "line %d has no v equal to 1" % number)
        kind = event.get("kind")
        check(kind in KINDS, "line %d has the kind %r" % (number, kind))
        members = set(event) - {"v", "kind"}
        check(members <= KINDS[kind], "line %d has members %r" % (number, members - KINDS[kind]))
        for name in ("diff", "message"):
            if name in event:
                check(isinstance(event[name], str), "%s is not a text" % name)
                check(len(event[name].encode("utf-8")) <= MAX_TEXT, "%s is above 16,384 bytes" % name)
        if event.get("omitted") is not None:
            check(event["omitted"] is True, "omitted is not true")
        if kind == "stats":
            for host, counts in event["hosts"].items():
                check(set(counts) == STATS_KEYS, "the counts of %s" % host)
                check(all(isinstance(v, int) and v >= 0 for v in counts.values()), "the counts of %s" % host)
        events.append(event)
    return events


def run(work, playbook, extra, events_file, plugin_dir, env_extra=None):
    cfg = os.path.join(work, "ansible.cfg")
    with open(cfg, "w") as handle:
        handle.write(
            "[defaults]\ninventory = %s/inventory.yml\ncallback_plugins = %s\n"
            "callbacks_enabled = pupitre_events\nretry_files_enabled = False\n"
            "host_key_checking = False\nlocal_tmp = %s/tmp\n" % (work, plugin_dir, work)
        )
    env = {
        "PATH": os.environ["PATH"],
        "HOME": work,
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "ANSIBLE_CONFIG": cfg,
        "ANSIBLE_LOCAL_TEMP": os.path.join(work, "tmp"),
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    if events_file is not None:
        env["PUPITRE_EVENTS_FILE"] = events_file
        env["PUPITRE_EVENTS_PHASE"] = "main"
    env.update(env_extra or {})
    done = subprocess.run(
        ["ansible-playbook", playbook] + extra, cwd=work, env=env, capture_output=True, text=True, check=False
    )
    return done


def by(events, kind):
    return [e for e in events if e["kind"] == kind]


def result(events, task, host="local"):
    found = [e for e in events if e["kind"] == "host_result" and e["task"] == task and e["host"] == host]
    check(len(found) == 1, "one host_result for %r on %s, found %d" % (task, host, len(found)))
    return found[0]


def fallback_keeps_the_characters():
    import importlib.util

    spec = importlib.util.spec_from_file_location("pupitre_events_under_test", PLUGIN)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    plugin = module.CallbackModule.__new__(module.CallbackModule)

    def broken(_diff):
        raise ValueError("no text form")

    plugin._get_diff = broken

    class Result:
        _result = {"diff": {"after": "mot de passe é-ü-日本-\U0001F600"}}

    text = plugin._diff_text(Result())
    check("mot de passe é-ü-日本-\U0001F600" in text, "the fallback form changed the characters: %r" % text)
    check("\\u" not in text, "the fallback form has escapes: %r" % text)


def message_of_a_collection_is_json():
    """A message that is a dictionary has the JSON form, not the repr of Python (4.21)."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("pupitre_events_under_test", PLUGIN)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    plugin = module.CallbackModule.__new__(module.CallbackModule)
    written = []
    plugin._write = written.append
    plugin._get_diff = lambda diff: None

    class Host:
        @staticmethod
        def get_name():
            return "h"

    class Task:
        no_log = False

        @staticmethod
        def get_name():
            return "t"

    class Result:
        _host = Host()
        _task = Task()
        _result = {"msg": {"after": 'pa"ss\\word\'x1'}}

    plugin._host_result(Result(), "failed", True)
    message = written[0]["message"]
    check(json.loads(message) == {"after": 'pa"ss\\word\'x1'}, "the message of a dictionary is not JSON text: %r" % message)


def main():
    work = tempfile.mkdtemp(prefix="pupitre-plugin-")
    plugin_dir = os.path.join(work, "callback_plugins")
    os.makedirs(plugin_dir)
    os.makedirs(os.path.join(work, "tmp"))
    shutil.copy(PLUGIN, os.path.join(plugin_dir, "pupitre_events.py"))
    with open(os.path.join(work, "inventory.yml"), "w") as handle:
        handle.write(INVENTORY.format(python=sys.executable))
    big = "x" * (MAX_TEXT + 1000)
    with open(os.path.join(work, "site.yml"), "w") as handle:
        handle.write(PLAYBOOK.format(work=work, big=big))
    with open(os.path.join(work, "failing.yml"), "w") as handle:
        handle.write(FAILING)

    # 1. The mode check with the differences: nothing changes on the machine.
    events_check = os.path.join(OUT, "events-check.jsonl")
    done = run(work, "site.yml", ["--check", "--diff"], events_check, plugin_dir)
    check(done.returncode == 4, "check run: exit code %d, expected 4 (an unreachable host)\n%s" % (done.returncode, done.stdout + done.stderr))
    check(not os.path.exists(os.path.join(work, "written.txt")), "the check wrote a file")
    ev = strict_events(events_check)
    start = by(ev, "playbook_start")
    check(len(start) == 1 and start[0] == {"kind": "playbook_start", "playbook": "site.yml", "check": True, "v": 1}, "playbook_start of the check: %r" % start)
    check([e["play"] for e in by(ev, "play_start")] == ["Local tasks", "Remote tasks"], "the plays")
    tasks = [e["task"] for e in by(ev, "task_start")]
    check(tasks[:3] == ["Write a file", "Skipped task", "Secret task"], "the tasks: %r" % tasks)
    r = result(ev, "Write a file")
    check(r["status"] == "changed" and "hello from the test" in r.get("diff", ""), "the difference of the check: %r" % r)
    check(result(ev, "Skipped task")["status"] == "skipped", "a skipped task")
    secret = result(ev, "Secret task")
    check("diff" not in secret and "message" not in secret, "no_log gives no diff and no message: %r" % secret)
    check("the secret value" not in json.dumps(ev), "the secret text is in the events")
    failed = result(ev, "Task that fails and is ignored")
    check(failed["status"] == "failed" and failed["message"] == "expected failure", "an ignored failure: %r" % failed)
    check(result(ev, "Unicode text")["status"] == "ok", "the unicode task")
    large = result(ev, "Large difference")
    check(large.get("omitted") is True and "diff" not in large, "a diff above 16,384 bytes is omitted: %r" % list(large))
    long_msg = result(ev, "Large message")
    check(long_msg.get("omitted") is True and "message" not in long_msg, "a message above 16,384 bytes is omitted")
    unreachable = result(ev, "Ping a host that is not reachable", "remote")
    check(unreachable["status"] == "unreachable" and unreachable.get("message"), "unreachable: %r" % unreachable)
    stats = by(ev, "stats")
    check(len(stats) == 1 and ev[-1]["kind"] == "stats", "one stats event at the end")
    check(stats[0]["hosts"]["local"]["ignored"] >= 2, "the ignored failures are counted")
    check(stats[0]["hosts"]["remote"]["unreachable"] == 1, "the unreachable host is counted")

    # 2. The mode apply: the file is written, the playbook_start says check false.
    events_apply = os.path.join(OUT, "events-apply.jsonl")
    done = run(work, "site.yml", [], events_apply, plugin_dir)
    check(done.returncode == 4, "apply run: exit code %d\n%s" % (done.returncode, done.stdout + done.stderr))
    check(os.path.exists(os.path.join(work, "written.txt")), "the apply wrote no file")
    ev = strict_events(events_apply)
    check(by(ev, "playbook_start")[0]["check"] is False, "playbook_start of the apply")
    check("diff" not in result(ev, "Write a file"), "no difference without --diff")

    # 3. A failed task: the exit code 2, the status failed, the counts.
    events_failed = os.path.join(OUT, "events-failed.jsonl")
    done = run(work, "failing.yml", [], events_failed, plugin_dir)
    check(done.returncode == 2, "failing run: exit code %d" % done.returncode)
    ev = strict_events(events_failed)
    r = result(ev, "Task that fails")
    check(r["status"] == "failed" and r["message"] == "the task failed", "the failed result: %r" % r)
    check(by(ev, "stats")[0]["hosts"]["local"]["failed"] == 1, "the failed count")

    # 4. Two phases write one file: the plugin appends (4.19, 4.25).
    events_two = os.path.join(OUT, "events-two-phases.jsonl")
    for _ in range(2):
        run(work, "failing.yml", [], events_two, plugin_dir)
    ev = strict_events(events_two)
    check(len(by(ev, "playbook_start")) == 2 and len(by(ev, "stats")) == 2, "the file of two phases")

    # 5. No PUPITRE_EVENTS_FILE: the plugin is off and the run is normal.
    done = run(work, "failing.yml", [], None, plugin_dir)
    check(done.returncode == 2 and "Traceback" not in done.stderr, "no events file: %s" % done.stderr)

    # 6. The text of a difference that the callback base cannot write: the JSON form keeps each
    #    character, so that the scrubber finds a value that is not ASCII (4.21).
    fallback_keeps_the_characters()
    message_of_a_collection_is_json()

    shutil.rmtree(work, ignore_errors=True)
    print("plugin test: ok (%s)" % subprocess.run(["ansible-playbook", "--version"], capture_output=True, text=True, check=False).stdout.splitlines()[0])


main()
