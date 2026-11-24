# SPDX-License-Identifier: MIT OR Apache-2.0
"""The smoke test of the images and of the host containers. It does not start Pupitre.

It writes by hand the files that a snapshot holds (4.10 to 4.15) and runs the real `ansible-playbook` of the runner
image against the host containers with the synthetic playbooks. It proves that:
  - the pinned `ansible-core` runs with the command line of 4.17 and the environment of 4.18;
  - the host containers accept the key of the runner, and only with the host key of the host id (7.8);
  - the render, deploy, status and command playbooks work, in check mode and in apply mode;
  - the output has the facts that the scenarios need: a diff in check mode, no secret with `no_log`;
  - a value in a collection with the tag `!unsafe` is never evaluated (4.12, 7.3).
"""
import json
import re

from .lab import HOST_NAMES, Lab, Instance, host_address

SECRET_PASSWORD = "Acc-Secret-7f3a91c2d4e8"
SECRET_TOKEN = 'tok"en\\back-4d2e91ab7c'
RUN_ID = "0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42"
ENV = "smoke"
UNIT = "demo"
RUN_HOSTS = ("host-a", "host-b")
# A file that a lookup of the plugin `pipe` makes on the runner. The tag `!unsafe` must stop the lookup.
MARK = "/tmp/pupitre-acc-unsafe-mark"
LOOKUP = "{{ lookup('pipe', 'touch %s') }}" % MARK


def yaml_text(value, unsafe: bool = False, level: int = 0, unsafe_members=()) -> str:
    """The YAML form of the runner (4.12): the layout of 2.4, and the tag `!unsafe` on each collection of a value.

    `unsafe_members` names the members of the top object whose values come from documents.
    """
    tag = "!unsafe " if unsafe else ""
    pad = "  " * (level + 1)
    if isinstance(value, dict):
        if not value:
            return tag + "{}"
        items = ["%s%s: %s" % (pad, json.dumps(k), yaml_text(v, unsafe or k in unsafe_members, level + 1)) for k, v in sorted(value.items())]
        return tag + "{\n" + ",\n".join(items) + "\n" + "  " * level + "}"
    if isinstance(value, list):
        if not value:
            return tag + "[]"
        items = [pad + yaml_text(v, unsafe, level + 1) for v in value]
        return tag + "[\n" + ",\n".join(items) + "\n" + "  " * level + "]"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if not (isinstance(value, str) and value.isascii()):
        raise ValueError("the smoke test writes ASCII texts only: %r" % (value,))
    return json.dumps(value)


class Smoke:
    """The steps of the smoke test, in order. Each step raises `AssertionError` when it fails."""

    def __init__(self, lab: Lab, runner: Instance) -> None:
        self.lab = lab
        self.runner = runner
        # The paths inside the container.
        self.run = "/var/lib/pupitre/runner/runs/%s" % RUN_ID
        self.snapshot = self.run + "/snapshot"
        self.secrets_dir = "/run/pupitre/secrets/%s/values" % RUN_ID
        self.render_dir = "/run/pupitre/secrets/%s/render" % RUN_ID
        self.last_output = ""

    # -- the files of a snapshot ----------------------------------------------------------------------------------

    def write(self, path: str, text: str, mode: str = "0644") -> None:
        """Write a file in the container, as the user of the runner."""
        self.runner.exec(["sh", "-c", 'mkdir -p "$(dirname "$1")" && cat > "$1" && chmod %s "$1"' % mode, "sh", path], input_text=text)

    def write_json(self, path: str, value) -> None:
        self.write(path, json.dumps(value, indent=2, sort_keys=True) + "\n")

    def prepare(self, banner: str = "hello", max_clients: int = 250, with_secrets: bool = True, address_of: dict = None, tagged: bool = True) -> None:
        """Write the inventory, the variables, the secrets and the `ansible.cfg` of a run (4.11 to 4.15).

        `address_of` gives a host the address of another host. `tagged = False` writes the old form, JSON with no tag.
        """
        all_hosts = {name: {} for name in HOST_NAMES}
        groups = {"demo_hosts": {"hosts": {name: {} for name in RUN_HOSTS}}, "pupitre_run": {"hosts": {name: {} for name in RUN_HOSTS}}}
        self.write_json(self.snapshot + "/inventory/hosts.yml", {"all": {"children": groups, "hosts": all_hosts}})
        for host in HOST_NAMES:
            variables = {
                "ansible_host": host_address((address_of or {}).get(host, host)),
                "ansible_ssh_common_args": "-o HostKeyAlias=%s.%s" % (ENV, host),
                "pupitre_environment": ENV,
                "pupitre_units": [UNIT] if host in RUN_HOSTS else [],
            }
            config = None
            if host in RUN_HOSTS:
                config = {"banner": banner, "limits": {"max_clients": max_clients}, "log": {"level": "info"}}
                if with_secrets:
                    base = "{{ pupitre_secrets_dir }}/environments/%s/units/%s" % (ENV, UNIT)
                    variables["pupitre_secret_files"] = {UNIT: {"api.token": base + "/api.token", "storage.password": base + "/storage.password"}}
            if config is not None:
                variables["demo_config"] = config
            path = "%s/inventory/host_vars/%s.yml" % (self.snapshot, host)
            if tagged:
                self.write(path, yaml_text(variables, unsafe_members=("demo_config",)) + "\n")
            else:
                self.write_json(path, variables)
        self.write_json(self.snapshot + "/vars/run.json", {"pupitre_kind": "deploy", "pupitre_node": "environment:%s/unit:%s" % (ENV, UNIT), "pupitre_playbook": "deploy-demo"})
        self.write_json(self.run + "/run-vars.json", {"pupitre_run_id": RUN_ID, "pupitre_secrets_dir": self.secrets_dir, "pupitre_render_dir": self.render_dir})
        self.runner.exec(["mkdir", "-p", self.render_dir, self.run + "/home", self.run + "/tmp"])
        if with_secrets:
            self.write("%s/environments/%s/units/%s/storage.password" % (self.secrets_dir, ENV, UNIT), SECRET_PASSWORD, "0600")
            self.write("%s/environments/%s/units/%s/api.token" % (self.secrets_dir, ENV, UNIT), SECRET_TOKEN, "0600")
        self.write_cfg("/run/secrets/ssh/known_hosts")

    def write_cfg(self, known_hosts: str) -> None:
        """The file `ansible.cfg` of 4.15, with no callback plugin: the plugin belongs to the runner."""
        cfg = (
            "[defaults]\n"
            "inventory = %s/inventory/hosts.yml\n"
            "retry_files_enabled = False\n"
            "host_key_checking = True\n"
            "remote_user = deploy\n"
            "private_key_file = /run/secrets/ssh/id_ed25519\n"
            "forks = 5\n"
            "local_tmp = %s/tmp\n"
            "\n"
            "[ssh_connection]\n"
            "ssh_args = -o UserKnownHostsFile=%s -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes "
            "-o CheckHostIP=no -o IdentitiesOnly=yes -o ForwardAgent=no -o PasswordAuthentication=no\n"
        ) % (self.snapshot, self.run, known_hosts)
        self.write(self.run + "/ansible.cfg", cfg)

    def params(self, params: dict) -> None:
        self.write(self.snapshot + "/vars/params.yml", yaml_text({"pupitre_params": params}, unsafe_members=("pupitre_params",)) + "\n")

    # -- the command ----------------------------------------------------------------------------------------------

    def ansible(self, playbook: str, check: bool = False, params: bool = False, expect_code: int = 0):
        """Run `ansible-playbook` as 4.17 and 4.18 say: no shell, an explicit environment. Return the result."""
        command = [
            "env", "-i",
            "PATH=/usr/local/bin:/usr/bin:/bin",
            "HOME=%s/home" % self.run,
            "LANG=C.UTF-8", "LC_ALL=C.UTF-8",
            "ANSIBLE_CONFIG=%s/ansible.cfg" % self.run,
            "ANSIBLE_LOCAL_TEMP=%s/tmp" % self.run,
            "PYTHONDONTWRITEBYTECODE=1",
            "/opt/ansible/bin/ansible-playbook",
            "--inventory", self.snapshot + "/inventory/hosts.yml",
            "--limit", "pupitre_run",
            "--extra-vars", "@" + self.snapshot + "/vars/run.json",
        ]
        if params:
            command += ["--extra-vars", "@" + self.snapshot + "/vars/params.yml"]
        command += ["--extra-vars", "@" + self.run + "/run-vars.json"]
        if check:
            command += ["--check", "--diff"]
        command.append("/srv/ansible/playbooks/%s.yml" % playbook)
        result = self.runner.exec(command, check=False, timeout=300)
        self.last_output = result.out + result.err
        if result.code != expect_code:
            raise AssertionError("ansible-playbook %s: exit code %d, expected %d\n%s" % (playbook, result.code, expect_code, self.last_output[-3000:]))
        return result

    def recap(self, output: str) -> dict:
        """The recap of a run: host to a dict of counts."""
        recap = {}
        for match in re.finditer(r"^(\S+)\s*: ok=(\d+)\s+changed=(\d+)\s+unreachable=(\d+)\s+failed=(\d+)", output, re.MULTILINE):
            recap[match.group(1)] = {"ok": int(match.group(2)), "changed": int(match.group(3)), "unreachable": int(match.group(4)), "failed": int(match.group(5))}
        return recap

    # -- the steps ------------------------------------------------------------------------------------------------

    def step_pinned_keys(self) -> str:
        self.lab.verify_pinned_host_keys(self.runner)
        return "the scanned host keys equal the pinned keys"

    def step_render_connects_to_no_host(self) -> str:
        before = {h: self.lab.host_ssh_logins(h) for h in RUN_HOSTS}
        self.ansible("render-demo")
        recap = self.recap(self.last_output)
        assert set(recap) == set(RUN_HOSTS), recap
        for host in RUN_HOSTS:
            data = json.loads(self.runner.read_file("%s/%s/config.json" % (self.render_dir, host)))
            assert data == {"banner": "hello", "limits": {"max_clients": 250}, "log": {"level": "info"}}, data
        after = {h: self.lab.host_ssh_logins(h) for h in RUN_HOSTS}
        assert before == after, "the render phase opened an SSH session: %s -> %s" % (before, after)
        return "render wrote the files in the render folder with no SSH login"

    def step_check_changes_nothing(self) -> str:
        self.ansible("deploy-demo", check=True)
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["changed"] > 0 and recap[host]["failed"] == 0, recap
            assert not self.lab.host_has_file(host, "/etc/demo-service/config.json"), "the check changed %s" % host
        assert '"banner"' in self.last_output and "--- before" in self.last_output, "the check has no diff of the configuration"
        assert SECRET_PASSWORD not in self.last_output, "no_log must hide the password in the diff"
        return "a check shows the diff and changes no host"

    def step_apply_deploys(self) -> str:
        self.ansible("deploy-demo")
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["failed"] == 0 and recap[host]["unreachable"] == 0, recap
            data = json.loads(self.lab.host_file(host, "/etc/demo-service/config.json"))
            assert data == {"banner": "hello", "limits": {"max_clients": 250}, "log": {"level": "info"}}, data
            assert self.lab.host_file(host, "/etc/demo-service/password") == SECRET_PASSWORD
            active = self.lab.docker.exec(self.lab.host(host), ["systemctl", "is-active", "demo-service"], check=False).out.strip()
            assert active == "active", "demo-service on %s is %r" % (host, active)
        assert not self.lab.host_has_file("host-c", "/etc/demo-service/config.json"), "host-c is not in the limit"
        assert SECRET_PASSWORD not in self.last_output, "the output of the apply holds the password"
        return "an apply wrote the configuration and the secret, and started the service on the two hosts only"

    def step_apply_is_idempotent(self) -> str:
        self.ansible("deploy-demo")
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["changed"] == 0, "a second apply changed %s: %s" % (host, recap)
        self.ansible("deploy-demo", check=True)
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["changed"] == 0, "a check after the apply shows a difference on %s" % host
        return "a second apply and a check after the apply change nothing"

    def step_changed_value_shows_in_check(self) -> str:
        self.prepare(banner="changed", max_clients=300)
        self.ansible("deploy-demo", check=True)
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["changed"] > 0, recap
        assert "changed" in self.last_output
        data = json.loads(self.lab.host_file("host-a", "/etc/demo-service/config.json"))
        assert data["banner"] == "hello", "the check changed the host"
        return "a changed value shows as a change in a check"

    def step_status_reads(self) -> str:
        self.ansible("status-demo")
        for host in RUN_HOSTS:
            assert "demo-service is active on %s" % host in self.last_output, self.last_output[-1500:]
        return "the status playbook prints the state of each host"

    def step_command_with_check(self) -> str:
        self.params({"banner": "rotated"})
        self.ansible("rotate-banner", check=True, params=True)
        assert not self.lab.host_has_file("host-a", "/etc/demo-service/banner"), "the check wrote the banner"
        assert "rotated" in self.last_output
        self.ansible("rotate-banner", params=True)
        for host in RUN_HOSTS:
            assert self.lab.host_file(host, "/etc/demo-service/banner") == "rotated\n"
        return "a command with a check shows a diff, then writes the banner"

    def step_command_without_check(self) -> str:
        self.params({"grace": 1000})
        self.ansible("restart-service", params=True)
        self.params({"seconds": 1})
        self.ansible("slow-task", params=True)
        return "the commands restart-service and slow-task run"

    def step_secrets_are_printed_without_the_scrubber(self) -> str:
        """The playbook leaks on purpose. This step proves that the scenario of the scrubber has a real leak to find."""
        self.params({"mode": "print"})
        self.ansible("leak-secret", params=True)
        assert SECRET_PASSWORD in self.last_output, "the leak playbook does not print the password"
        self.params({"mode": "fail"})
        self.ansible("leak-secret", params=True, expect_code=2)
        assert SECRET_PASSWORD in self.last_output
        return "the leak playbook prints the password in the output, as the scrubber test needs"

    def step_wrong_host_key_stops(self) -> str:
        self.prepare()
        self.write_cfg("/run/secrets/ssh-wrong/known_hosts")
        self.ansible("status-demo", expect_code=4)
        recap = self.recap(self.last_output)
        for host in RUN_HOSTS:
            assert recap[host]["unreachable"] > 0, recap
        assert "Host key verification failed" in self.last_output or "host key" in self.last_output.lower(), self.last_output[-1500:]
        self.write_cfg("/run/secrets/ssh/known_hosts")
        return "a host key that differs from the pinned key stops the connection"

    def step_address_of_another_host_stops(self) -> str:
        """H1: host-a gets the address of host-b. SSH looks for the key of `smoke.host-a`: the key of host-b fails."""
        self.prepare(address_of={"host-a": "host-b"})
        self.ansible("status-demo", expect_code=4)
        recap = self.recap(self.last_output)
        assert recap["host-a"]["unreachable"] > 0 and recap["host-b"]["unreachable"] == 0, recap
        assert "Host key verification failed" in self.last_output or "host key" in self.last_output.lower(), self.last_output[-1500:]
        assert "demo-service is active on host-a" not in self.last_output, "host-a reached the machine of host-b"
        self.prepare()
        return "a host with the address of another host stops at the check of the host key of its id"

    def step_template_marker_stays_text(self) -> str:
        """C1: a lookup in a value of a collection with the tag `!unsafe` never runs, in the render and in the check."""
        self.runner.exec(["rm", "-f", MARK])
        self.prepare(banner=LOOKUP)
        self.ansible("render-demo")
        for host in RUN_HOSTS:
            data = json.loads(self.runner.read_file("%s/%s/config.json" % (self.render_dir, host)))
            assert data["banner"] == LOOKUP, "the rendered file does not hold the literal text: %r" % data
        self.ansible("deploy-demo", check=True)
        assert "lookup('pipe'" in self.last_output, "the diff of the check does not show the literal text"
        self.params({"banner": LOOKUP})
        self.ansible("rotate-banner", check=True, params=True)
        marked = self.runner.exec(["test", "-e", MARK], check=False).code == 0
        assert not marked, "a lookup with the tag !unsafe ran on the runner"
        # The old form: the same value in JSON with no tag. The lookup runs: the step can find the defect.
        self.prepare(banner=LOOKUP, tagged=False)
        self.ansible("deploy-demo", check=True)
        assert self.runner.exec(["test", "-e", MARK], check=False).code == 0, "the old form did not run the lookup"
        self.runner.exec(["rm", "-f", MARK])
        self.prepare()
        return "a lookup in a tagged value stays text in the render, the check and the parameters; the old form ran it"

    def step_remote_state_is_clean(self) -> str:
        """The hosts and the runner hold no stray secret file outside the planned place."""
        for host in RUN_HOSTS:
            out = self.lab.docker.exec(self.lab.host(host), ["grep", "-rl", SECRET_PASSWORD, "/etc", "/home", "/var/log", "/usr/local"], check=False).out.split()
            assert out == ["/etc/demo-service/password"], "the password is in %s on %s" % (out, host)
        return "the password is only in /etc/demo-service/password on each host"

    def step_harness_reaches_a_container(self) -> str:
        """The scenarios connect from the host to a container on the private network. Check that route with a plain server."""
        import http.client

        from .dockerctl import wait_until

        self.runner.exec(["sh", "-c", "mkdir -p /tmp/www && echo reachable > /tmp/www/index.html && cd /tmp/www && exec python3 -m http.server 8080 --bind 0.0.0.0 >/dev/null 2>&1"], detach=True)

        def reachable():
            connection = http.client.HTTPConnection(self.runner.ip(), 8080, timeout=3)
            try:
                connection.request("GET", "/index.html")
                return connection.getresponse().read().strip() == b"reachable"
            finally:
                connection.close()

        wait_until(reachable, 20, what="a plain HTTP server in the runner container")
        self.runner.exec(["pkill", "-f", "http.server"], check=False)
        return "the host reaches a container of the internal network by its IP address"

    def steps(self) -> list:
        return [
            ("the private network", self.step_harness_reaches_a_container),
            ("the pinned host keys", self.step_pinned_keys),
            ("write the files of a snapshot", lambda: (self.prepare(), "written")[1]),
            ("render connects to no host", self.step_render_connects_to_no_host),
            ("deploy in check mode", self.step_check_changes_nothing),
            ("deploy in apply mode", self.step_apply_deploys),
            ("deploy is idempotent", self.step_apply_is_idempotent),
            ("a changed value", self.step_changed_value_shows_in_check),
            ("status", self.step_status_reads),
            ("command with a check", self.step_command_with_check),
            ("commands without a check", self.step_command_without_check),
            ("the leak playbook", self.step_secrets_are_printed_without_the_scrubber),
            ("a wrong host key", self.step_wrong_host_key_stops),
            ("the address of another host", self.step_address_of_another_host_stops),
            ("a template marker stays text", self.step_template_marker_stays_text),
            ("no stray secret on the hosts", self.step_remote_state_is_clean),
        ]


def run(lab: Lab, runner: Instance, report) -> bool:
    """Run the steps. `report(name, ok, detail)` gets each result. Return true when all steps pass."""
    smoke = Smoke(lab, runner)
    for name, step in smoke.steps():
        try:
            detail = step()
        except Exception as error:  # a failed step stops the others: each one needs the state of the one before
            report("smoke: " + name, False, str(error))
            return False
        report("smoke: " + name, True, detail or "")
    return True
