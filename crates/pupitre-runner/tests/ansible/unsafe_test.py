# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check the tag !unsafe of the variable files of a snapshot with a real ansible-playbook (4.12, 7.3).

Use: python3 -I unsafe_test.py <folder> [control]

With the word `control`, the script runs the case of hostvars only and expects the defect: it is a control run on
an ansible-core that Pupitre does not support (before 2.19), and it proves that the case can find the defect.

The folder holds the files that the unit test `the_files_of_the_test_with_a_real_ansible` of the runner
writes: host-a.yml (a file inventory/host_vars), params.yml (the file vars/params.yml) and expected.json.
The values hold lookups of the plugin `pipe` that make a file in /tmp/pupitre-unsafe. The checks of 2.12
refuse such values, so the test covers the tag !unsafe alone.

The script checks that:
  - the loader of Ansible, the Python loader of PyYAML and its C loader read each file and give the values of
    expected.json: no escape changes a character;
  - a run in the mode check and a run in the mode apply evaluate no lookup, and write the literal texts;
  - a text such as "true", "" or "0x10" in a collection with the tag stays a text;
  - a plain string that Pupitre builds (a path of pupitre_secret_files) is still evaluated;
  - the same values in the old form, JSON with no tag, make the files: the test can find the defect;
  - a value that a playbook reads through hostvars, decodes with b64decode and stores with set_fact is not
    evaluated (7.3). Before ansible-core 2.19 the tag is lost in this path, so Pupitre requires 2.19 or later (7.8).
"""

import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile

import yaml

SOURCE = sys.argv[1]
CONTROL = len(sys.argv) > 2 and sys.argv[2] == "control"
MARKS = "/tmp/pupitre-unsafe"
MARK_NAMES = ("M-value", "M-list", "M-nested", "M-param", "M-hostvars")

# A host value that holds the text of a template in base64. A filter that builds a new string from it gives
# a plain string, so the tag !unsafe of the source does not protect the result on ansible-core before 2.19.
HOSTVARS_SECRET = "{{ lookup('pipe', 'touch /tmp/pupitre-unsafe/M-hostvars') }}"

HOSTVARS_PLAYBOOK = """\
- name: Read a value through hostvars
  hosts: all
  connection: local
  gather_facts: false
  tasks:
    - name: Decode the value and store it
      ansible.builtin.set_fact:
        decoded: "{{ hostvars[groups['all'][0]].hv_cfg.cert_b64 | b64decode }}"

    - name: Use the stored value
      ansible.builtin.debug:
        msg: "{{ decoded }}"
"""

PLAYBOOK = """\
- name: Use the values of the snapshot
  hosts: all
  connection: local
  gather_facts: false
  tasks:
    - name: Show the values
      ansible.builtin.debug:
        msg: "{{ demo_config.banner }} {{ demo_config.names | join(' ') }} {{ demo_config.nested.deep }} {{ pupitre_params.reason }}"

    - name: Write the values
      ansible.builtin.copy:
        dest: "{{ out_file }}"
        mode: "0600"
        content: "{{ demo_config.banner }}\\n{{ demo_config.names[1] }}\\n{{ demo_config.nested.deep }}\\n{{ pupitre_params.reason }}\\n{{ demo_config.cases | join('|') }}\\n"

    - name: Write the path of a secret file
      ansible.builtin.copy:
        dest: "{{ out_file }}.path"
        mode: "0600"
        content: "{{ pupitre_secret_files.demo['api.token'] }}"

    - name: Show the address and the environment
      ansible.builtin.debug:
        msg: "{{ ansible_host }} {{ pupitre_environment }} {{ pupitre_units | join(',') }}"
"""


def fail(text):
    print("unsafe test: FAILED: %s" % text, file=sys.stderr)
    sys.exit(1)


def check(condition, text):
    if not condition:
        fail(text)


def unsafe_loader(base):
    """A PyYAML loader that reads the tag !unsafe as a plain value, as the constructor of Ansible does."""

    class Loader(base):
        pass

    def construct(loader, node):
        if isinstance(node, yaml.ScalarNode):
            return loader.construct_scalar(node)
        if isinstance(node, yaml.SequenceNode):
            return loader.construct_sequence(node, deep=True)
        return loader.construct_mapping(node, deep=True)

    Loader.add_constructor("!unsafe", construct)
    return Loader


def check_loaders(expected):
    """Each loader gives the values of expected.json, character for character."""
    from ansible.parsing.dataloader import DataLoader

    loaders = [("PyYAML Python", unsafe_loader(yaml.SafeLoader))]
    if hasattr(yaml, "CSafeLoader"):
        loaders.append(("PyYAML C", unsafe_loader(yaml.CSafeLoader)))
    for name, key in (("host-a.yml", "host_vars"), ("params.yml", "params")):
        path = os.path.join(SOURCE, name)
        with open(path, "rb") as handle:
            raw = handle.read()
        check(raw.isascii(), "%s is not ASCII" % name)
        text = raw.decode("ascii")
        for label, loader in loaders:
            check(yaml.load(text, Loader=loader) == expected[key], "%s reads %s with another value" % (label, name))
        loaded = DataLoader().load_from_file(path)
        check(loaded == expected[key], "the loader of Ansible reads %s with another value" % name)
        if key == "host_vars":
            # Each item one by one, so that a failure names the item.
            for index, (got, want) in enumerate(zip(loaded["demo_config"]["cases"], expected[key]["demo_config"]["cases"])):
                check(got == want, "item %d of cases: %r is not %r" % (index, got[:80], want[:80]))
    return [label for label, _ in loaders] + ["Ansible"]


def write_snapshot(work, host_vars_bytes, params_bytes):
    os.makedirs(os.path.join(work, "inventory", "host_vars"))
    os.makedirs(os.path.join(work, "vars"))
    os.makedirs(os.path.join(work, "tmp"))
    with open(os.path.join(work, "inventory", "hosts.yml"), "w") as handle:
        handle.write('{\n  "all": {\n    "hosts": {\n      "host-a": {}\n    }\n  }\n}\n')
    with open(os.path.join(work, "inventory", "host_vars", "host-a.yml"), "wb") as handle:
        handle.write(host_vars_bytes)
    with open(os.path.join(work, "vars", "params.yml"), "wb") as handle:
        handle.write(params_bytes)
    with open(os.path.join(work, "playbook.yml"), "w") as handle:
        handle.write(PLAYBOOK)
    with open(os.path.join(work, "ansible.cfg"), "w") as handle:
        handle.write("[defaults]\nretry_files_enabled = False\nlocal_tmp = %s/tmp\n" % work)


def run(work, check_mode, playbook="playbook.yml"):
    """Run ansible-playbook as 4.17 does: the inventory and the variable files of the snapshot."""
    command = [
        "ansible-playbook",
        "--inventory", os.path.join(work, "inventory", "hosts.yml"),
        "--extra-vars", "@" + os.path.join(work, "vars", "params.yml"),
        "--extra-vars", json.dumps({
            "out_file": os.path.join(work, "rendered.txt"),
            "pupitre_secrets_dir": "/srv/pupitre-secrets/run-1/values",
            "ansible_python_interpreter": sys.executable,
        }),
    ]
    if check_mode:
        command += ["--check", "--diff"]
    command.append(os.path.join(work, playbook))
    env = {
        "PATH": os.environ["PATH"],
        "HOME": work,
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "ANSIBLE_CONFIG": os.path.join(work, "ansible.cfg"),
        "ANSIBLE_LOCAL_TEMP": os.path.join(work, "tmp"),
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    return subprocess.run(command, cwd=work, env=env, capture_output=True, text=True, check=False)


def clear_marks():
    shutil.rmtree(MARKS, ignore_errors=True)
    os.makedirs(MARKS)


def made_marks():
    return sorted(name for name in MARK_NAMES if os.path.exists(os.path.join(MARKS, name)))


def hostvars_case():
    """Run the playbook that reads a value through hostvars. Return the marks and the output."""
    work = tempfile.mkdtemp(prefix="pupitre-hostvars-")
    cert = base64.b64encode(HOSTVARS_SECRET.encode()).decode()
    # The form of the runner: the tag on the object, plain strings inside.
    host_vars = ('hv_cfg: !unsafe {"cert_b64": "%s"}\n' % cert).encode()
    write_snapshot(work, host_vars, b"pupitre_params: {}\n")
    with open(os.path.join(work, "hostvars.yml"), "w") as handle:
        handle.write(HOSTVARS_PLAYBOOK)
    clear_marks()
    done = run(work, check_mode=True, playbook="hostvars.yml")
    output = done.stdout + done.stderr
    found = made_marks()
    shutil.rmtree(work)
    return done.returncode, found, output


def version_line():
    return subprocess.run(["ansible-playbook", "--version"], capture_output=True, text=True, check=False).stdout.splitlines()[0]


def main_control():
    """The control run: before ansible-core 2.19 the defect exists, so the case must find it."""
    code, found, output = hostvars_case()
    check("M-hostvars" in found, "the control did not find the defect of hostvars (found %s, code %d): the case cannot find it\n%s" % (found, code, output[-2000:]))
    shutil.rmtree(MARKS, ignore_errors=True)
    print("unsafe test (control): ok (%s; hostvars + b64decode + set_fact ran the lookup: Pupitre refuses this version)" % version_line())


def main():
    if CONTROL:
        main_control()
        return
    with open(os.path.join(SOURCE, "expected.json"), encoding="utf-8") as handle:
        expected = json.load(handle)
    with open(os.path.join(SOURCE, "host-a.yml"), "rb") as handle:
        host_vars_bytes = handle.read()
    with open(os.path.join(SOURCE, "params.yml"), "rb") as handle:
        params_bytes = handle.read()
    readers = check_loaders(expected)

    # The form of the runner: a check, then an apply. No lookup runs.
    work = tempfile.mkdtemp(prefix="pupitre-unsafe-")
    write_snapshot(work, host_vars_bytes, params_bytes)
    clear_marks()
    done = run(work, check_mode=True)
    output = done.stdout + done.stderr
    check(done.returncode == 0, "the check ended with %d:\n%s" % (done.returncode, output[-3000:]))
    check(made_marks() == [], "the check evaluated a value with the tag !unsafe: %s" % made_marks())
    check("touch /tmp/pupitre-unsafe/M-value" in output, "the check does not show the literal value:\n%s" % output[-3000:])
    check(not os.path.exists(os.path.join(work, "rendered.txt")), "the check wrote a file")

    done = run(work, check_mode=False)
    output = done.stdout + done.stderr
    check(done.returncode == 0, "the apply ended with %d:\n%s" % (done.returncode, output[-3000:]))
    check(made_marks() == [], "the apply evaluated a value with the tag !unsafe: %s" % made_marks())
    with open(os.path.join(work, "rendered.txt"), encoding="utf-8") as handle:
        rendered = handle.read().split("\n")
    demo, params = expected["host_vars"]["demo_config"], expected["params"]["pupitre_params"]
    check(rendered[0] == demo["banner"], "the rendered banner is %r" % rendered[0])
    check(rendered[1] == demo["names"][1], "the rendered list item is %r" % rendered[1])
    check(rendered[2] == demo["nested"]["deep"], "the rendered nested value is %r" % rendered[2])
    check(rendered[3] == params["reason"], "the rendered parameter is %r" % rendered[3])
    check("|{{ 7 * 6 }}|" in "\n".join(rendered), "a case with a template is not literal")
    with open(os.path.join(work, "rendered.txt.path"), encoding="utf-8") as handle:
        path = handle.read()
    check(path == "/srv/pupitre-secrets/run-1/values/environments/production/api.token", "the path of the secret file is %r" % path)
    shutil.rmtree(work)

    # The old form: the same values as JSON with no tag. Ansible evaluates them: the check finds the defect.
    control = tempfile.mkdtemp(prefix="pupitre-unsafe-control-")
    old_vars = dict(expected["host_vars"])
    old_vars["demo_config"] = {k: v for k, v in old_vars["demo_config"].items() if k != "cases"}
    write_snapshot(control, json.dumps(old_vars).encode(), json.dumps(expected["params"]).encode())
    clear_marks()
    done = run(control, check_mode=True)
    found = made_marks()
    check({"M-value", "M-param"} <= set(found), "the old form did not run the lookups (found %s): the test cannot find the defect\n%s" % (found, (done.stdout + done.stderr)[-2000:]))
    shutil.rmtree(control)

    # A value read through hostvars, decoded and stored with set_fact: no lookup runs (7.3, 7.8).
    code, marked, output = hostvars_case()
    check(code == 0, "the hostvars case ended with %d:\n%s" % (code, output[-3000:]))
    check(marked == [], "a value read through hostvars was evaluated after b64decode and set_fact: %s\n%s" % (marked, output[-2000:]))
    check("lookup('pipe'" in output, "the hostvars case does not show the literal value:\n%s" % output[-2000:])
    shutil.rmtree(MARKS, ignore_errors=True)

    version = version_line()
    print("unsafe test: ok (%s; readers: %s; the old form ran %s)" % (version, ", ".join(readers), ", ".join(found)))


if __name__ == "__main__":
    main()
