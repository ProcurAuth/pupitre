# SPDX-License-Identifier: MIT OR Apache-2.0
"""The laboratory of the acceptance tests (8.12): images, keys, host containers, runner containers, PostgreSQL.

Everything that the lab makes is a throwaway container or network with the prefix `pupitre-acc-<run>-`, a folder
below one temporary folder, or an image with the tag `pupitre-acc-host` or `pupitre-acc-runner`. The lab never
connects to a real server: the network of the lab is internal, and the host containers are the only targets.
"""
import hashlib
import os
import secrets
import shutil
import stat
import tempfile

from .client import Client
from .dockerctl import Docker, DockerError, wait_until

HOST_NAMES = ("host-a", "host-b", "host-c")
DOMAIN = "hosts.example.invalid"
PG_NAME = "postgres.example.invalid"
# The base image of PostgreSQL is pinned by digest. The tag is for the reader only.
POSTGRES_IMAGE = "postgres:18.4-trixie@sha256:a02db8cac496f15b094798a38254f14d6e00741f709360e5e00bb6668ea31636"
RUNNER_SSH_USER = "deploy"

# The profiles of the configuration files (config/header-<profile>.hcl).
PROFILES = ("local", "distributed-runner", "distributed-api", "large")


def host_address(host: str) -> str:
    """The DNS name of a host container on the private network. It is the `address` of the document `host`."""
    return "%s.%s" % (host, DOMAIN)


def known_hosts_name(host: str) -> str:
    """The name of a host in the file of pinned host keys. The runner looks for `<environment>.<host>` (4.12, 7.8).

    Each scenario makes its own environment, so the harness writes the pattern `*.<host>` of OpenSSH: the key of a
    host id then holds in each environment. A real file names each environment.
    """
    return "*.%s" % host


def rmtree_force(path: str) -> None:
    """Remove a folder, also when the snapshot of a run made its folders read-only (4.10)."""
    for base, dirs, files in os.walk(path):
        for name in dirs + files:
            full = os.path.join(base, name)
            if not os.path.islink(full):
                os.chmod(full, os.stat(full).st_mode | stat.S_IRWXU)
    os.chmod(path, 0o700)
    shutil.rmtree(path)


def sha12(*paths: str) -> str:
    digest = hashlib.sha256()
    for path in paths:
        with open(path, "rb") as handle:
            digest.update(handle.read())
    return digest.hexdigest()[:12]


class Instance:
    """One container of the runner image, and the Pupitre process that runs in it."""

    def __init__(self, lab: "Lab", name: str, profile: str, container: str, folder: str) -> None:
        self.lab = lab
        self.name = name
        self.profile = profile
        self.container = container
        self.folder = folder  # on the host; mounted at /var/lib/pupitre in the container
        self.network = lab.network
        self.config_path = "/etc/pupitre/pupitre.hcl"

    # -- the process ----------------------------------------------------------------------------------------------

    @property
    def log_path(self) -> str:
        return os.path.join(self.folder, "logs", "pupitre.log")

    @property
    def pid_path(self) -> str:
        return os.path.join(self.folder, "logs", "pupitre.pid")

    def start(self) -> None:
        """Start `pupitre serve` in the container. The process has no shell around it: `exec` replaces the shell."""
        script = "echo $$ > /var/lib/pupitre/logs/pupitre.pid; exec /opt/pupitre/pupitre serve --config %s 2>>/var/lib/pupitre/logs/pupitre.log" % self.config_path
        # Remove the old pid file first: a reader must never take the pid of a process that ended.
        try:
            os.remove(self.pid_path)
        except FileNotFoundError:
            pass
        self.lab.docker.exec(self.container, ["sh", "-c", script], detach=True)
        wait_until(lambda: os.path.exists(self.pid_path) and os.path.getsize(self.pid_path) > 0, 15, what="the pid file of %s" % self.name)

    def pid(self) -> int:
        with open(self.pid_path, encoding="utf-8") as handle:
            return int(handle.read().strip())

    def alive(self) -> bool:
        try:
            pid = self.pid()
        except (OSError, ValueError):
            return False
        return self.lab.docker.exec(self.container, ["test", "-d", "/proc/%d" % pid], check=False).code == 0

    def kill(self, signal: str = "KILL") -> None:
        """Send a signal to the process of Pupitre only. The children of a run are in their own group (4.19)."""
        self.lab.docker.exec(self.container, ["kill", "-%s" % signal, str(self.pid())])
        wait_until(lambda: not self.alive(), 30, what="the end of the process of %s" % self.name)

    def log_text(self) -> str:
        try:
            with open(self.log_path, encoding="utf-8", errors="replace") as handle:
                return handle.read()
        except OSError:
            return ""

    def clear_log(self) -> None:
        with open(self.log_path, "w", encoding="utf-8"):
            pass

    def ip(self) -> str:
        return self.lab.docker.ip(self.container, self.network)

    def client(self) -> Client:
        return Client("http://%s:8080/" % self.ip())

    def ops_url(self) -> str:
        return "http://%s:9090" % self.ip()

    def wait_ready(self, timeout: float = 60.0) -> None:
        """Wait until the ops listener answers `/readyz` with 200 (5.20)."""
        import http.client

        def ready():
            if not self.alive():
                raise RuntimeError("the process of %s ended:\n%s" % (self.name, self.log_text()[-2000:]))
            connection = http.client.HTTPConnection(self.ip(), 9090, timeout=3)
            try:
                connection.request("GET", "/readyz")
                return connection.getresponse().status == 200
            finally:
                connection.close()

        wait_until(ready, timeout, what="/readyz of %s" % self.name)

    def exec(self, command: list, **kwargs):
        return self.lab.docker.exec(self.container, command, **kwargs)

    def read_file(self, path: str) -> str:
        return self.exec(["cat", path]).out


class Lab:
    """The throwaway laboratory of one run."""

    def __init__(self, repo: str, binary: str, run_id: str, log) -> None:
        self.repo = os.path.abspath(repo)
        self.binary = os.path.abspath(binary)
        self.acc = os.path.join(self.repo, "tests", "acceptance")
        self.docker = Docker(run_id, log)
        self.log = log
        self.uid = os.getuid()
        self.gid = os.getgid()
        self.work = tempfile.mkdtemp(prefix="pupitre-acc-%s-" % run_id)
        os.chmod(self.work, 0o755)
        self.network = None
        self.host_image = None
        self.runner_image = None
        self.instances: dict = {}
        self.postgres_container = None

    # -- the images -----------------------------------------------------------------------------------------------

    def build_images(self) -> None:
        """Build the two local images. The tag holds the hash of the recipe, so a build runs one time for each recipe."""
        images = os.path.join(self.acc, "images")
        self.host_image = "pupitre-acc-host:" + sha12(os.path.join(images, "Dockerfile.host"), os.path.join(images, "sshd-acceptance.conf"))
        self.runner_image = "pupitre-acc-runner:%s-u%d" % (
            sha12(os.path.join(images, "Dockerfile.ansible"), os.path.join(images, "ansible-constraints.txt")),
            self.uid,
        )
        if not self.docker.image_exists(self.host_image):
            self.log("build the host image %s" % self.host_image)
            self.docker.build(self.host_image, os.path.join(images, "Dockerfile.host"), images)
        if not self.docker.image_exists(self.runner_image):
            self.log("build the runner image %s" % self.runner_image)
            self.docker.build(self.runner_image, os.path.join(images, "Dockerfile.ansible"), images, {"RUNNER_UID": str(self.uid)})

    # -- the folders and the keys ---------------------------------------------------------------------------------

    def path(self, *parts: str) -> str:
        return os.path.join(self.work, *parts)

    def prepare(self) -> None:
        """Make the binary copy, the SSH keys, the store key, the database credentials and the certificates."""
        os.makedirs(self.path("bin"))
        shutil.copy(self.binary, self.path("bin", "pupitre"))
        os.chmod(self.path("bin", "pupitre"), 0o755)
        self.network = self.docker.network_create("net", internal=True)
        self._make_ssh_keys()
        os.makedirs(self.path("store-key"), mode=0o700)
        self._write_secret(self.path("store-key", "store-key"), secrets.token_hex(32))
        self._make_database_files()

    def _tool(self, short: str, folder: str, command: list, extra: list = None) -> None:
        """Run a throwaway command in the runner image with one writable folder at /out."""
        options = ["--user", "%d:%d" % (self.uid, self.gid), "--network", "none", "--volume", "%s:/out" % folder]
        options += ["--volume", "%s:/harness:ro" % os.path.join(self.acc, "harness")]
        self.docker.run_once(short, self.runner_image, options + (extra or []), command)

    def _make_ssh_keys(self) -> None:
        keys = self.path("keys")
        os.makedirs(keys)
        script = "set -e; cd /out; ssh-keygen -q -t ed25519 -N '' -C 'pupitre-acc runner' -f id_ed25519"
        for host in HOST_NAMES:
            script += "; mkdir -p hosts/%s; ssh-keygen -q -t ed25519 -N '' -C '%s' -f hosts/%s/ssh_host_ed25519_key" % (host, host, host)
        self._tool("keygen", keys, ["sh", "-c", script])
        with open(self.path("keys", "id_ed25519.pub"), encoding="utf-8") as handle:
            public = handle.read().strip()
        with open(self.path("keys", "authorized_keys"), "w", encoding="utf-8") as handle:
            handle.write(public + "\n")
        # The runner mounts one folder: the private key and the pinned host keys (7.8).
        ssh = self.path("keys", "ssh")
        os.makedirs(ssh, mode=0o700)
        shutil.copy(self.path("keys", "id_ed25519"), os.path.join(ssh, "id_ed25519"))
        os.chmod(os.path.join(ssh, "id_ed25519"), 0o600)
        lines = []
        wrong = []
        keys = {}
        for index, host in enumerate(HOST_NAMES):
            with open(self.path("keys", "hosts", host, "ssh_host_ed25519_key.pub"), encoding="utf-8") as handle:
                kind, blob = handle.read().split()[:2]
            keys[host] = (kind, blob)
            # The key of the host id, not of the address (HostKeyAlias, 4.12).
            lines.append("%s %s %s" % (known_hosts_name(host), kind, blob))
            # A file for the negative test: the key of the next host under the name of this host.
            other = HOST_NAMES[(index + 1) % len(HOST_NAMES)]
            with open(self.path("keys", "hosts", other, "ssh_host_ed25519_key.pub"), encoding="utf-8") as handle:
                okind, oblob = handle.read().split()[:2]
            wrong.append("%s %s %s" % (known_hosts_name(host), okind, oblob))
        with open(os.path.join(ssh, "known_hosts"), "w", encoding="utf-8") as handle:
            handle.write("\n".join(lines) + "\n")
        wrong_dir = self.path("keys", "wrong")
        os.makedirs(wrong_dir, mode=0o700)
        with open(os.path.join(wrong_dir, "known_hosts"), "w", encoding="utf-8") as handle:
            handle.write("\n".join(wrong) + "\n")
        self.known_hosts_keys = keys

    @staticmethod
    def _write_secret(path: str, text: str) -> None:
        """Write a secret file with no line end at the end (6.5) and the mode 0400."""
        with open(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o400), "w", encoding="utf-8") as handle:
            handle.write(text)

    def _make_database_files(self) -> None:
        pg = self.path("pg")
        for who in ("owner", "serve"):
            os.makedirs(os.path.join(pg, who), mode=0o700)
        self.pg_owner = "pupitre_owner"
        self.pg_serve = "pupitre_serve"
        self.pg_passwords = {"owner": secrets.token_hex(16), "serve": secrets.token_hex(16)}
        for who, user in (("owner", self.pg_owner), ("serve", self.pg_serve)):
            self._write_secret(os.path.join(pg, who, "username"), user)
            self._write_secret(os.path.join(pg, who, "password"), self.pg_passwords[who])
        tls = self.path("pg-tls")
        os.makedirs(tls, mode=0o755)
        self._tool("certs", tls, ["/opt/ansible/bin/python3", "/harness/mkcerts.py", "/out", PG_NAME])
        os.makedirs(self.path("pg-ca"), mode=0o755)
        shutil.copy(os.path.join(tls, "ca.crt"), self.path("pg-ca", "ca.crt"))
        os.chmod(os.path.join(tls, "server.key"), 0o644)  # the container copies it and sets the mode 0600 for the database

    # -- the hosts ------------------------------------------------------------------------------------------------

    def start_hosts(self, names=HOST_NAMES) -> None:
        """Start the host containers: systemd and sshd, with a pinned host key each. Wait until sshd runs."""
        for host in names:
            options = [
                "--network", self.network,
                "--network-alias", host_address(host),
                "--hostname", host,
                "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp",
                "--cgroupns", "private",
                "--security-opt", "writable-cgroups=true",
                "--memory", "512m", "--pids-limit", "512",
            ]
            name = self.docker.create(host, self.host_image, options)
            self.docker.cp_in(self.path("keys", "hosts", host, "ssh_host_ed25519_key"), name, "/etc/ssh/acceptance/ssh_host_ed25519_key")
            self.docker.cp_in(self.path("keys", "authorized_keys"), name, "/etc/ssh/acceptance/authorized_keys")
            self.docker.start(name)
            # `docker cp` keeps the owner of the host user. sshd checks the owner of `authorized_keys` (StrictModes).
            self.docker.exec(name, ["sh", "-c", "chown root:root /etc/ssh/acceptance/* && chmod 0644 /etc/ssh/acceptance/authorized_keys"], user="0")
        for host in names:
            name = self.docker.name(host)
            wait_until(lambda n=name: self.docker.exec(n, ["systemctl", "is-active", "ssh"], check=False).out.strip() == "active", 60, what="sshd on %s" % host)

    def stop_hosts(self, names=HOST_NAMES) -> None:
        """Remove the host containers. A scenario group starts again with fresh hosts."""
        for host in names:
            self.docker.cmd(["rm", "--force", "--volumes", self.docker.name(host)], check=False)

    def reset_hosts(self, names=HOST_NAMES) -> None:
        self.stop_hosts(names)
        self.start_hosts(names)

    def host(self, host: str) -> str:
        return self.docker.name(host)

    def host_file(self, host: str, path: str) -> str:
        return self.docker.exec(self.host(host), ["cat", path]).out

    def host_has_file(self, host: str, path: str) -> bool:
        return self.docker.exec(self.host(host), ["test", "-e", path], check=False).code == 0

    def host_ssh_logins(self, host: str) -> int:
        """The count of accepted SSH logins in the journal of a host: a render phase must not change it (4.25)."""
        out = self.docker.exec(self.host(host), ["journalctl", "-u", "ssh", "-o", "cat"], check=False).out
        return sum(1 for line in out.splitlines() if line.startswith("Accepted publickey"))

    # -- the instances --------------------------------------------------------------------------------------------

    def config_text(self, profile: str, fixture_schema_blocks: str = None) -> str:
        """The text of a configuration file: the header of the profile, then the catalog (4.2)."""
        config = os.path.join(self.acc, "config")
        with open(os.path.join(config, "header-%s.hcl" % profile), encoding="utf-8") as handle:
            text = handle.read()
        if profile == "large":
            return text + (fixture_schema_blocks or "")
        with open(os.path.join(config, "catalog.hcl"), encoding="utf-8") as handle:
            return text + handle.read()

    def _mounts(self, profile: str, folder: str, config_file: str, pg_who: str, extra: list) -> list:
        ro = lambda src, dst: ["--volume", "%s:%s:ro" % (src, dst)]
        mounts = []
        mounts += ro(self.path("bin"), "/opt/pupitre")
        mounts += ro(os.path.join(self.acc, "ansible"), "/srv/ansible")
        mounts += ro(self.path("keys", "ssh"), "/run/secrets/ssh")
        mounts += ro(self.path("store-key"), "/run/secrets/pupitre")
        mounts += ro(config_file, "/etc/pupitre/pupitre.hcl")
        mounts += ["--volume", "%s:/var/lib/pupitre" % folder]
        if profile.startswith("distributed"):
            mounts += ro(self.path("pg", pg_who), "/run/secrets/pg")
            mounts += ro(self.path("pg-ca"), "/run/secrets/pg-ca")
        return mounts + extra

    def _options(self, profile: str, folder: str, config_file: str, pg_who: str, extra: list) -> list:
        return (
            ["--network", self.network, "--user", "%d:%d" % (self.uid, self.gid)]
            + ["--tmpfs", "/run/pupitre/secrets:rw,noexec,nosuid,size=64m,mode=0700,uid=%d,gid=%d" % (self.uid, self.gid)]
            + ["--memory", "1g", "--pids-limit", "1024"]
            + self._mounts(profile, folder, config_file, pg_who, extra)
        )

    def _instance_folder(self, name: str) -> str:
        folder = self.path("inst", name)
        for sub, mode in (("data", 0o755), ("secrets", 0o700), ("runs", 0o750), ("runner", 0o755), ("logs", 0o755)):
            os.makedirs(os.path.join(folder, sub), mode=mode, exist_ok=True)
            os.chmod(os.path.join(folder, sub), mode)
        return folder

    def make_instance(self, name: str, profile: str, pg_who: str = "serve", fixture_schema_blocks: str = None, extra: list = None) -> Instance:
        """Make a runner container that holds the files of a profile. It does not start Pupitre."""
        folder = self._instance_folder(name)
        config_file = os.path.join(folder, "pupitre.hcl")
        with open(config_file, "w", encoding="utf-8") as handle:
            handle.write(self.config_text(profile, fixture_schema_blocks))
        options = self._options(profile, folder, config_file, pg_who, extra or [])
        options += ["--init"]
        container = self.docker.run(name, self.runner_image, options, ["sleep", "infinity"])
        inst = Instance(self, name, profile, container, folder)
        self.instances[name] = inst
        return inst

    def run_tool(self, short: str, profile: str, args: list, pg_who: str = "serve", check: bool = True, fixture_schema_blocks: str = None, extra: list = None, prefix: list = None, config_text: str = None):
        """Run one command of the binary in a throwaway container with the files of a profile. Return the result."""
        folder = self._instance_folder(short)
        config_file = os.path.join(folder, "pupitre.hcl")
        with open(config_file, "w", encoding="utf-8") as handle:
            handle.write(config_text if config_text is not None else self.config_text(profile, fixture_schema_blocks))
        options = self._options(profile, folder, config_file, pg_who, extra or [])
        return self.docker.run_once(short, self.runner_image, options, (prefix or []) + ["/opt/pupitre/pupitre"] + args, check=check)

    # -- PostgreSQL -----------------------------------------------------------------------------------------------

    def start_postgres(self) -> None:
        """Start PostgreSQL with TLS in a container. The account `owner` makes the tables: `migrate` runs with it."""
        script = (
            "set -e; cp /acc/tls/server.crt /var/lib/postgresql/server.crt; cp /acc/tls/server.key /var/lib/postgresql/server.key; "
            "chown postgres:postgres /var/lib/postgresql/server.crt /var/lib/postgresql/server.key; "
            "chmod 0600 /var/lib/postgresql/server.key; "
            "exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/var/lib/postgresql/server.crt "
            "-c ssl_key_file=/var/lib/postgresql/server.key -c listen_addresses='*' -c fsync=off"
        )
        options = [
            "--network", self.network,
            "--network-alias", PG_NAME,
            "--tmpfs", "/var/lib/postgresql",
            "--volume", "%s:/acc/tls:ro" % self.path("pg-tls"),
            "--env", "POSTGRES_USER=%s" % self.pg_owner,
            "--env", "POSTGRES_PASSWORD=%s" % self.pg_passwords["owner"],
            "--env", "POSTGRES_DB=pupitre",
            "--memory", "1g", "--pids-limit", "512",
        ]
        self.postgres_container = self.docker.run("postgres", POSTGRES_IMAGE, options, ["sh", "-c", script])
        wait_until(
            lambda: self.docker.exec(self.postgres_container, ["pg_isready", "-h", "127.0.0.1", "-U", self.pg_owner, "-d", "pupitre"], check=False).code == 0,
            90,
            what="PostgreSQL",
        )

    def psql(self, sql: str) -> str:
        return self.docker.exec(
            self.postgres_container,
            ["psql", "-v", "ON_ERROR_STOP=1", "-U", self.pg_owner, "-d", "pupitre", "-At", "-c", sql],
        ).out.strip()

    def grant_serve_account(self) -> None:
        """Make the account of `serve` with the rights of 2.36 only: no right to change a table."""
        self.psql("CREATE ROLE %s LOGIN PASSWORD '%s'" % (self.pg_serve, self.pg_passwords["serve"]))
        self.psql("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO %s" % self.pg_serve)
        self.psql("GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO %s" % self.pg_serve)

    # -- the checks of the lab ------------------------------------------------------------------------------------

    def verify_pinned_host_keys(self, runner: Instance) -> None:
        """Scan each host from the runner container and compare its key with the key that the file pins for its id."""
        scanned = {}
        for host in HOST_NAMES:
            out = runner.exec(["ssh-keyscan", "-t", "ed25519", host_address(host)], check=True).out
            for line in out.splitlines():
                if line and not line.startswith("#"):
                    scanned[host] = tuple(line.split()[1:3])
        if scanned != self.known_hosts_keys:
            raise AssertionError("the host keys differ from the pinned keys:\nscanned %s\npinned %s" % (sorted(scanned.items()), sorted(self.known_hosts_keys.items())))

    # -- the end --------------------------------------------------------------------------------------------------

    def close(self) -> list:
        """Remove each container and network of the run and the temporary folder. Return the list of the problems."""
        problems = self.docker.cleanup()
        try:
            if os.path.isdir(self.work):
                rmtree_force(self.work)
        except OSError as error:
            problems.append("folder %s: %s" % (self.work, error))
        problems += ["left over: " + item for item in self.docker.leftovers()]
        return problems


__all__ = ["Lab", "Instance", "HOST_NAMES", "DOMAIN", "PG_NAME", "PROFILES", "host_address", "known_hosts_name", "DockerError"]
