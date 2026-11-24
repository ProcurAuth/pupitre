# SPDX-License-Identifier: MIT OR Apache-2.0
"""A small wrapper around the Docker command line, for the acceptance tests (8.12).

Each container and each network that the harness makes has the name prefix `pupitre-acc-<run>-` and the label
`pupitre-acc.run=<run>`. The wrapper records what it makes. It removes only what it recorded, or what carries the
label of its own run. It never lists, stops or removes a resource that it did not make.
"""
import json
import re
import subprocess
import sys
import threading
import time

LABEL = "pupitre-acc.run"
PREFIX = "pupitre-acc-"


class DockerError(RuntimeError):
    """A Docker command failed."""


class Result:
    """The result of a command."""

    def __init__(self, code: int, out: str, err: str) -> None:
        self.code = code
        self.out = out
        self.err = err


class Docker:
    """The Docker commands of one run. The run id is 8 hexadecimal characters."""

    def __init__(self, run_id: str, log=None) -> None:
        if not re.fullmatch(r"[0-9a-f]{8}", run_id):
            raise ValueError("the run id has 8 hexadecimal characters")
        self.run_id = run_id
        self.prefix = "%s%s-" % (PREFIX, run_id)
        self.containers: list = []
        self.networks: list = []
        self.volumes: set = set()
        self.log = log or (lambda text: print(text, file=sys.stderr, flush=True))
        self._lock = threading.Lock()

    # -- raw commands ---------------------------------------------------------------------------------------------

    def cmd(self, args: list, check: bool = True, input_text: str = None, timeout: float = 600) -> Result:
        """Run `docker <args>` and return its result. With `check`, a failure raises `DockerError`."""
        proc = subprocess.run(
            ["docker"] + args,
            input=input_text,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        result = Result(proc.returncode, proc.stdout, proc.stderr)
        if check and result.code != 0:
            raise DockerError("docker %s failed with code %d: %s" % (" ".join(args[:3]), result.code, result.err.strip()))
        return result

    def name(self, short: str) -> str:
        """The full name of a resource of this run."""
        return self.prefix + short

    # -- images ---------------------------------------------------------------------------------------------------

    def image_exists(self, tag: str) -> bool:
        return self.cmd(["image", "inspect", tag], check=False).code == 0

    def build(self, tag: str, dockerfile: str, context: str, build_args: dict = None) -> None:
        """Build a local image. The harness never pushes an image."""
        args = ["build", "--quiet", "--tag", tag, "--file", dockerfile]
        for key, value in (build_args or {}).items():
            args += ["--build-arg", "%s=%s" % (key, value)]
        args.append(context)
        self.cmd(args, timeout=1800)

    def remove_image(self, tag: str) -> None:
        self.cmd(["image", "rm", tag], check=False)

    # -- networks -------------------------------------------------------------------------------------------------

    def network_create(self, short: str, internal: bool = True) -> str:
        name = self.name(short)
        args = ["network", "create", "--label", "%s=%s" % (LABEL, self.run_id)]
        if internal:
            args.append("--internal")
        args.append(name)
        self.cmd(args)
        with self._lock:
            self.networks.append(name)
        return name

    # -- containers -----------------------------------------------------------------------------------------------

    def create(self, short: str, image: str, options: list, command: list = None) -> str:
        """Make a container with `docker create`. It does not start it."""
        name = self.name(short)
        args = ["create", "--name", name, "--label", "%s=%s" % (LABEL, self.run_id)] + options + [image] + (command or [])
        with self._lock:
            self.containers.append(name)
        self.cmd(args)
        return name

    def start(self, name: str) -> None:
        self.cmd(["start", name])

    def run(self, short: str, image: str, options: list, command: list = None) -> str:
        """Make and start a container in the background."""
        name = self.create(short, image, options, command)
        self.start(name)
        return name

    def run_once(self, short: str, image: str, options: list, command: list, check: bool = True, timeout: float = 600) -> Result:
        """Run a throwaway container in the foreground. The container is removed at its end, also after an error."""
        name = self.name(short)
        args = ["run", "--rm", "--name", name, "--label", "%s=%s" % (LABEL, self.run_id)] + options + [image] + command
        with self._lock:
            self.containers.append(name)
        return self.cmd(args, check=check, timeout=timeout)

    def exec(self, name: str, command: list, user: str = None, check: bool = True, detach: bool = False, input_text: str = None, timeout: float = 600) -> Result:
        args = ["exec"]
        if detach:
            args.append("--detach")
        if input_text is not None:
            args.append("--interactive")
        if user:
            args += ["--user", user]
        args.append(name)
        return self.cmd(args + command, check=check, input_text=input_text, timeout=timeout)

    def cp_in(self, source: str, name: str, target: str) -> None:
        self.cmd(["cp", source, "%s:%s" % (name, target)])

    def inspect(self, name: str) -> dict:
        return json.loads(self.cmd(["inspect", name]).out)[0]

    def ip(self, name: str, network: str) -> str:
        return self.inspect(name)["NetworkSettings"]["Networks"][network]["IPAddress"]

    def logs(self, name: str) -> str:
        result = self.cmd(["logs", name], check=False)
        return result.out + result.err

    def state(self, name: str) -> str:
        result = self.cmd(["inspect", "--format", "{{.State.Status}}", name], check=False)
        return result.out.strip()

    def stop(self, name: str, seconds: int = 5) -> None:
        self.cmd(["stop", "--time", str(seconds), name], check=False)

    # -- cleanup --------------------------------------------------------------------------------------------------

    def _remember_volumes(self, name: str) -> None:
        result = self.cmd(["inspect", "--format", "{{json .Mounts}}", name], check=False)
        if result.code != 0 or not result.out.strip():
            return
        for mount in json.loads(result.out):
            if mount.get("Type") == "volume" and mount.get("Name"):
                self.volumes.add(mount["Name"])

    def cleanup(self) -> list:
        """Remove each container, network and volume of this run. Return the list of the problems."""
        problems: list = []
        for name in reversed(self.containers):
            self._remember_volumes(name)
            result = self.cmd(["rm", "--force", "--volumes", name], check=False)
            if result.code != 0 and "No such container" not in result.err:
                problems.append("container %s: %s" % (name, result.err.strip()))
        for name in reversed(self.networks):
            result = self.cmd(["network", "rm", name], check=False)
            if result.code != 0 and "not found" not in result.err.lower():
                problems.append("network %s: %s" % (name, result.err.strip()))
        for name in sorted(self.volumes):
            result = self.cmd(["volume", "rm", name], check=False)
            if result.code != 0 and "no such volume" not in result.err.lower():
                problems.append("volume %s: %s" % (name, result.err.strip()))
        return problems

    def leftovers(self) -> list:
        """List what of this run still exists: by label and by recorded name. An empty list is a clean run."""
        found: list = []
        label = "label=%s=%s" % (LABEL, self.run_id)
        for kind, args in (
            ("container", ["ps", "--all", "--quiet", "--filter", label]),
            ("network", ["network", "ls", "--quiet", "--filter", label]),
            ("volume", ["volume", "ls", "--quiet", "--filter", label]),
        ):
            for line in self.cmd(args, check=False).out.split():
                found.append("%s %s" % (kind, line))
        for name in self.containers:
            if self.cmd(["container", "inspect", name], check=False).code == 0:
                found.append("container %s" % name)
        for name in self.networks:
            if self.cmd(["network", "inspect", name], check=False).code == 0:
                found.append("network %s" % name)
        for name in sorted(self.volumes):
            if self.cmd(["volume", "inspect", name], check=False).code == 0:
                found.append("volume %s" % name)
        return found


def wait_until(check, timeout: float, step: float = 0.5, what: str = "a condition"):
    """Call `check()` until it gives a true value. Return the value. Raise `TimeoutError` after `timeout` seconds."""
    deadline = time.monotonic() + timeout
    last = None
    while True:
        try:
            last = check()
        except Exception as error:  # a step of a poll can fail while a service starts
            last = error
            value = None
        else:
            value = last
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError("timeout after %ss while waiting for %s (last: %r)" % (timeout, what, last))
        time.sleep(step)
