# SPDX-License-Identifier: MIT OR Apache-2.0
"""Helpers shared by the scenarios: a fleet of hosts with one unit, the run requests of 4.4, and the checks."""
import json
import re

from harness.client import ApiError, Client
from harness.lab import host_address

UNKNOWN_RUN = "0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42"
SCHEMA = "demo"
PLAYBOOKS = {"deploy": "deploy-demo", "render": "render-demo", "status": "status-demo"}
FINGERPRINT = re.compile(r"^[0-9a-f]{16}$")
SNAPSHOT = re.compile(r"^sha256:[0-9a-f]{64}$")


class Fleet:
    """One environment with some hosts and one unit of the schema `demo`. The ids carry the name of the scenario."""

    def __init__(self, client: Client, env: str, hosts=("host-a", "host-b"), unit: str = "demo") -> None:
        self.client = client
        self.env = env
        self.unit = unit
        self.hosts = list(hosts)

    @property
    def base(self) -> str:
        return "/api/environments/%s" % self.env

    @property
    def unit_path(self) -> str:
        return "%s/units/%s" % (self.base, self.unit)

    @property
    def node(self) -> str:
        """The node path of the unit (3.8)."""
        return "environment:%s/unit:%s" % (self.env, self.unit)

    def create(self, banner: str = "hello", max_clients: int = 250, with_banner: bool = True) -> "Fleet":
        """Write the environment, the host documents, the unit and its values. The order follows 2.16."""
        c = self.client
        c.create_environment(self.env, "Acceptance %s" % self.env)
        for host in self.hosts:
            c.create_document("%s/hosts/%s" % (self.base, host), {"address": host_address(host), "format": 1, "labels": []}, "acceptance")
        unit = {
            "format": 1,
            "hosts": sorted(self.hosts),
            "label": "Demo unit",
            "playbooks": PLAYBOOKS,
            "schema": SCHEMA,
        }
        c.create_document(self.unit_path, unit, "acceptance")
        values = {"limits.max_clients": max_clients}
        if with_banner:
            values["banner"] = banner
        c.create_document(self.unit_path + "/values", {"format": 1, "values": values}, "acceptance")
        return self

    def set_values(self, **values) -> None:
        """Save the document `unit_values` with new values. `limits_max_clients` stands for `limits.max_clients`."""
        document, etag = self.client.get_document(self.unit_path + "/values")
        for name, value in values.items():
            document["values"][name.replace("__", ".")] = value
        self.client.save_document(self.unit_path + "/values", document, etag, "acceptance")

    def set_secret(self, name: str, value: str) -> dict:
        return self.client.put_secret(self.unit_path, name, value)

    # -- the run requests (4.4) -----------------------------------------------------------------------------------

    def check_body(self, hosts=None, message: str = "acceptance check") -> dict:
        return {"kind": "deploy", "mode": "check", "node": self.node, "hosts": sorted(hosts or self.hosts), "message": message}

    def status_body(self, hosts=None) -> dict:
        return {"kind": "status", "mode": "apply", "node": self.node, "hosts": sorted(hosts or self.hosts)}

    def command_body(self, command: str, mode: str, params: dict, hosts=None) -> dict:
        return {"kind": "command", "mode": mode, "command": command, "node": self.node, "hosts": sorted(hosts or self.hosts), "params": params}

    @staticmethod
    def apply_body(check_run: str, message: str = "acceptance apply") -> dict:
        return {"kind": "deploy", "mode": "apply", "check_run": check_run, "message": message}

    # -- the steps ------------------------------------------------------------------------------------------------

    def run_and_wait(self, body: dict, timeout: float = 180.0, expect_status: str = "passed") -> dict:
        """Start a run, wait for its end, and check the final status. Return the final record."""
        answer = self.client.start_run(body)
        record = answer.json()
        assert record["status"] == "queued", "a new run has the status queued, not %r" % record["status"]
        assert answer.headers["location"].endswith("/api/runs/%s" % record["id"]), answer.headers.get("location")
        final = self.client.wait_run(record["id"], timeout=timeout)
        assert final["status"] == expect_status, "run %s ended %s (%s), expected %s\n%s" % (
            record["id"], final["status"], final.get("reason"), expect_status, output_tail(self.client, record["id"]),
        )
        return final

    def deploy(self) -> dict:
        """A check, then its apply. Return the record of the apply."""
        check = self.run_and_wait(self.check_body())
        return self.run_and_wait(self.apply_body(check["id"]))


def output_tail(client: Client, run: str, size: int = 2500) -> str:
    try:
        return client.output(run).decode("utf-8", "replace")[-size:]
    except Exception as error:  # the tail is a help for the reader; a failure here must not hide the real error
        return "(no output: %s)" % error


def expect_error(expected_status: int, code: str, call) -> ApiError:
    """Call a function that must fail with this status and this error code (5.4). Return the error."""
    try:
        call()
    except ApiError as error:
        assert error.status == expected_status and error.code == code, "expected %d %s, got %d %s: %s" % (expected_status, code, error.status, error.code, error.body)
        return error
    raise AssertionError("expected %d %s, but the call succeeded" % (expected_status, code))


def process_exists(container_exec, pid: int) -> bool:
    """True when the process `pid` exists in the container."""
    return container_exec(["test", "-d", "/proc/%d" % pid], check=False).code == 0


def event_kinds(events: list) -> list:
    return [event["kind"] for event in events]


def assert_contiguous_events(events: list) -> None:
    """The numbers `seq` of the stored events are 1, 2, 3 and so on, with no gap and no copy (4.20, 2.28)."""
    seqs = [event["seq"] for event in events]
    assert seqs == list(range(1, len(seqs) + 1)), "the event numbers are not contiguous: %s" % seqs[:40]


def hosts_state(lab, hosts, path: str) -> dict:
    """The content of a file on each host, or None when the file does not exist."""
    state = {}
    for host in hosts:
        state[host] = lab.host_file(host, path) if lab.host_has_file(host, path) else None
    return state


def config_on(lab, host: str):
    """The parsed `/etc/demo-service/config.json` of a host, or None."""
    if not lab.host_has_file(host, "/etc/demo-service/config.json"):
        return None
    return json.loads(lab.host_file(host, "/etc/demo-service/config.json"))

