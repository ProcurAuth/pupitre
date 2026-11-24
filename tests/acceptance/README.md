# Acceptance tests

This folder holds the acceptance tests of section 8.12 of the specification. They run a real `ansible-playbook` in a container against throwaway host containers. Docker runs everything. The host needs Docker and `python3` only.

## What the tests do

The levels are Acceptance and Two instances. The synthetic fixtures hold no real name, address or secret.

| Part | Where | Content |
|---|---|---|
| Images | `images/` | A host image with systemd and `sshd`, and a runner image with a pinned `ansible-core`. Both bases are pinned by digest. |
| Ansible files | `ansible/` | One synthetic role, the playbooks `deploy`, `render`, `status` and four `command` playbooks, and the schema `demo`. |
| Configuration | `config/` | A header for each profile, and one shared catalog of capabilities. |
| Harness | `harness/` | The lab (keys, hosts, instances, PostgreSQL), the smoke test, the API client and the Docker wrapper. |
| Scenarios | `scenarios/` | The scenarios of the profile local, of two instances, and of the large fixture. |
| Unit tests | `unit/` | The tests of the generator, the client and the Docker wrapper. They need no binary. |

## How to run

Run the script from the root of the repository:

```sh
tests/ci/acceptance.sh                       # build the binary, then run all stages
tests/ci/acceptance.sh --bin /path/pupitre   # use a binary that exists
tests/ci/acceptance.sh --stage smoke         # one stage: images, configs, smoke or scenarios
tests/ci/acceptance.sh --scenario cancel     # one scenario
tests/ci/acceptance.sh --require-server      # fail when `pupitre serve` is a stub
tests/ci/acceptance.sh --list                # list the scenarios
```

The stage `smoke` does not start Pupitre. It runs `ansible-playbook` with files that the harness writes by hand, in the form of 4.10 to 4.15. It proves that the images, the host keys and the synthetic playbooks work. It also proves that a lookup in a value with the tag `!unsafe` does not run. A host with the address of another host stops at the check of the host key.

The file of pinned host keys of the harness names each host with the pattern `*.<host>`, for example `*.host-a`. Each scenario makes its own environment, and the pattern holds in each of them. A real file names each environment: `production.host-a`.

The stage `scenarios` needs `pupitre serve`. While `serve` is a stub, the stage is skipped with a message. After the merge of the server, use `--require-server`, so that a stub fails the run.

## Use the runner image in another test

The runner image has Python 3, OpenSSH and `ansible-core` 2.20.11 with its dependencies at exact versions (`images/ansible-constraints.txt`). Build it with the user id of the host user, so that mounted files have the owner of the process:

```sh
docker build --build-arg RUNNER_UID="$(id -u)" --tag pupitre-acc-runner:local \
  --file tests/acceptance/images/Dockerfile.ansible tests/acceptance/images
```

## What a run leaves

A run makes containers and one internal network with the prefix `pupitre-acc-<run>-`. It removes them at the end, also after an error. It then checks that no container, volume or network of the run is left, and it reports the result. The two local images stay as a build cache. Use `--remove-images` to remove them. The run never pushes an image and never connects to a real server.

If the script is killed, use `tests/ci/acceptance.sh --cleanup <run>`. The command removes the resources that carry the label of that run only.

## The large fixture

`tests/fixtures/generate.py` makes the large fixture from a seed. The same seed gives the same bytes. The fixture has one environment with 1,000 units, 1,000 hosts and 100 hosts in each unit, and 64 small environments. Each host is in 100 units, so the fixture has 100 schemas (2.9). The 65 environments are more than the default `limits.max_environments`. The profile `large` sets the limit to 100.

## Choices of the harness

- The access mode of every profile is `open`. The acceptance tests have no PDP and no edge. The series U of the ProcurAuth suite tests the mode `authzen`.
- `runner.check_max_age` is `1m`, the lowest value, so that a test can wait for `check_too_old`.
- The host containers start with `--security-opt writable-cgroups=true` and a private cgroup namespace. They are not privileged.
- The network of the lab is internal. No container can reach another network.
