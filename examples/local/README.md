# Pupitre example: the profile local

This example runs Pupitre with the backend `files`, the access mode `open` and one runner node. All data are synthetic. The playbooks write two files in `/tmp/pupitre-example` on the test hosts.

The guide [docs/GETTING-STARTED.md](../../docs/GETTING-STARTED.md) tells each step. This file lists the content.

## Files

| File | Content |
| --- | --- |
| `pupitre.hcl` | The configuration. The root of the example is `/opt/pupitre-example`. |
| `schemas/relay.json` | The schema of the unit `relay`: four keys, one of them a secret. |
| `playbooks/deploy-relay.yml` | The deploy playbook. It writes the configuration file on each host. |
| `playbooks/render-relay.yml` | The render playbook of the check run. It connects to no host. |
| `playbooks/status-relay.yml` | The status playbook. It only reads the hosts. |
| `playbooks/restart-relay.yml` | The playbook of the command `restart-relay`. |
| `inventory.json` | An inventory in the form of `ansible-inventory --list --export`, for the command `import`. |

## Rules of the example

- The values of the unit reach the playbooks in the variable `relay_config`, as one nested object (4.12 of the specification).
- The schema gives a default to each key that is not a secret. Thus each key has a value on each host.
- The example sets `runner.allow_apply_in_open_mode = true` because it has no proxy. The listeners are bound to `127.0.0.1`.
- The example has no host name of a real service. The addresses are in the range `203.0.113.0/24`, which is for documents.
- The file `ssh/known_hosts` is not in the example: the guide makes it. Each line names a host by `production.host-a` or `production.host-b`, not by its address (4.12 of the specification).
- The script `tools/check-examples.sh` runs `pupitre check-config` on `pupitre.hcl`. The test fails when the file is not valid.
