# Pupitre specification, part 4: runner and commands

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 4. Runner and commands

### 4.1 The two roles of an instance

Each instance serves the API. With `runner.enabled = true`, each instance accepts run requests and writes them in the store with the status `queued` (4.5). The one instance with `runner.execute = true` is the runner node. Only the runner node builds snapshots and starts `ansible-playbook`.

| Backend | Instances | Runner node |
|---|---|---|
| `files` | One | The one instance. `runner.enabled = true` needs `runner.execute = true` (6.14). |
| `postgres` | One or more | The instance that holds the lease `runner` (4.8). A second instance with `runner.execute = true` stops at start. |

With `runner.enabled = false`, no instance runs anything. The run routes give `404 not_found` (3.3), and the file has no block `playbook` and no block `command` (6.14).

A run that waits for a runner node stays `queued`. Pupitre never refuses a run because no runner node is up. The page shows the age of the oldest queued run, and the metric `pupitre_runs_queued` counts them (6.28).

### 4.2 The catalog of capabilities

The configuration file holds the catalog of capabilities. The store never holds a part of it.

| Block | Content | Rule |
|---|---|---|
| `schema "<id>"` | The path of a schema file (2.11) | Read at start on each instance. |
| `playbook "<id>"` | `kind` (`deploy`, `render`, `status` or `command`) and the absolute `path` of a playbook file | Only with `runner.enabled = true`. The runner node reads each file at start and at each run. |
| `command "<id>"` | `label`, `playbook`, `target`, `check` and 0 to 16 blocks `param` (4.32) | Only with `runner.enabled = true`. |

The catalog stays the same for the life of the process. A change needs a restart. Two blocks `playbook` MUST NOT have the same `path` (6.14).

The folder of a playbook MUST NOT hold an entry `group_vars` or `host_vars` (4.12, 7.8). Else the runner node stops at start with the error `playbook "<id>".path`, the expected state and the entry found. Ansible reads these entries next to the playbook, and they win over the variables of Pupitre. Put the variables in a role, or in the playbook, or in a folder that is not next to a playbook.

The runner runs only a playbook of the catalog. The path of each run comes from the block `playbook` of its id, never from a document or a request. A document `unit` names playbook ids (2.9), and a request names a command id. The server checks each id against the catalog.

### 4.3 Kinds and modes of a run

A run has a kind and a mode. The mode `check` adds `--check --diff` to the command line. The mode `apply` runs the playbook with no such flag.

| Kind | Mode | Playbook | Node | Use action (3.12) | Needs a check run |
|---|---|---|---|---|---|
| `deploy` | `check` | `render` of the unit (4.25), then `deploy` of the unit | unit or host | `run.check` | No |
| `deploy` | `apply` | `deploy` of the unit | from the check run | `run.apply` | Yes (4.24) |
| `status` | `apply` | `status` of the unit | unit or host | `run.check` | No |
| `command` | `check` | the playbook of the command | the type of `target` | `command.run` | No |
| `command` | `apply` | the playbook of the command | the type of `target` | `command.run` | Only for a command with `check = true` |

- A status playbook MUST only read the hosts. It runs in the mode `apply` so that a task that reads a state can run. Pupitre cannot check that the playbook only reads (7.11).
- A command with `check = false` accepts the mode `apply` only. A request of the mode `check` for it gives `422 invalid_run_request`.
- A request to cancel a run asks the use action of the kind and the mode of that run.

### 4.4 The run request

`POST /api/runs` (5.16) takes one JSON object. The members depend on the kind and the mode. No other member is valid.

| Kind and mode | Members |
|---|---|
| `deploy`, `check` | `kind`, `mode`, `node`, `hosts`, optional `message` |
| `deploy`, `apply` | `kind`, `mode`, `check_run`, optional `message` |
| `status`, `apply` | `kind`, `mode`, `node`, `hosts`, optional `message` |
| `command`, `check` | `kind`, `mode`, `command`, `node`, `hosts`, `params`, optional `message` |
| `command`, `apply`, with a check | `kind`, `mode`, `check_run`, optional `message` |
| `command`, `apply`, with `check = false` | `kind`, `mode`, `command`, `node`, `hosts`, `params`, optional `message` |

The server checks the request in the order of 3.13. The checks of step 10:

1. `node` is a node path of 3.8 whose type the table of 4.3 permits. For a command, its type equals `target` of the command.
2. The unit of the node is valid in the catalog (2.41). For `deploy`, it has `playbooks.deploy`. For `status`, it has `playbooks.status`. Each named playbook is in the catalog of capabilities with the right kind.
3. `hosts` has 1 or more host ids, in byte order, each one time. For a unit node, each host is in `hosts` of the unit. For a host node, `hosts` is exactly the host of the node. For an environment node, each host has a document `host` in the environment.
4. The count of `hosts` is not above the effective `max_hosts` (3.10).
5. `params` passes the checks of 4.33. It is an object, and it can be empty.
6. `message` is a text of 0 to 500 characters with no control character except U+000A and no format character (2.1). It never reaches Ansible.
7. For a request with `check_run`, the guard of 4.24 holds at the time of the request.

A refusal of rules 1 to 6 gives `422 invalid_run_request`, with one error for each problem. A refusal of rule 7 gives the code of 4.24.

The answer of a success is `201` with the run record (4.38). The run then has the status `queued`, the subject of the request and the time of the instance.

### 4.5 The queue

The queue is the set of runs with the status `queued` in the store. Its order is the order of the run ids, which is the order of time (2.1).

- `create_run` refuses a run when `runner.max_queued_runs` runs are queued (2.27). The server then answers `503 run_queue_full` with `Retry-After: 5`.
- A queued run waits for the runner node and for the lease of its environment (4.7).
- A person can cancel a queued run (4.27).

### 4.6 The loop of the runner node

The runner node repeats these steps. It starts them at once after a run request of its own instance, and else at the period of the poll of its backend (6.10).

1. Read the cancel requests of its runs, and send the signals (4.27).
2. When fewer than `runner.max_parallel_runs` runs are active, call `next_queued` with the environments that it holds.
3. For each run that it gets, take the lease of its environment (4.7). With the lease, change the status to `running` (4.39). Without it, leave the run `queued`.
4. For each run with the status `running`, build the snapshot (4.9 to 4.16) and check the guard again (4.24). Then start the child (4.17 to 4.19) and follow it (4.20 to 4.23).
5. Renew each lease at one third of `runner.lease_ttl`.

A run never starts before it holds the lease of its environment. An error of the store in a step leaves the run in its status. The runner writes the log line `runner.store_error` and tries again at the next period.

### 4.7 The lease of an environment

The lease `environment:<env>` permits one run at a time in an environment, of any kind and mode. So a check never runs at the same time as an apply of the same environment.

- The holder is the text `<node id>/<run id>`. The time to live is `runner.lease_ttl`.
- The runner node renews the lease while the run is `running`. It releases the lease after the end of the run.
- A lease that nobody renews ends after `runner.lease_ttl`. Another run can then start.

### 4.8 The lease of the runner

With the backend `postgres`, the lease `runner` names the runner node.

- At start, an instance with `runner.execute = true` takes the lease `runner` with its node id. When another node holds it, the instance stops at start with exit code 1 and the message `another runner node holds the lease until <time>`.
- The same node id can take the lease again at once. So a restart of the runner node keeps its role.
- The runner node renews the lease at one third of `runner.lease_ttl`. When a renewal gives `Lost`, or when no renewal succeeds for `runner.lease_ttl`, the runner node stops each child (4.27). Then it writes the log line `runner.lease_lost` and the process exits with code 1.

The node id is a UUID that the runner node writes one time in the file `<runner.work_dir>/node-id`, with the mode 0600. It keeps the same id after a restart.

With the backend `files`, the lock of 2.32 makes the one process the runner node. The leases are in memory (2.29).

### 4.9 The snapshot: inputs

The runner builds one snapshot for each run. Its inputs are:

- the documents of the environment, from `read_environment` (2.23);
- the catalog of capabilities (4.2): the schemas, the playbook ids, kinds and paths, and the command;
- the bytes of each playbook file of the run, and of the file `runner.known_hosts_file`;
- the run record: the kind, the node, the hosts and the parameters;
- the secret values of the units of the run (4.13), from `read_secrets` (2.24).

Before it writes a file, the runner checks the inputs. An error stops the run. The errors are a unit of the run that is not valid, a value that fails 2.13, and a broken reference of 2.16. The status is then `failed` with the reason `invalid_values`, and no process starts. The output of the run holds the list of the errors.

### 4.10 The snapshot: the folders

```text
<runner.work_dir>/runs/<run id>/
  snapshot/                          immutable: mode 0500 for the folders and 0400 for the files
    manifest.json                    4.16
    inventory/hosts.yml              4.11
    inventory/host_vars/<host>.yml   4.12, one file for each host of the environment
    vars/run.json                    4.14
    vars/params.yml                  4.14, only for a command
    callback_plugins/pupitre_events.py   4.20
  ansible.cfg                        4.15
  run-vars.json                      4.14
  pid                                4.19
  output.log                         4.19
  events.jsonl                       4.20
  home/  tmp/
<runner.secrets_dir>/<run id>/       4.13, mode 0700
  values/                            4.13
  render/                            4.25
```

The runner writes `inventory/hosts.yml` and `vars/run.json` in the canonical form of 2.4. This JSON text is also YAML. These two files hold no text of a document: they hold ids, names and paths. The files with values of documents or parameters are in the YAML form of 4.12a. The runner changes the modes after the last file, then it computes the hash (4.16). The folder of a run stays until `prune_runs` removes the run (4.31).

### 4.11 The inventory

`inventory/hosts.yml` holds each host of the environment and these groups:

- the group of each schema (2.11) whose units have hosts in the environment, with the hosts of these units;
- one group for each host label (2.8), with the hosts that have the label;
- the group `pupitre_run`, with the hosts of the run.

A group with no host is absent. No label and no group of a schema can be `pupitre_run` (2.1). Example:

```json
{
  "all": {
    "children": {
      "canary": {
        "hosts": {
          "host-a": {}
        }
      },
      "pupitre_run": {
        "hosts": {
          "host-a": {}
        }
      },
      "relay_hosts": {
        "hosts": {
          "host-a": {},
          "host-b": {}
        }
      }
    },
    "hosts": {
      "host-a": {},
      "host-b": {},
      "host-c": {}
    }
  }
}
```

The inventory holds each host of the environment, so a playbook can read the variables of the other hosts. The group `pupitre_run` gives the hosts that the run reaches: the limit of the command line names it (4.17). A host id that is also the name of a label or of a group thus adds no host to the run.

### 4.12 The variables of a host

`inventory/host_vars/<host>.yml` holds, for one host:

| Variable | Content |
|---|---|
| `ansible_host` | The `address` of the document `host` |
| `ansible_port` | The `port` of the document `host`, when it has one |
| `ansible_ssh_common_args` | `-o HostKeyAlias=<environment>.<host>`, with the ids of the environment and of the host |
| `pupitre_environment` | The id of the environment |
| `pupitre_units` | The ids of the units that hold the host, in byte order |
| `<variable>` of each schema | For each unit U that holds the host: the effective values of U on the host (2.13), as one nested object |
| `pupitre_secret_files` | Only for a host of the run, and only when one of its units has a secret. An object keyed by unit id. The value of a unit is an object from each secret key path to the path of its file (4.13). |

The nested object comes from the key paths: each segment is one level. Example: the values `limits.max_clients = 500` and `log.level = "info"` give `{"limits": {"max_clients": 500}, "log": {"level": "info"}}`. A duration stays the text of 2.12. A key with no effective value is absent.

Example of `pupitre_secret_files`, for a host of the units `feed` and `relay`:

```json
{
  "feed": {"api.token": "{{ pupitre_secrets_dir }}/environments/production/api.token"},
  "relay": {"storage.password": "{{ pupitre_secrets_dir }}/environments/production/units/relay/storage.password"}
}
```

A unit lists only the secret keys that have a stored value. The value of the unit scope wins over the value of the environment scope (2.13), so the path names the scope that wins. A unit with no secret has no entry, and the variable is absent when no unit of the host has an entry. A playbook reads the files of its unit with `pupitre_secret_files[<unit id>]`.

The path of a secret file is the text `{{ pupitre_secrets_dir }}/<scope folder>/<name>`. Ansible replaces `pupitre_secrets_dir` with the value of `run-vars.json` (4.14). So the snapshot holds no path that changes with each run.

Inventory variables have a low precedence in Ansible. Ansible also reads the entries `group_vars` and `host_vars` next to the playbook for each run, and they win over Pupitre. A label of a host becomes a group (4.11). So a file `group_vars/production.yml` would reach the run of another environment when an editor gives a host the label `production`. A host id that equals a file of `host_vars` has the same effect.

The generated inventory folder holds only the `host_vars` of Pupitre. The playbook folder is the second place where Ansible looks.

So the runner node refuses a playbook whose folder holds an entry `group_vars` or `host_vars`. It checks this at start and for each snapshot (4.2, 7.8). The entry can be a file, a folder or a symbolic link.

SSH finds the host key of a host by the name `<environment>.<host>`, never by the address (`HostKeyAlias`). The ids have no `.`, so each pair gives one name. The file `runner.known_hosts_file` MUST hold one line for each host that the runner reaches, with this name in place of the address. Example: `production.host-a ssh-ed25519 AAAA...`. The file MUST hold only lines of this form.

The operator MUST remove the lines of a host or an environment when it is deleted. An id that is made again gets the old key (7.11).

A person can change the `address` of a host to the address of another machine. The key of that machine is not the key of the host id, so the connection stops (7.8). This holds also when the other machine is in another environment.

### 4.12a The YAML form of the variable files

Ansible evaluates a template in the value of a variable when a playbook reads it. A string with `{{ lookup('pipe', ...) }}` would run a program on the runner node, also in the mode `check`. The tag `!unsafe` of Ansible makes a value data: Ansible never evaluates a string in it. JSON has no tags, so the files `inventory/host_vars/<host>.yml` and `vars/params.yml` are in this YAML form:

- The layout is the layout of 2.4: the members in byte order, two spaces for each level, one item on each line.
- The object of the variable of each schema (4.12) and the object `pupitre_params` (4.14) have the tag `!unsafe`. Each object and each array in them also has the tag. A string in them has no tag.
- A string writes `"`, the characters, `"`. The escapes are `\"`, `\\`, `\n`, `\r` and `\t`. Each other character outside U+0020 to U+007E is `\xXX`, `\uXXXX` or `\UXXXXXXXX`, with the fewest digits that hold its code and lower-case digits. So each file is ASCII.
- The names of the members are ids of 2.1, segments of key paths, or fixed names. No name can hold a template marker.

Example of the variable of a schema:

```yaml
  "relay_config": !unsafe {
    "banner": "Caf\xe9 {open}",
    "limits": !unsafe {
      "max_clients": 500
    }
  }
```

The tag is on the objects and the arrays, not on the strings. From ansible-core 2.19, a tag on a string makes Ansible resolve its type again. Then the text `"true"` becomes a boolean, and `""` becomes `null`.

The runner node needs ansible-core 2.19 or later, and it refuses an older version at start (7.8). Before 2.19, the tag is lost when a playbook reads a value through `hostvars`. A filter that builds a new string, for example `b64decode`, `from_json`, `join` or `~`, then gives a plain string. `set_fact` stores this string, and each later read evaluates it. Only a version from 2.19 keeps the protection in this path.

The address, `ansible_ssh_common_args`, `pupitre_environment` and `pupitre_units` have no tag. They hold an address of 2.8 or ids of 2.1, and these rules permit no `{`, `%` or `#`. The paths of `pupitre_secret_files` have no tag, because Ansible must evaluate `{{ pupitre_secrets_dir }}` in them. Rule 2.12 and rule 4.33 also refuse the template markers in each text, before a value reaches the store or a run (7.3).

### 4.13 The private folder of the secrets

The runner writes the secrets of a run in `<runner.secrets_dir>/<run id>/`:

- The folder has the mode 0700. Each file has the mode 0600 and the bytes of one value, with no line end added.
- The files are `values/<scope folder>/<name>`. They hold the secrets of each unit that has a host of the run. They also hold the secrets of the environment scope that these units use (2.13).
- The folder `render` is empty at the start. The render phase writes the configuration files of the hosts in it (4.25). These files can hold secrets, so they stay on `tmpfs` with the values.
- `runner.secrets_dir` MUST be on `tmpfs` or `ramfs`. The runner node checks it at start (6.13).
- The runner removes the folder at the end of the run, whatever its status. At start, the runner node removes each folder of a run that it does not adopt (4.29). When the record of a run refuses a write, the runner stops the child and removes the folder. Then it frees the environment.

### 4.14 The variable files

The snapshot holds two variable files. The command line gives them with `--extra-vars @<file>`.

`vars/run.json` holds the facts of the run that never change between a check and its apply:

| Variable | Content |
|---|---|
| `pupitre_kind` | `deploy`, `status` or `command` |
| `pupitre_node` | The node path of the run |
| `pupitre_playbook` | The id of the playbook |
| `pupitre_command` | The id of the command, for a command |
| `pupitre_releases_dir` | The value of `runner.releases_dir`, when the configuration has it |

`vars/params.yml` holds `pupitre_params` for a command, in the YAML form of 4.12a. It has the parameters of the request in the form of 4.34. A duration becomes an integer of milliseconds. The command line never holds a parameter.

`run-vars.json` is outside the snapshot, because it changes with each run:

| Variable | Content |
|---|---|
| `pupitre_run_id` | The run id |
| `pupitre_secrets_dir` | `<runner.secrets_dir>/<run id>/values` |
| `pupitre_render_dir` | `<runner.secrets_dir>/<run id>/render`, an empty folder in the private folder for the render phase (4.13, 4.25) |

### 4.15 The file `ansible.cfg`

The runner writes `<run folder>/ansible.cfg`:

```ini
[defaults]
inventory = <run folder>/snapshot/inventory/hosts.yml
callback_plugins = <run folder>/snapshot/callback_plugins
callbacks_enabled = pupitre_events
retry_files_enabled = False
host_key_checking = True
remote_user = <runner.ssh_user>
private_key_file = <runner.ssh_key_file>
forks = <runner.forks>
local_tmp = <run folder>/tmp

[ssh_connection]
ssh_args = -o UserKnownHostsFile=<runner.known_hosts_file> -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes -o CheckHostIP=no -o IdentitiesOnly=yes -o ForwardAgent=no -o PasswordAuthentication=no
```

The file has no other line. The manifest holds the values of the settings, not the file (4.16). Each host adds `-o HostKeyAlias=<environment>.<host>` through `ansible_ssh_common_args` (4.12). `CheckHostIP=no` stops a second check by the address: the name of the host id is the only key of the check.

### 4.16 The manifest and the hash

`snapshot/manifest.json` describes the snapshot. Example:

```json
{
  "callback_plugin": "<64 hex>",
  "environment": "production",
  "files": {
    "inventory/host_vars/host-a.yml": "<64 hex>",
    "inventory/hosts.yml": "<64 hex>",
    "vars/run.json": "<64 hex>"
  },
  "format": 1,
  "kind": "deploy",
  "playbooks": {
    "deploy": {
      "id": "deploy-relay",
      "path": "/srv/ansible/playbooks/deploy-relay.yml",
      "sha256": "<64 hex>"
    },
    "render": {
      "id": "render-relay",
      "path": "/srv/ansible/playbooks/render-relay.yml",
      "sha256": "<64 hex>"
    }
  },
  "secrets": {
    "environment:production/unit:relay": {
      "storage.password": "<16 hex>"
    }
  },
  "settings": {
    "forks": 5,
    "known_hosts_sha256": "<64 hex>",
    "ssh_key_file": "/run/secrets/ssh/id_ed25519",
    "ssh_user": "deploy"
  }
}
```

- `files` holds the SHA-256 of each file of `snapshot/` except the manifest and the callback plugin.
- `playbooks` holds the playbooks of the run by their role in 4.3: `render`, `deploy`, `status` or `command`. A `deploy` run, check or apply, holds `deploy` and, when the unit has one, `render`. Only a check starts the render playbook (4.25), but the apply holds it too. So a check and its apply have the same hash.
- `secrets` holds the fingerprint of each secret of the private folder (2.26). It never holds a value.
- `settings` holds the values of `ansible.cfg` that are not paths of the run folder.

The snapshot hash is the text `sha256:` and the 64 lower-case hexadecimal characters of the SHA-256 of the canonical bytes of the manifest. The mode, the run id, the times and the subject are not in the manifest. So a check and an apply with the same inputs have the same hash. The run record keeps the hash (4.38).

The hash does not cover the roles and the other files of the playbook folder. A change of a role between a check and its apply does not change the hash. Section 7.11 lists this limit.

### 4.17 The command line

The runner starts the program `runner.ansible_playbook` with these arguments, as separate items, with no shell:

```text
--inventory <run>/snapshot/inventory/hosts.yml
--limit pupitre_run
--extra-vars @<run>/snapshot/vars/run.json
--extra-vars @<run>/snapshot/vars/params.yml       (a command only)
--extra-vars @<run>/run-vars.json
--check --diff                                      (the mode check only)
<path of the playbook>
```

The arguments hold paths, flags and the fixed name of the group `pupitre_run` (4.11). They never hold a value, a parameter or a secret.

### 4.18 The environment of the child

The runner clears the environment of the child, then sets exactly these variables:

| Variable | Value |
|---|---|
| `PATH` | `runner.path` |
| `HOME` | `<run>/home` |
| `LANG`, `LC_ALL` | `C.UTF-8` |
| `ANSIBLE_CONFIG` | `<run>/ansible.cfg` |
| `ANSIBLE_LOCAL_TEMP` | `<run>/tmp` |
| `PYTHONDONTWRITEBYTECODE` | `1` |
| `PUPITRE_EVENTS_FILE` | `<run>/events.jsonl` |
| `PUPITRE_EVENTS_PHASE` | `render` or `main` |

The child gets no variable of the environment of Pupitre. It has no `SSH_AUTH_SOCK`, so no agent forwards a key.

### 4.19 The child process

- The child starts in a new process group, whose id is its pid. Its standard input is `/dev/null`. Its standard output and its standard error go to `<run>/output.log`, opened with `O_APPEND`.
- The runner writes `<run>/pid` at once: a canonical object with `boot_id` (the text of `/proc/sys/kernel/random/boot_id`), `pid`, `phase`, and `start_time` (field 22 of `/proc/<pid>/stat`). The run record gets the same values in `runner` (4.38).
- The runner never kills the child when the process of Pupitre stops. A stop of Pupitre leaves the child alone (4.29).
- The runner stops a child with the signal `SIGTERM` to its process group. After `runner.cancel_grace`, it sends `SIGKILL` to the group.

### 4.20 The callback plugin and the events

The binary holds the callback plugin `pupitre_events` (`crates/pupitre-runner/src/callback/pupitre_events.py`). The runner writes it into the snapshot. The plugin writes one JSON object on each line of the file `PUPITRE_EVENTS_FILE`. It flushes each line. It works with `ansible-core` 2.19 and later (7.8).

| `kind` | Other members |
|---|---|
| `playbook_start` | `playbook` (the file name), `check` (`true` or `false`) |
| `play_start` | `play` (the name of the play) |
| `task_start` | `task` (the name of the task) |
| `host_result` | `host`, `task`, `status` (`ok`, `changed`, `failed`, `skipped` or `unreachable`), optional `diff`, optional `message`, optional `omitted` |
| `stats` | `hosts`: for each host, the integers `changed`, `failed`, `ignored`, `ok`, `rescued`, `skipped` and `unreachable` |

Each line also has `v` with the value 1.

- `diff` is the text of the difference of the task, when the result has one. `message` is the message of a failed or unreachable result. A message that is a list or a dictionary is JSON text, and a text with non-ASCII characters keeps them.
- A task with `no_log` gives no `diff` and no `message`.
- A text of `diff` or `message` above 16,384 bytes is absent, and the line has `omitted` equal to `true`. The plugin never cuts a text.

The runner reads the file while the child runs. It reads each line with the strict reader of 2.5. It adds `seq` (1, 2, 3 and so on), `phase` and `t` (the time of the read) and stores the line (4.22). A line that is not valid, or that is longer than `runner.max_event_bytes`, becomes `{"kind": "invalid", "length": <bytes>}`.

### 4.21 The scrubber

Before it stores output bytes or an event line, the runner replaces each copy of each secret value of the run with the text `[secret]`. It also replaces the JSON string form of each value in the event lines. An event line can hold a value after two layers of escapes. A `diff` is JSON text, and the line writes it as a JSON string again. So the runner also replaces the JSON form of the JSON form. It replaces the Python `repr` form of a value, and its JSON form, for a text that Python made with `str()`.

- The match is exact, on bytes. A value has 8 bytes or more (2.25).
- Copies that overlap, of one value or of two values, become one `[secret]`. So no part of a value stays. Copies that touch with no overlap become one `[secret]` each.
- The runner keeps the last bytes of each output chunk until the next chunk, so that a value across two chunks is also replaced.
- A role can still print a value in another form, for example in base64. The scrubber does not find it. A role MUST use `no_log` for a task that handles a secret (7.11).

### 4.22 The output and the events in the store

The runner adds the output to the store in chunks of 65,536 bytes at most (2.28). It adds a chunk at most one time each second, and at the end of the run. It adds the events in the same way.

- When the stored output of a run reaches `runner.max_log_bytes`, the runner stores no more output. The run record gets `output_truncated` equal to `true`. The full output stays in `<run>/output.log` on the runner node.
- When a run has `runner.max_events` events, the runner stores no more events. The run record gets `events_truncated` equal to `true`.
- The run continues in both cases. The page shows each such mark.

### 4.23 The end of a run

| Cause | Status | `reason` |
|---|---|---|
| The main phase ends with exit code 0 | `passed` | absent |
| The main phase ends with another exit code | `failed` | `exit_code` |
| The render phase ends with an exit code other than 0 | `failed` | `render_failed` |
| A check of 4.9 fails | `failed` | `invalid_values` |
| A secret does not decrypt (2.39) | `failed` | `secret_unreadable` |
| The snapshot cannot be written | `failed` | `snapshot_failed` |
| The guard of 4.24 fails at the start of the run | `failed` | `snapshot_changed` or `check_too_old` |
| The program cannot start | `failed` | `spawn_failed` |
| The run reaches `runner.run_timeout` | `failed` | `timeout` |
| A person cancels the run | `canceled` | absent |
| The runner cannot find the child after a restart | `lost` | `process_gone` |
| Another runner node held the run | `lost` | `node_changed` |
| An adopted child ends with no `stats` event | `failed` | `lost_exit` |
| An adopted child ends with a `stats` event | `passed` when no host has `failed` or `unreachable` above 0, else `failed` with `host_failed` | |

The record also gets `exit_code` when the runner knows it, `stats` from the event `stats`, and `finished_at`.

### 4.24 The guard of an apply

An apply that needs a check (4.3) names the check run in `check_run`. The guard holds when each rule holds:

| Rule | Code of a refusal at the request | Reason of a failure at the start |
|---|---|---|
| The check run exists and the subject can see it (3.14) | `403 forbidden` or `404 not_found` | |
| It has the same kind, the mode `check` and the status `passed` | `409 check_required` | |
| Its `applied_by` is absent | `409 check_used` | |
| Its `finished_at` is not older than `runner.check_max_age` | `409 check_too_old` | `check_too_old` |
| The snapshot of the apply has the hash of the check | | `snapshot_changed` |

- The apply takes the node, the hosts, the command and the parameters of the check run. The request cannot change them.
- `create_run` sets `applied_by` of the check in the same step (2.27). So one check gives one apply at most.
- At the start of the apply, the runner builds the snapshot and compares the two hashes. It also checks the age again: the time in the queue counts. A failure stops the run before the child starts.
- The subject of the apply can differ from the subject of the check. Each one needs its own action.

### 4.25 The render phase

A `deploy` check of a unit with `playbooks.render` has two phases. The render phase runs first:

- It starts the render playbook on the same snapshot, in the mode `apply`, with the same command line except the playbook. `pupitre_render_dir` names an empty folder for the files that the playbook writes. The folder is in the private folder of the run, on `tmpfs`, and it goes at the end of the run (4.13).
- The render playbook writes the configuration files of the service locally and checks them, for example with `--check-config` of the binary of the service. It MUST NOT connect to a host. Pupitre cannot check this rule (7.11).
- An exit code other than 0 ends the run with `render_failed`. The main phase does not start, so no host gets a connection.
- The events of each phase have their `phase`: `render` or `main`.

An apply has no render phase: its check ran it on the same snapshot.

### 4.26 Status runs

A `status` run starts the status playbook of the unit (4.3). Its events show the version and the state of each host, as the playbook prints them. A status run never starts by itself, also not after an apply. The page shows a button for it.

### 4.27 Cancel

`POST /api/runs/{r}/cancel` (5.18):

- A `queued` run gets the status `canceled` at once, with a compare and set of its status.
- A `running` run gets the member `cancel` with the subject and the time. The runner node reads it at its next step. It sends `SIGTERM` to the process group, waits `runner.cancel_grace`, then sends `SIGKILL`. The status then becomes `canceled`.
- A finished run gives `409 run_finished`.

A cancel stops the child at once. The playbook decides what a half run leaves on the hosts. Pupitre decides no rollback.

### 4.28 The timeout

A run that is `running` for longer than `runner.run_timeout` gets the steps of a cancel, with the status `failed` and the reason `timeout`. The time starts when the status becomes `running`, and it covers the two phases.

### 4.29 Adoption after a restart

At start, the runner node reads each run with the status `running` and its node id.

1. It reads `<run>/pid`. It reads `/proc/<pid>/stat` and the boot id.
2. When the boot id and the start time are the same, the child is the same process. The runner adopts it. It takes the lease of the environment again. It reads the output and the events from the offsets that the store holds.
3. Else, the run gets the status `lost` with the reason `process_gone`.

An adopted child is not a child of the new process, so the runner cannot read its exit code. It checks each second that the pid exists with the same start time. At the end, it reads the event `stats` (4.23).

Adoption needs a child that lives after the stop of Pupitre. It needs the same machine and the same process namespace. The service manager must not kill the children, for example with `KillMode=process` of systemd. A restart of a container kills the children, and the runs become `lost`. Section 7.11 lists this limit.

### 4.30 Runs of another runner node

The runner node holds the lease `runner`. So each run with the status `running` and another node id gets the status `lost`, with the reason `node_changed`. The old node stopped its children when it lost the lease (4.8).

### 4.31 Cleanup

- After the end of each run, the runner node calls `prune_runs(runner.runs_keep)` (2.27). It then removes the folder of each run that the store no longer holds.
- At start, it removes each folder of `runner.secrets_dir` that is not the folder of an adopted run.
- The runner node never removes a folder of a run with the status `queued` or `running`.

### 4.32 Commands

A command is a named action of the configuration file. A person launches it by hand on chosen hosts.

```hcl
command "restart-service" {
  label    = "Restart the service"
  playbook = "restart-service"
  target   = "unit"
  check    = false

  param "grace" {
    label        = "Grace period"
    type         = "duration"
    required     = true
    min_duration = "1s"
    max_duration = "5m"
  }
}
```

| Key | Rule |
|---|---|
| `label` | The text of the page: 1 to 64 characters with no control character. |
| `playbook` | The id of a block `playbook` with the kind `command`. Another id stops the start (6.14). |
| `target` | `environment`, `unit` or `host`: the type of the node of a launch. |
| `check` | Necessary. `true`: an apply needs a check of the same snapshot (4.24). `false`: the command runs after the confirmation of the person. |
| `param "<name>"` | 0 to 16 blocks. The name follows 2.1. |

The store holds no command. A person who can launch a command cannot add or change one.

### 4.33 The parameters of a command

| `type` | Keys of the block | JSON of the request | Check | Value in `pupitre_params` |
|---|---|---|---|---|
| `text` | `min`, `max` (characters) | string | The count of Unicode scalar values is in the range. No control character. No template marker `{{`, `{%` or `{#` (2.12). | string |
| `integer` | `min`, `max` | integer | In the range | integer |
| `boolean` | none | `true` or `false` | None | boolean |
| `duration` | `min_duration`, `max_duration` | string | A canonical duration of 2.12, in the range | integer of milliseconds |
| `choice` | `choices`: 1 to 32 texts of 1 to 64 bytes, each one time | string | One of `choices` | string |

Each block also has `label` (1 to 64 characters) and `required` (`true` or `false`). A key of another type is forbidden in the block (6.14). A parameter never holds a secret.

- A parameter that the command does not declare gives an error.
- A parameter with `required = true` MUST be in the request.
- A parameter with `required = false` can be absent. It is then absent from `pupitre_params`. Pupitre adds no default.
- A wrong value gives an error with the name, the expected type and range, and the value. Pupitre never changes a value.

### 4.34 The canonical form of the parameters

The run record keeps `params` as a canonical object (2.4). The members are in byte order. Each value has the JSON of the request column of 4.33. So the same parameters give the same bytes, and a check and its apply have the same `vars/params.yml`.

### 4.35 The flow of a command

1. The person chooses the command, the node and the hosts. The options route and the lists show only the nodes that the subject can see.
2. The person fills the parameters. The page checks each one with the rules of 4.33, and the server checks them again.
3. With `check = true`, the page sends a request of the mode `check`. The person reads the result and the differences. Then the page sends the mode `apply` with `check_run`.
4. With `check = false`, the page asks the person to confirm in place. Then it sends the mode `apply`.
5. The run record keeps the command, the parameters, the subject, the hosts and the result.

### 4.36 What the runner never does

- It never starts a program other than `runner.ansible_playbook`, and never with a playbook outside the catalog.
- It never starts a shell, and it never puts a value on a command line.
- It never turns off the check of the host keys, and it never forwards an SSH agent.
- It never connects to the internet. A role that needs a release file reads it from `pupitre_releases_dir`.
- It never decides a rollback, and it never starts a run by itself.

### 4.37 The interface in the code

The crate `pupitre-runner` gives the server two things. The code of `crates/pupitre-runner/src/lib.rs` gives the exact signatures.

| Item | Role |
|---|---|
| `start_executor(settings, capabilities, store, wake)` | Run the checks of 7.8, take the lease `runner`, adopt the runs, and start the loop of 4.6. It gives a `RunnerHandle`. |
| `Wake::wake()` | Tell the loop that a run is queued or that a cancel waits. The server keeps a copy of `Wake`. |
| `RunnerHandle::stop(grace)` | Stop the loop. It leaves the children alone (4.19). |
| `ProcessControl` | The trait of the process steps: spawn, probe, signal. A test uses a fake. |
| `build_snapshot(input, folders)` | The steps of 4.9 to 4.16. It gives the hash and the secret values for the scrubber. |

### 4.38 The run record

A run record is a canonical object (2.4). No other member is valid.

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `id` | The run id (2.1). |
| `kind`, `mode` | 4.3 |
| `environment` | The environment id |
| `node` | The node path (3.8) |
| `hosts` | The host ids of the run, in byte order |
| `playbook` | The id of the main playbook |
| `command`, `params` | For a command only (4.34) |
| `check_run` | For an apply that needed a check |
| `applied_by` | For a check that an apply consumed |
| `message` | Optional. The message of the request. |
| `subject` | The subject of the request |
| `created_at`, `started_at`, `finished_at` | Times in the form `2026-11-24T10:00:00.000Z`. The last two are absent before the event. |
| `status` | `queued`, `running`, `passed`, `failed`, `canceled` or `lost` |
| `reason` | A code of 4.23, with a status `failed` or `lost` |
| `snapshot` | The snapshot hash, after the build |
| `runner` | `node`, `pid`, `start_time`, `boot_id`, after the start of the child |
| `exit_code` | An integer, when known |
| `stats` | The counts of the event `stats` |
| `cancel` | `by` and `at`, after a cancel request |
| `output_truncated`, `events_truncated` | `true` after 4.22. Absent else. |

### 4.39 The status transitions

```text
queued  --> running --> passed | failed | canceled | lost
queued  --> canceled
```

No other transition is valid. `passed`, `failed`, `canceled` and `lost` are final. `update_run` refuses another change (2.27).
