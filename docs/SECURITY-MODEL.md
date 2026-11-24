# Security model

Pupitre can run playbooks as root on production hosts. Treat it as the most sensitive part of your system. This page tells what Pupitre protects, how, and what it does not protect. To report a vulnerability, read [SECURITY.md](../SECURITY.md). Part 7 of the [specification](07-security-and-limits.md) has the full rules.

## What Pupitre protects

| Asset | Where it is |
| --- | --- |
| The power to change the hosts | The run routes, the runner node and the SSH key |
| The SSH key of the runner | `runner.ssh_key_file`, on the runner node only |
| The secrets of the units | The store and the private folder of a run |
| The key of the store | `storage.secret_key` |
| The credential of Pupitre at the PDP | `access.pdp.token` |
| The values and their history | The store |
| The pinned host keys | `runner.known_hosts_file` |

## Deployment rules

The operator MUST follow these rules. A deployment that breaks one of them is outside the security model.

1. Only the edge or the proxy reaches the main listener. A network rule enforces it.
2. Nothing outside the operator network reaches the ops listener.
3. In the mode `open`, a reverse proxy authenticates each person before the main listener.
4. The runner node is a dedicated machine or container. Only its operators have a shell on it.
5. `runner.secrets_dir` is on `tmpfs`. The configuration file, the SSH key and the key of the store have the mode 0600 or 0400.
6. The playbooks, the roles and the schema files are in a Git repository with a review.
7. The account of `serve` on PostgreSQL has no right to change a table.

## The controls

| Threat | Control |
| --- | --- |
| A person who reaches the page starts a deploy | A grant of `run.apply` for each environment, unit and host. In the mode `open`, `run.apply` is off by default. |
| A person who edits values makes a role run code | The values have closed schemas. No document names a program or a path. No text holds `{{`, `{%` or `{#`. The variable files tag each value of a document `!unsafe`, so Ansible never evaluates it as a template. |
| A person adds a command or a playbook | The commands, the playbooks and the schemas are in the configuration file only. |
| A secret leaks | No route gives a value. The private folder is on `tmpfs`, and Pupitre removes it after the run. The runner scrubs the output and the events. |
| An apply that nobody checked | An apply needs a check of the same snapshot hash, not older than `runner.check_max_age`, used one time. |
| Two applies in one environment at the same time | The lease of the environment |
| The runner node is taken | One runner node. The SSH key has the mode 0600. The host keys are pinned. No agent forwarding. |
| A log shows hidden control codes or HTML | The page shows logs as plain text, with a visible symbol for each control character. |
| A person learns that a unit exists | An unknown node and a forbidden node give the same `403`. |
| A command parameter becomes a shell command | Closed types and ranges. A text parameter holds no template marker. The parameters go in a variable file with the tag `!unsafe`, never on the command line. |
| A person changes the address of a host to a machine of another environment | SSH finds the host key by `<environment>.<host>`, not by the address. The key of the other machine does not match. |
| Two people save one document | The etag and `409 conflict` |
| A page of another site uses the browser of a person | The write guard and the strict policy of the page |
| A grant is revoked | Each write and each run asks the PDP with no kept answer. |
| The PDP is down | Pupitre fails closed with `503 pdp_unavailable`. |
| A copy of the PostgreSQL database leaks | The secrets are encrypted with a key that is not in the database. |

## The host check

An attacker can point a name that he owns to the main listener. The key `server.allowed_hosts` lists the values of `Host` that the listener serves. Another value gets `421 misdirected_request`, for each method. With a loopback listener, the default is the loopback names. With any other address, the key is necessary.

## The write guard

A page of another site must not use the browser of a person to change a document or to start a run. Each `POST`, `PUT` and `DELETE` passes this guard before Pupitre reads the body.

- A request with `Sec-Fetch-Site` needs the value `same-origin` or `none`. Else the answer is `403 cross_site_request`.
- A request with `Origin` and no `Sec-Fetch-Site` needs an `Origin` that equals the header `Host`. `Origin: null` fails.
- A body needs `Content-Type: application/json`. Else the answer is `415 unsupported_media_type`.
- No answer has an `Access-Control-Allow-*` header. Pupitre sets no cookie.

The policy of the page is `default-src 'none'` with `'self'` for the script, style, image, font and connect sources. It has no inline script and no `style` attribute. Each answer also has `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy` and the two `Cross-Origin` policies.

## The runner node

- At start, the runner node checks that the SSH key is a regular file of its user with the mode 0600 or 0400. Another mode stops the start.
- The file `ansible.cfg` of each run sets `StrictHostKeyChecking=yes` with the file of pinned keys only. It sets `ForwardAgent=no` and `IdentitiesOnly=yes`.
- SSH finds the key of each host by the name `<environment>.<host>` (`HostKeyAlias`), not by the address. A new address of a host thus needs no new line, and the key of another machine fails.
- Before the first child starts, the runner node sets `PR_SET_DUMPABLE` to 0. A child cannot read the environment of Pupitre, where a secret of the form `_env` is.
- The child has an explicit environment with no `SSH_AUTH_SOCK`.
- The runner starts no shell. The arguments hold paths and flags only.
- The runner never starts a program other than `runner.ansible_playbook`, and never with a playbook outside the configuration file.

## Logs, metrics and traces

A log line, a metric label and a span attribute never hold a secret, a token, a credential or a value of a document. A log line of a write holds the key, the subject, the etags and the key paths. The output and the events of a run are scrubbed before the store keeps them.

## What Pupitre does not protect

These limits are part of the design. They are not vulnerabilities.

1. **The power of the roles.** A person with `config.write` changes what a role does on a host. Review the roles, not only the grants.
2. **Roles that use values in a shell.** A role that puts a value in a shell with no quotes can run it. A role that evaluates a value as a template on purpose can also run it.
3. **The scrubber.** It finds exact copies of a secret value and their JSON form. Another form, for example base64, stays in the output. Use `no_log` in each task that handles a secret.
4. **Secrets of the backend `files`.** They are in clear text on the disk, with the mode 0600. The history of the backend `files` is as trusted as the folder. A person who can write the folder can write the history.
5. **The hash of a snapshot.** It covers the playbook file, not the roles. A change of a role between a check and its apply is not found.
6. **Variables of the playbook folder.** `group_vars` and `host_vars` next to a playbook win over the inventory of Pupitre.
7. **The render and status playbooks.** Pupitre cannot check that a render playbook connects to no host, or that a status playbook only reads.
8. **Adoption.** A run survives a restart of Pupitre only when its child survives. Else it becomes `lost`, and the hosts can be in a half state.
9. **A lost runner node.** When it loses the database, it stops its children and exits. A child that it cannot stop can still run.
10. **Rules across documents.** Two instances of `postgres` can accept two writes that break a rule together. The catalog then shows the error, and the runs of the unit stop.
11. **Approvals.** No route can wait for an approval. A key with a `confirm` grant cannot use that action through the API.
12. **The mode `open`.** It has no login. Pupitre trusts each request. The proxy is the only guard.
13. **Kept answers.** A revoked grant stays active on a read route for `access.pdp.cache_ttl` at most, on each instance.
14. **The time of a refusal.** The PDP can take another time for an unknown node and for a forbidden node.
15. **A cancel and the run record.** `Store::update_run` compares only the status. A `cancel` that the server writes between the read and the write of the runner can be lost. Cancel the run again.
16. **The fingerprints of the runner.** The runner has no store key. It reads the fingerprints of the secrets with `list_secrets` before and after `read_secrets`. It tries again when they differ. A secret that changes from A to B and back to A between the two lists is not found.
17. **Who adds a playbook or a command.** A person who edits the store cannot add a playbook, a command or a schema. They are in the configuration file of the operator.
18. **The poll of the backend `postgres`.** The poll waits for each open writer with a barrier of the database. A transaction that stays open delays each poll and each load of all instances.
19. **The file `output.log`.** It holds the output of the child before the scrubber, on the disk of the runner node.
20. **The user of the child.** The child has the user of Pupitre. It can read the secrets of the form `_file`, the key of the store and the SSH key. A separate user for the child is the stronger setup.
21. **The file of pinned keys.** A line that gives one key to two names, or a pattern such as `*.host-a`, joins these names.

## Supply chain

Each direct dependency has an exact version and one line with its reason. `deny.toml` permits only the crates.io registry and the repository of the ProcurAuth specification. It forbids OpenSSL, native TLS and the bundled root certificates.

The page has no third-party script. Each GitHub action is pinned to a full commit SHA. The release holds static binaries only. No container image is published.
