# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scenarios of the profile local: the backend `files`, one instance that is the runner node (8.12, level Acceptance).

Each scenario makes its own environment, so the scenarios do not depend on each other. The paragraph numbers
are those of the specification. The scenarios use the HTTP API of section 5 and the host containers.
"""
import json
import os
import time

from harness.client import ApiError
from harness.dockerctl import wait_until
from harness.lab import host_address
from harness.smoke import LOOKUP, MARK, SECRET_PASSWORD, SECRET_TOKEN

from .common import (
    FINGERPRINT,
    SNAPSHOT,
    UNKNOWN_RUN,
    assert_contiguous_events,
    config_on,
    event_kinds,
    expect_error,
    hosts_state,
    output_tail,
    process_exists,
)

# The effective values (2.13): the values of the unit and the default of `log.level`. A secret key is not in the variable (4.12).
EXPECTED_HELLO = {"banner": "hello", "limits": {"max_clients": 250}, "log": {"level": "info"}}


def check_then_apply(ctx):
    """4.3, 4.24: a check does not change a host. Its apply does, with the same snapshot hash. One check gives one apply."""
    fleet = ctx.fleet("s1-check-apply").create(banner="hello", max_clients=250)
    client, lab = ctx.client, ctx.lab

    check = fleet.run_and_wait(fleet.check_body())
    assert check["kind"] == "deploy" and check["mode"] == "check" and check["playbook"] == "deploy-demo", check
    assert check["hosts"] == ["host-a", "host-b"] and check["node"] == fleet.node, check
    assert SNAPSHOT.match(check["snapshot"]), "the record of a run keeps the snapshot hash (4.16): %r" % check.get("snapshot")
    assert check.get("started_at") and check.get("finished_at") and check.get("exit_code") == 0, check
    for host in fleet.hosts:
        assert config_on(lab, host) is None, "a check changed %s" % host

    events = client.events(check["id"])
    assert_contiguous_events(events)
    kinds = event_kinds(events)
    assert "stats" in kinds and "host_result" in kinds and kinds[0] == "playbook_start", kinds[:10]
    phases = [event["phase"] for event in events]
    assert phases[0] == "render" and "main" in phases and phases == sorted(phases, key=["render", "main"].index), "4.25: the render phase comes first: %s" % phases[:12]
    diffs = [event["diff"] for event in events if event["kind"] == "host_result" and "diff" in event]
    assert any("hello" in diff for diff in diffs), "the check shows the difference of the configuration (4.20)"
    output = client.output(check["id"]).decode("utf-8", "replace")
    assert "PLAY RECAP" in output, output[-800:]

    apply = fleet.run_and_wait(fleet.apply_body(check["id"]))
    assert apply["check_run"] == check["id"] and apply["mode"] == "apply", apply
    assert apply["snapshot"] == check["snapshot"], "4.16: a check and its apply with the same inputs have the same hash"
    assert client.get_run(check["id"])["applied_by"] == apply["id"], "4.24: the check keeps the id of its apply"
    for host in fleet.hosts:
        assert config_on(lab, host) == EXPECTED_HELLO, "the apply did not write %s" % host
        active = lab.docker.exec(lab.host(host), ["systemctl", "is-active", "demo-service"], check=False).out.strip()
        assert active == "active", "demo-service on %s: %s" % (host, active)

    expect_error(409, "check_used", lambda: client.start_run(fleet.apply_body(check["id"]), expect=(201,)))
    expect_error(404, "not_found", lambda: client.start_run(fleet.apply_body(UNKNOWN_RUN), expect=(201,)))
    status = fleet.run_and_wait(fleet.status_body())
    assert status["kind"] == "status" and status["playbook"] == "status-demo", status
    expect_error(409, "check_required", lambda: client.start_run(fleet.apply_body(status["id"]), expect=(201,)))
    output = client.output(status["id"]).decode("utf-8", "replace")
    for host in fleet.hosts:
        assert "demo-service is active on %s" % host in output, output[-1200:]


def snapshot_changed(ctx):
    """4.24: a value that changes between a check and its apply stops the apply before the child starts."""
    fleet = ctx.fleet("s2-changed").create(banner="hello")
    client, lab = ctx.client, ctx.lab
    check = fleet.run_and_wait(fleet.check_body())
    fleet.set_values(banner="changed-between")
    apply = fleet.run_and_wait(fleet.apply_body(check["id"]), expect_status="failed")
    assert apply["reason"] == "snapshot_changed", apply
    assert "runner" not in apply, "the child must not start after a failed guard (4.24): %s" % apply
    for host in fleet.hosts:
        assert config_on(lab, host) is None, "the apply changed %s" % host
    expect_error(409, "check_used", lambda: client.start_run(fleet.apply_body(check["id"]), expect=(201,)))

    # The same values give the same hash again: a new check and its apply pass (4.16).
    check2 = fleet.run_and_wait(fleet.check_body())
    assert check2["snapshot"] != check["snapshot"], "a new value must change the hash"
    apply2 = fleet.run_and_wait(fleet.apply_body(check2["id"]))
    assert apply2["snapshot"] == check2["snapshot"]
    for host in fleet.hosts:
        assert config_on(lab, host)["banner"] == "changed-between"


def check_too_old(ctx):
    """4.24: an apply with a check older than `runner.check_max_age` (1m in this profile) is refused at the request."""
    fleet = ctx.fleet("s3-too-old").create()
    check = fleet.run_and_wait(fleet.check_body())
    finished = time.time()
    time.sleep(max(0.0, 65 - (time.time() - finished)))
    expect_error(409, "check_too_old", lambda: ctx.client.start_run(fleet.apply_body(check["id"]), expect=(201,)))
    for host in fleet.hosts:
        assert config_on(ctx.lab, host) is None


def render_failure_stops_before_any_connection(ctx):
    """4.25: a render phase that fails gives `render_failed`, and no host gets a connection."""
    fleet = ctx.fleet("s4-render").create(with_banner=False)
    lab = ctx.lab
    before = {host: lab.host_ssh_logins(host) for host in fleet.hosts}
    check = fleet.run_and_wait(fleet.check_body(), expect_status="failed")
    assert check["reason"] == "render_failed", check
    events = ctx.client.events(check["id"])
    assert {event["phase"] for event in events} == {"render"}, "the main phase must not start: %s" % {e["phase"] for e in events}
    after = {host: lab.host_ssh_logins(host) for host in fleet.hosts}
    assert before == after, "the render phase opened an SSH session: %s -> %s" % (before, after)
    fleet.set_values(banner="now-set")
    fleet.run_and_wait(fleet.check_body())


def commands(ctx):
    """4.3, 4.32, 4.33: a command with `check = false`, and one with `check = true`."""
    fleet = ctx.fleet("s5-commands").create(banner="hello")
    client, lab = ctx.client, ctx.lab
    fleet.deploy()

    # check = false: the mode check is refused, the parameters are checked, the apply runs.
    restart = lambda mode, params, **kw: fleet.command_body("restart-service", mode, params, **kw)
    error = expect_error(422, "invalid_run_request", lambda: client.start_run(restart("check", {"grace": "1s"}), expect=(201,)))
    assert error.body.get("errors"), "422 holds one error for each problem (5.4): %s" % error.body
    expect_error(422, "invalid_run_request", lambda: client.start_run(restart("apply", {}), expect=(201,)))
    expect_error(422, "invalid_run_request", lambda: client.start_run(restart("apply", {"grace": "1m30s"}), expect=(201,)))
    expect_error(422, "invalid_run_request", lambda: client.start_run(restart("apply", {"grace": "10m"}), expect=(201,)))
    expect_error(422, "invalid_run_request", lambda: client.start_run(restart("apply", {"grace": "1s", "other": 1}), expect=(201,)))
    started = {host: lab.docker.exec(lab.host(host), ["systemctl", "show", "-p", "ActiveEnterTimestampMonotonic", "demo-service"]).out for host in fleet.hosts}
    done = fleet.run_and_wait(restart("apply", {"grace": "2s"}))
    assert done["command"] == "restart-service" and done["params"] == {"grace": "2s"} and done["mode"] == "apply", done
    for host in fleet.hosts:
        now = lab.docker.exec(lab.host(host), ["systemctl", "show", "-p", "ActiveEnterTimestampMonotonic", "demo-service"]).out
        assert now != started[host], "the command did not restart the service on %s" % host

    # check = true: an apply needs its check. A request without check_run has members that are not valid (4.4).
    rotate = lambda mode, params, **kw: fleet.command_body("rotate-banner", mode, params, **kw)
    expect_error(422, "invalid_run_request", lambda: client.start_run(rotate("apply", {"banner": "rotated-1"}), expect=(201,)))
    check = fleet.run_and_wait(rotate("check", {"banner": "rotated-1"}))
    assert hosts_state(lab, fleet.hosts, "/etc/demo-service/banner") == {"host-a": None, "host-b": None}, "the check wrote the banner"
    diffs = [e["diff"] for e in client.events(check["id"]) if e["kind"] == "host_result" and "diff" in e]
    assert any("rotated-1" in diff for diff in diffs), "the check of a command shows its difference"
    apply = fleet.run_and_wait({"kind": "command", "mode": "apply", "check_run": check["id"]})
    assert apply["command"] == "rotate-banner" and apply["params"] == {"banner": "rotated-1"}, "4.24: the apply takes the parameters of the check: %s" % apply
    assert apply["snapshot"] == check["snapshot"]
    assert hosts_state(lab, fleet.hosts, "/etc/demo-service/banner") == {"host-a": "rotated-1\n", "host-b": "rotated-1\n"}
    expect_error(409, "check_used", lambda: client.start_run({"kind": "command", "mode": "apply", "check_run": check["id"]}, expect=(201,)))


def cancel(ctx):
    """4.27: cancel a running run and a queued run. The child and its process group end."""
    fleet = ctx.fleet("s6-cancel").create()
    client, inst = ctx.client, ctx.inst
    slow = lambda seconds: fleet.command_body("slow-task", "apply", {"seconds": seconds})

    first = client.start_run(slow(120)).json()
    running = client.wait_run(first["id"], until=("running", "passed", "failed", "canceled", "lost"), timeout=60)
    assert running["status"] == "running", running
    runner = wait_until(lambda: client.get_run(first["id"]).get("runner"), 60, what="the runner member of the run record (4.38)")
    pid = runner["pid"]
    assert process_exists(inst.exec, pid), "the child %d does not exist" % pid

    # One run at a time in an environment (4.7): the second run waits, then a person cancels it.
    second = client.start_run(slow(60)).json()
    time.sleep(3)
    queued = client.get_run(second["id"])
    assert queued["status"] == "queued", "4.7: a second run of the environment waits for the lease: %s" % queued
    gone = client.cancel_run(second["id"]).json()
    assert gone["status"] == "canceled" and "started_at" not in gone, "4.27: a queued run is canceled at once: %s" % gone

    canceled = client.cancel_run(first["id"]).json()
    assert canceled["status"] in ("running", "canceled"), canceled
    final = client.wait_run(first["id"], timeout=60)
    assert final["status"] == "canceled" and "reason" not in final and final.get("finished_at"), final
    assert final["cancel"]["at"] and "by" in final["cancel"], final
    wait_until(lambda: not process_exists(inst.exec, pid), 30, what="the end of the child")
    left = inst.exec(["pgrep", "-f", "slow-task.yml"], check=False)
    assert left.code != 0, "a process of the playbook is still alive: %s" % left.out
    expect_error(409, "run_finished", lambda: client.cancel_run(first["id"]))
    assert_contiguous_events(client.events(first["id"]))
    leftover = inst.exec(["sh", "-c", "ls -A /run/pupitre/secrets"]).out.strip()
    assert leftover == "", "4.13: the private folder of a run is removed at its end: %r" % leftover


def adoption_after_restart(ctx):
    """4.29: Pupitre stops during a run, starts again, adopts the child, and the run ends with the status `passed`."""
    fleet = ctx.fleet("s7-adoption").create()
    client, inst = ctx.client, ctx.inst
    run = client.start_run(fleet.command_body("slow-task", "apply", {"seconds": 30})).json()
    runner = wait_until(lambda: client.get_run(run["id"]).get("runner"), 60, what="the runner member of the run record")
    pid, start_time, boot_id = runner["pid"], runner["start_time"], runner["boot_id"]
    wait_until(lambda: any(e["kind"] == "task_start" for e in client.events(run["id"])), 60, what="the first task of the playbook")

    inst.kill("KILL")  # no clean stop: the worst case for the runner
    assert process_exists(inst.exec, pid), "4.19: the child must live after the end of Pupitre"
    inst.clear_log()
    inst.start()
    inst.wait_ready(90)
    client = inst.client()
    assert "run.adopted" in inst.log_text() or wait_until(lambda: "run.adopted" in inst.log_text(), 20, what="the log line run.adopted (6.27)")
    record = client.get_run(run["id"])
    assert record["status"] in ("running", "passed"), "an adopted run keeps its status: %s" % record

    final = client.wait_run(run["id"], timeout=120)
    assert final["status"] == "passed" and "reason" not in final, "4.23: an adopted child that ends with a stats event passes: %s\n%s" % (final, output_tail(client, run["id"]))
    assert final["runner"]["pid"] == pid and final["runner"]["start_time"] == start_time and final["runner"]["boot_id"] == boot_id, final["runner"]
    events = client.events(run["id"])
    assert_contiguous_events(events)
    assert event_kinds(events).count("stats") == 1 and event_kinds(events)[0] == "playbook_start"
    assert "PLAY RECAP" in client.output(run["id"]).decode("utf-8", "replace")

    # The child of another start time is not the same process: a lost run has `process_gone` (4.29). Not tested here:
    # the harness cannot change the start time of a process.


def secrets_never_show(ctx):
    """2.24 to 2.26, 4.13, 4.21, 7.9: a secret reaches the host, and never shows in the stored output, events, log or store."""
    fleet = ctx.fleet("s8-secrets").create()
    client, lab, inst = ctx.client, ctx.lab, ctx.inst

    answer = fleet.set_secret("storage.password", SECRET_PASSWORD)
    assert FINGERPRINT.match(answer["fingerprint"]) and list(answer) == ["fingerprint"], answer
    fleet.set_secret("api.token", SECRET_TOKEN)
    listing = client.call("GET", fleet.unit_path + "/secrets").json()
    assert [item["name"] for item in listing["items"]] == ["api.token", "storage.password"], listing
    assert SECRET_PASSWORD not in json.dumps(listing) and SECRET_TOKEN not in json.dumps(listing)
    effective = client.call("GET", fleet.unit_path + "/effective").json()
    shown = json.dumps(effective)
    assert SECRET_PASSWORD not in shown and SECRET_TOKEN not in shown, "5.12: the effective view holds no secret value"
    # A value that is too short is refused, and the message never holds the value (2.25, 5.14).
    short = expect_error(422, "invalid_secret", lambda: client.put_secret(fleet.unit_path, "storage.password", "Zq7!"))
    assert "Zq7!" not in short.body.get("message", ""), short.body

    deploy = fleet.deploy()
    for host in fleet.hosts:
        assert lab.host_file(host, "/etc/demo-service/password") == SECRET_PASSWORD, "the secret did not reach %s" % host
    out = client.output(deploy["id"]).decode("utf-8", "replace")
    assert SECRET_PASSWORD not in out, "the output of the deploy holds the password (no_log)"

    printed = fleet.run_and_wait(fleet.command_body("leak-secret", "apply", {"mode": "print"}))
    failed = fleet.run_and_wait(fleet.command_body("leak-secret", "apply", {"mode": "fail"}), expect_status="failed")
    assert failed["reason"] == "exit_code", failed

    token_json = json.dumps(SECRET_TOKEN)[1:-1]  # the JSON string form of the value (4.21)
    secrets_ = (SECRET_PASSWORD, SECRET_TOKEN, token_json)
    for run in (printed, failed):
        output = client.output(run["id"]).decode("utf-8", "replace")
        events = json.dumps(client.events(run["id"]), ensure_ascii=False)
        record = json.dumps(client.get_run(run["id"]), ensure_ascii=False)
        # 4.21: the scrubber finds the exact bytes of a value. The JSON string form is replaced in event lines only.
        # Ansible prints the JSON string form in the output of a failed task, so the output check uses the raw values.
        for secret in (SECRET_PASSWORD, SECRET_TOKEN):
            assert secret not in output, "4.21: the stored output of %s holds a secret" % run["id"]
        for secret in secrets_:
            assert secret not in events, "4.21: the stored events of %s hold a secret" % run["id"]
            assert secret not in record, "the run record holds a secret"
        assert "[secret]" in output, "4.21: the scrubber puts [secret] in the output\n%s" % output[-1500:]
    failed_events = [e for e in client.events(failed["id"]) if e["kind"] == "host_result" and e["status"] == "failed"]
    assert failed_events and all("[secret]" in e.get("message", "") for e in failed_events), "the failed message is scrubbed: %s" % failed_events

    log = inst.log_text()
    for secret in secrets_:
        assert secret not in log, "7.9: the log of the server holds a secret"
    # The store holds the documents and the runs. The secret folder of the backend `files` holds the values, by design (7.11).
    for folder in ("data", "runs"):
        hits = _grep(inst.folder, folder, (SECRET_PASSWORD, SECRET_TOKEN))
        assert not hits, "7.9: the store folder %s holds a secret in %s" % (folder, hits)
    manifest = [name for name in _walk(inst.folder, "runner") if name.endswith("snapshot/manifest.json")]
    for path in manifest:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        assert not any(secret in text for secret in (SECRET_PASSWORD, SECRET_TOKEN)), "4.16: a manifest holds a secret value: %s" % path
    private = inst.exec(["sh", "-c", "ls -A /run/pupitre/secrets"]).out.strip()
    assert private == "", "4.13: the private folder of a run is removed at its end: %r" % private


def host_key_of_the_host_id(ctx):
    """4.12, 7.8: SSH finds the host key by `<environment>.<host>`. A host that gets the address of another host fails
    at the check of the host key, so a person who edits one environment cannot reach a machine of another one."""
    fleet = ctx.fleet("s9-host-key", hosts=("host-a",)).create()
    client = ctx.client
    path = fleet.base + "/hosts/host-a"
    document, etag = client.get_document(path)
    document["address"] = host_address("host-b")
    client.save_document(path, document, etag, "acceptance")
    status = fleet.run_and_wait(fleet.status_body(), expect_status="failed")
    assert status["reason"] == "exit_code", status
    output = client.output(status["id"]).decode("utf-8", "replace")
    assert "Host key verification failed" in output or "host key" in output.lower(), output[-1500:]
    assert "demo-service is" not in output, "the status playbook ran on the machine of host-b"
    if status.get("stats"):
        assert status["stats"]["host-a"]["unreachable"] > 0, status["stats"]


def template_markers(ctx):
    """2.12, 4.33, 4.9, 7.3: a template marker never reaches Ansible. The server refuses it in a value and in a text
    parameter, and the runner refuses a value that a person wrote in the store files, before a child starts."""
    fleet = ctx.fleet("s10-markers").create(banner="hello")
    client, inst = ctx.client, ctx.inst
    inst.exec(["rm", "-f", MARK])

    document, etag = client.get_document(fleet.unit_path + "/values")
    document["values"]["banner"] = LOOKUP
    error = expect_error(422, "invalid_document", lambda: client.save_document(fleet.unit_path + "/values", document, etag, "acceptance"))
    shown = json.dumps(error.body)
    assert "banner" in shown and "no template marker" in shown, shown
    error = expect_error(422, "invalid_run_request", lambda: client.start_run(fleet.command_body("rotate-banner", "check", {"banner": LOOKUP}), expect=(201,)))
    shown = json.dumps(error.body)
    assert "params.banner" in shown and "no template marker" in shown, shown

    # The backend files: the value goes straight into the store file, in the canonical form of 2.4.
    path = os.path.join(inst.folder, "data", "environments", fleet.env, "units", fleet.unit, "values.json")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(json.dumps({"format": 1, "values": {"banner": LOOKUP, "limits.max_clients": 250}}, indent=2, sort_keys=True) + "\n")
    time.sleep(3)  # rescan_interval is 1s (2.33)
    try:
        record = client.start_run(fleet.check_body()).json()
    except ApiError as refused:
        ctx.detail = "the server refused the run: %d %s" % (refused.status, refused.code)
    else:
        final = client.wait_run(record["id"], timeout=120)
        assert final["status"] == "failed" and final["reason"] == "invalid_values", final
        assert "runner" not in final, "a child started: %s" % final
        output = client.output(record["id"]).decode("utf-8", "replace")
        assert "banner" in output and "no template marker" in output, output[-1500:]
        ctx.detail = "the runner refused the run with invalid_values"
    assert inst.exec(["test", "-e", MARK], check=False).code != 0, "a lookup of a stored value ran on the runner"
    for host in fleet.hosts:
        assert config_on(ctx.lab, host) is None, "a run changed %s" % host


def _walk(base: str, folder: str):
    import os

    for root, _dirs, files in os.walk(os.path.join(base, folder)):
        for name in files:
            yield os.path.join(root, name)


def _grep(base: str, folder: str, needles) -> list:
    hits = []
    for path in _walk(base, folder):
        try:
            with open(path, "rb") as handle:
                data = handle.read().decode("utf-8", "replace")
        except OSError:
            continue
        if any(needle in data for needle in needles):
            hits.append(path)
    return hits


SCENARIOS = [
    ("check_then_apply", check_then_apply),
    ("snapshot_changed", snapshot_changed),
    ("render_failure", render_failure_stops_before_any_connection),
    ("commands", commands),
    ("cancel", cancel),
    ("adoption_after_restart", adoption_after_restart),
    ("secrets_never_show", secrets_never_show),
    ("check_too_old", check_too_old),
    ("host_key_of_the_host_id", host_key_of_the_host_id),
    ("template_markers", template_markers),
]
