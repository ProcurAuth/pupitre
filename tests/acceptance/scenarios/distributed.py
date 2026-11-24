# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scenarios of two instances on one PostgreSQL (8.12, level Two instances).

The instance A is the runner node (`runner.execute = true`). The instance B serves the API only. Both use the
same database with TLS. The account of `serve` has the rights of 2.36 only.
"""
import os
import re
import time

from harness.dockerctl import wait_until

from .common import assert_contiguous_events, config_on, process_exists


def _node_id(instance) -> str:
    with open(os.path.join(instance.folder, "runner", "node-id"), encoding="utf-8") as handle:
        return handle.read().strip()


def request_on_the_api_node(ctx):
    """4.1: a run request on the instance that is not the runner node is stored `queued` and runs on the runner node."""
    fleet = ctx.fleet("t1-api-node", client=ctx.b.client())
    fleet.create(banner="two-instances")
    # The catalog of A reads the store at its poll (2.41): wait until A sees the unit.
    wait_until(lambda: ctx.a.client().request("GET", fleet.unit_path).status == 200, 30, what="the catalog of A")

    check = fleet.run_and_wait(fleet.check_body())
    assert check["runner"]["node"] == _node_id(ctx.a), "the run must run on the runner node: %s" % check.get("runner")
    assert not os.path.exists(os.path.join(ctx.b.folder, "runner", "node-id")), "4.8: only the runner node has a node id"
    apply = fleet.run_and_wait(fleet.apply_body(check["id"]))
    assert apply["snapshot"] == check["snapshot"]
    for host in fleet.hosts:
        assert config_on(ctx.lab, host)["banner"] == "two-instances"
    # The record is the same on both instances (2.17: one store).
    assert ctx.a.client().get_run(apply["id"]) == ctx.b.client().get_run(apply["id"])
    # The API instance starts no child (4.1).
    assert ctx.b.exec(["pgrep", "-f", "ansible-playbook"], check=False).code != 0, "the instance B started ansible-playbook"
    assert not re.search(r'"event"\s*:\s*"run\.started"', ctx.b.log_text()), "the instance B logged run.started"
    assert re.search(r'"event"\s*:\s*"run\.started"', ctx.a.log_text()), "the runner node logged no run.started (6.27)"
    # 5.20: the instance B has no check `runner_lease`. Both instances are ready.
    for inst in (ctx.a, ctx.b):
        inst.wait_ready(10)


def runner_node_restart_during_run(ctx):
    """4.8, 4.29: the runner node stops during a run and starts again with the same node id. The run goes on."""
    fleet = ctx.fleet("t2-restart", client=ctx.b.client()).create()
    queued_fleet = ctx.fleet("t2-queued", client=ctx.b.client()).create()
    client_b = ctx.b.client()
    node_before = _node_id(ctx.a)

    run = client_b.start_run(fleet.command_body("slow-task", "apply", {"seconds": 40})).json()
    runner = wait_until(lambda: client_b.get_run(run["id"]).get("runner"), 60, what="the start of the child")
    pid = runner["pid"]
    wait_until(lambda: any(e["kind"] == "task_start" for e in client_b.events(run["id"])), 60, what="the first task")

    ctx.a.kill("KILL")
    assert process_exists(ctx.a.exec, pid), "4.19: the child lives after the end of the runner node"
    # While no runner node is up, a request is stored and waits (4.1). Pupitre never refuses it for that reason.
    waiting = client_b.start_run(queued_fleet.check_body()).json()
    time.sleep(3)
    assert client_b.get_run(waiting["id"])["status"] == "queued", "a run waits for a runner node"
    assert client_b.get_run(run["id"])["status"] == "running"
    ctx.b.wait_ready(10)  # the API instance stays ready with no runner node

    ctx.a.clear_log()
    ctx.a.start()
    ctx.a.wait_ready(90)
    assert _node_id(ctx.a) == node_before, "4.8: the node id is the same after a restart"
    wait_until(lambda: "run.adopted" in ctx.a.log_text(), 30, what="the log line run.adopted")
    final = client_b.wait_run(run["id"], timeout=150)
    assert final["status"] == "passed" and final["runner"]["pid"] == pid, final
    assert_contiguous_events(client_b.events(run["id"]))
    queued_final = client_b.wait_run(waiting["id"], timeout=150)
    assert queued_final["status"] == "passed", "the run that waited ran after the restart: %s" % queued_final
    assert queued_final["runner"]["node"] == node_before


def second_runner_node_stops_at_start(ctx):
    """4.8: a second instance with `runner.execute = true` stops at start, with exit code 1 and the message of 4.8."""
    third = ctx.lab.make_instance("node-c", "distributed-runner")
    script = "exec /opt/pupitre/pupitre serve --config %s" % third.config_path
    result = third.exec(["sh", "-c", script], check=False, timeout=90)
    text = result.err + result.out
    assert result.code == 1, "exit code %d, expected 1\n%s" % (result.code, text[-1500:])
    assert "another runner node holds the lease until" in text, text[-1500:]
    # The runner node A is not harmed: it still holds its lease and serves.
    ctx.a.wait_ready(10)
    assert ctx.a.alive()
    fleet = ctx.fleet("t3-after-c", client=ctx.a.client()).create()
    fleet.run_and_wait(fleet.status_body())


def serve_account_has_the_rights_of_2_36(ctx):
    """2.36: the account of `serve` can read and write the rows and cannot change a table."""
    lab = ctx.lab
    owners = lab.psql("SELECT DISTINCT tableowner FROM pg_tables WHERE schemaname = 'public'").split()
    assert owners == [lab.pg_owner], "the tables belong to the account of migrate: %s" % owners
    serve = lab.pg_serve
    assert lab.psql("SELECT has_table_privilege('%s', 'public.documents', 'UPDATE')" % serve) == "t"
    assert lab.psql("SELECT has_table_privilege('%s', 'public.runs', 'DELETE')" % serve) == "t"
    assert lab.psql("SELECT has_schema_privilege('%s', 'public', 'CREATE')" % serve) == "f", "the serve account can make a table"
    assert lab.psql("SELECT has_table_privilege('%s', 'public.documents', 'TRUNCATE')" % serve) == "f"


SCENARIOS = [
    ("serve_account_rights", serve_account_has_the_rights_of_2_36),
    ("request_on_api_node", request_on_the_api_node),
    ("runner_node_restart", runner_node_restart_during_run),
    ("second_runner_node", second_runner_node_stops_at_start),
]
