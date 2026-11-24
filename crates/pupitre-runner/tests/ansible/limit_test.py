# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check the limit of a run with a real ansible-playbook (4.11, 4.17).

Use: python3 -I limit_test.py

The inventory has the host `web` and the label `web` on the host `db`. A run of the host `web` only must not
reach `db`. The limit of the runner names the group `pupitre_run`, which holds the hosts of the run. The script
also prints what the old limit, a file with the host ids, reaches. ansible-core 2.19 to 2.21 take the host when
a host and a group have the same name, so the old limit also gave `web` only: the group removes the dependence
on this rule of Ansible.
"""

import json
import os
import subprocess
import sys
import tempfile

INVENTORY = {
    "all": {
        "children": {
            "pupitre_run": {"hosts": {"web": {}}},
            "web": {"hosts": {"db": {}}},
        },
        "hosts": {"db": {}, "web": {}},
    }
}

PLAYBOOK = """\
- name: Each host
  hosts: all
  connection: local
  gather_facts: false
  tasks:
    - name: Nothing
      ansible.builtin.debug:
        msg: hello
"""


def hosts_of(work, limit):
    done = subprocess.run(
        ["ansible-playbook", "--inventory", os.path.join(work, "hosts.yml"), "--limit", limit, "--list-hosts", os.path.join(work, "site.yml")],
        cwd=work,
        env={"PATH": os.environ["PATH"], "HOME": work, "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "ANSIBLE_LOCAL_TEMP": os.path.join(work, "tmp")},
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        print("limit test: FAILED: ansible-playbook ended with %d:\n%s" % (done.returncode, done.stdout + done.stderr), file=sys.stderr)
        sys.exit(1)
    lines = done.stdout.splitlines()
    start = next(i for i, line in enumerate(lines) if "hosts (" in line)
    return sorted(line.strip() for line in lines[start + 1:] if line.strip())


def main():
    work = tempfile.mkdtemp(prefix="pupitre-limit-")
    os.makedirs(os.path.join(work, "tmp"))
    with open(os.path.join(work, "hosts.yml"), "w") as handle:
        json.dump(INVENTORY, handle)
    with open(os.path.join(work, "site.yml"), "w") as handle:
        handle.write(PLAYBOOK)
    with open(os.path.join(work, "limit.txt"), "w") as handle:
        handle.write("web\n")
    new = hosts_of(work, "pupitre_run")
    old = hosts_of(work, "@" + os.path.join(work, "limit.txt"))
    if new != ["web"]:
        print("limit test: FAILED: the group pupitre_run reaches %s" % new, file=sys.stderr)
        sys.exit(1)
    print("limit test: ok (the group reaches %s; the old limit reaches %s)" % (new, old))


if __name__ == "__main__":
    main()
