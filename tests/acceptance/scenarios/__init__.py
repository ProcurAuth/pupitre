# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scenarios of the acceptance tests, and the code that starts the instances that they need.

The scenarios need `pupitre serve`. `server_status` runs the binary one time to tell if `serve` is a stub. A stub gives
the text `not built in this version`. The scenarios are then skipped with a message, or they fail with `--require-server`.
"""
import os
import shutil
import time
import traceback

from harness.report import Report

from . import distributed, large, local
from .common import Fleet

STUB_TEXT = "not built in this version"

GROUPS = {
    "local": local.SCENARIOS,
    "distributed": distributed.SCENARIOS,
    "large": large.SCENARIOS,
}


def catalog() -> list:
    """Each scenario as (name, function, group)."""
    return [(name, func, group) for group, items in GROUPS.items() for name, func in items]


class Context:
    """What a scenario gets: the lab, the instances, the clients and a maker of fleets."""

    def __init__(self, lab, **fields) -> None:
        self.lab = lab
        self.detail = ""
        self.__dict__.update(fields)

    def fleet(self, env: str, hosts=("host-a", "host-b"), client=None) -> Fleet:
        # The host containers are shared by the scenarios: each fleet starts on clean hosts.
        for host in hosts:
            self.lab.docker.exec(self.lab.host(host), ["rm", "-rf", "/etc/demo-service"], check=False)
        return Fleet(client or self.client, env, hosts)


def server_status(lab) -> str:
    """Run `pupitre serve` once in a throwaway container. Return `stub`, or `real`. A server that runs is stopped by `timeout`."""
    result = lab.run_tool("serve-probe", "local", ["serve", "--config", "/etc/pupitre/pupitre.hcl"], check=False, prefix=["timeout", "10"])
    text = result.out + result.err
    return "stub" if STUB_TEXT in text else "real"


# -- the setup of each group -----------------------------------------------------------------------------------------


def _migrate(instance, who_report=None) -> None:
    result = instance.exec(["/opt/pupitre/pupitre", "migrate", "--config", instance.config_path], check=False)
    if result.code != 0:
        raise RuntimeError("migrate failed with code %d: %s%s" % (result.code, result.out, result.err))


def setup_local(lab) -> Context:
    lab.reset_hosts()
    inst = lab.make_instance("local", "local")
    _migrate(inst)
    inst.start()
    inst.wait_ready(90)
    return Context(lab, inst=inst, client=inst.client())


def setup_distributed(lab) -> Context:
    lab.reset_hosts()
    lab.start_postgres()
    result = lab.run_tool("migrate-pg", "distributed-runner", ["migrate", "--config", "/etc/pupitre/pupitre.hcl"], pg_who="owner", check=False)
    if result.code != 0:
        raise RuntimeError("migrate failed with code %d: %s%s" % (result.code, result.out, result.err))
    lab.grant_serve_account()
    a = lab.make_instance("node-a", "distributed-runner")
    b = lab.make_instance("api-b", "distributed-api")
    a.start()
    a.wait_ready(90)
    b.start()
    b.wait_ready(90)
    return Context(lab, a=a, b=b, inst=a, client=a.client())


def setup_large(lab) -> Context:
    blocks = lab.config_blocks_for_large
    inst = lab.make_instance("large", "large", fixture_schema_blocks=blocks, extra=["--volume", "%s:/srv/fixture:ro" % lab.path("fixture")])
    _migrate(inst)
    data = os.path.join(inst.folder, "data")
    source = lab.path("fixture", "data")
    for name in ("environments", ".history"):
        shutil.copytree(os.path.join(source, name), os.path.join(data, name), dirs_exist_ok=True)
    started = time.monotonic()
    inst.start()
    inst.wait_ready(180)
    return Context(lab, inst=inst, client=inst.client(), ready_after=time.monotonic() - started)


SETUPS = {"local": setup_local, "distributed": setup_distributed, "large": setup_large}


def run_all(lab, report: Report, args, generator) -> None:
    """Run the selected scenarios group by group. Skip them all when `serve` is a stub."""
    wanted = set(args.scenario or [name for name, _, _ in catalog()])
    unknown = wanted - {name for name, _, _ in catalog()}
    if unknown:
        report.fail("scenarios", "unknown scenario: %s" % ", ".join(sorted(unknown)))
        return
    lab.config_blocks_for_large = generator.hcl_blocks(generator.PROFILES["large"], "/srv/fixture/schemas")
    status = server_status(lab)
    if status == "stub":
        message = "`pupitre serve` is a stub in this binary (%s). Build the server, then run again with --require-server." % STUB_TEXT
        names = [name for name, _, _ in catalog() if name in wanted]
        (report.fail if args.require_server else report.skip)("scenarios (%d)" % len(names), "%s Scenarios: %s." % (message, ", ".join(names)))
        return
    for group, items in GROUPS.items():
        chosen = [(name, func) for name, func in items if name in wanted]
        if not chosen:
            continue
        try:
            ctx = SETUPS[group](lab)
        except Exception:
            report.fail("group %s: start the instances" % group, traceback.format_exc())
            for name, _ in chosen:
                report.skip("scenario %s" % name, "the group %s did not start" % group)
            continue
        for name, func in chosen:
            started = time.monotonic()
            ctx.detail = ""
            try:
                func(ctx)
            except Exception:
                text = traceback.format_exc()
                tail = ctx.inst.log_text()[-1500:] if getattr(ctx, "inst", None) else ""
                report.fail("scenario %s" % name, "%s\n--- end of the log of the instance ---\n%s" % (text, tail))
            else:
                report.ok("scenario %s" % name, "%.0fs %s" % (time.monotonic() - started, ctx.detail))
