# SPDX-License-Identifier: MIT OR Apache-2.0
"""The stages of a run that do not need the server: the checks of the configuration files and the smoke test."""
from . import smoke
from .lab import Lab, PROFILES

VALID = "the configuration is valid"


def check_configs(lab: Lab, report, fixture_blocks: str) -> bool:
    """Run `check-config` and `print-config` of the built binary on each acceptance profile (6.16, 6.17, 6.24).

    The files are read inside a container that has the same mounts as a real run: stage 2 reads the schema files and
    the certificate files (6.13), and the secrets of the file are read from `_file` values (6.5).
    """
    good = True
    for profile in PROFILES:
        name = "config %s: check-config" % profile
        blocks = fixture_blocks if profile == "large" else None
        result = lab.run_tool("cfg-" + profile, profile, ["check-config", "--config", "/etc/pupitre/pupitre.hcl"], check=False, fixture_schema_blocks=blocks, extra=_fixture_mount(lab, profile))
        if result.code == 0 and VALID in result.out:
            report.ok(name, "exit code 0")
        else:
            good = False
            report.fail(name, "exit code %d\n%s%s" % (result.code, result.out, result.err))
            continue
        shown = lab.run_tool("pc-" + profile, profile, ["print-config", "--config", "/etc/pupitre/pupitre.hcl"], check=False, fixture_schema_blocks=blocks, extra=_fixture_mount(lab, profile))
        with open(lab.path("store-key", "store-key"), encoding="utf-8") as handle:
            key = handle.read()
        problems = []
        if shown.code != 0:
            problems.append("print-config exit code %d: %s" % (shown.code, shown.err))
        if key in shown.out or key in shown.err:
            problems.append("print-config shows the store key")
        if "storage.secret_key = <hidden>" not in shown.out:
            problems.append("print-config does not hide storage.secret_key")
        if profile.startswith("distributed") and lab.pg_passwords["serve"] in shown.out + shown.err:
            problems.append("print-config shows the database password")
        if problems:
            good = False
            report.fail("config %s: print-config" % profile, "; ".join(problems))
        else:
            report.ok("config %s: print-config" % profile, "secrets hidden")
    return check_bad_configs(lab, report) and good


def check_bad_configs(lab: Lab, report) -> bool:
    """Prove that the check is not empty: a file with a known defect stops `check-config` with exit code 1 (6.2, 6.14)."""
    base = lab.config_text("local")
    cases = [
        ("an unknown attribute", base.replace("  ops_listen = ", '  bogus_key = 1\n  ops_listen = ', 1), "bogus_key"),
        ("the contradiction C2", base.replace("  execute                  = true", "  execute                  = false", 1), "execute"),
        ("a secret in clear text", base.replace('secret_key_file = "/run/secrets/pupitre/store-key"', 'secret_key = "%s"' % ("ab" * 32), 1), "secret_key"),
        ("a relative path", base.replace('"/srv/ansible/schemas/demo.json"', '"schemas/demo.json"', 1), "path"),
    ]
    good = True
    for index, (label, text, word) in enumerate(cases):
        result = lab.run_tool("bad-%d" % index, "local", ["check-config", "--config", "/etc/pupitre/pupitre.hcl"], check=False, config_text=text)
        shown = result.out + result.err
        if text == base:
            report.fail("config: " + label, "the test did not change the file")
            good = False
        elif result.code == 1 and word in shown and ("ab" * 32) not in shown:
            report.ok("config: %s stops the start" % label, "exit code 1")
        else:
            good = False
            report.fail("config: %s stops the start" % label, "exit code %d\n%s" % (result.code, shown))
    return good


def _fixture_mount(lab: Lab, profile: str) -> list:
    """The schema folder of the large fixture is mounted at /srv/fixture for the profile `large`."""
    if profile != "large":
        return []
    return ["--volume", "%s:/srv/fixture:ro" % lab.path("fixture")]


def run_smoke(lab: Lab, report) -> bool:
    """Start the host containers and run the smoke test of the images, with no Pupitre process."""
    runner = lab.make_instance("smoke", "local", extra=["--volume", "%s:/run/secrets/ssh-wrong:ro" % lab.path("keys", "wrong")])
    lab.start_hosts()
    report.ok("hosts: systemd and sshd run", "host-a, host-b and host-c")
    return smoke.run(lab, runner, report.callback)
