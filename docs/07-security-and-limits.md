# Pupitre specification, part 7: security and limits

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 7. Security and limits

### 7.1 Assets

Pupitre can run playbooks as root on production hosts. Treat it as the most sensitive part of the system.

| Asset | Where |
|---|---|
| The power to change the hosts | The run routes, the runner node, the SSH key |
| The SSH key of the runner | `runner.ssh_key_file`, on the runner node only |
| The secrets of the units | The store (2.24), the private folder of a run (4.13) |
| The key of the store | `storage.secret_key` (2.26) |
| The credential of Pupitre at the PDP | `access.pdp.token` |
| The values and their history | The store |
| The pinned host keys | `runner.known_hosts_file` |

### 7.2 Trust boundaries and deployment rules

The operator MUST follow these rules. A deployment that breaks one of them is outside the security model.

1. Only the edge or the proxy reaches the main listener. A network rule enforces it (P 9.4).
2. Nothing outside the operator network reaches the ops listener. The edge never routes to it.
3. In the mode `open`, a reverse proxy authenticates each person before the main listener. Pupitre then trusts each request.
4. The runner node is a dedicated machine or container. Only its operators have a shell on it.
5. `runner.secrets_dir` is on `tmpfs` (4.13). The configuration file, the SSH key and the key of the store have the mode 0600 or 0400.
6. The playbooks and the roles are in a Git repository with a review. The schema files are in the same repository.
7. The account of `serve` on PostgreSQL has no right to change a table (2.36).

### 7.3 Threats and controls

| Threat | Control | Limit |
|---|---|---|
| A person who reaches the page starts a deploy | The grants of `run.apply` for each environment, unit and host (3.9). In the mode `open`, `run.apply` is off by default (3.22). | In the mode `open`, the proxy is the only guard. |
| A person who edits values makes a role run code | The values have closed schemas (2.11). No document names a program or a path (4.2). The variable files tag each value of a document `!unsafe`, so Ansible never evaluates it as a template (4.12a). The runner node needs ansible-core 2.19 or later, because older versions lose the tag when a playbook reads a value through `hostvars` (7.8). As defence in depth, no text holds `{{`, `{%` or `{#` (2.12). | The refusal of the markers is defence in depth and not a guarantee: a value can hide a marker, for example in base64, and a filter can decode it. The tag `!unsafe` and the version of Ansible are the control. A role that puts a value in a shell with no quotes, or that evaluates a value as a template on purpose, stays unsafe. Review the roles. |
| A person adds a command or a playbook | The commands, the playbooks and the schemas are in the configuration file only (4.2). | The operator of the file has this power. |
| A secret leaks | No route gives a value (5.14). The page shows a fingerprint with a key of the server (2.26). The private folder is on `tmpfs` and removed after the run (4.13). The output and the events are scrubbed (4.21). | The scrubber finds exact values only. A role MUST use `no_log`. |
| An apply that nobody checked | An apply needs a check of the same snapshot hash, at most `runner.check_max_age` old, used one time (4.24). | The check shows the difference. A person must read it. The hash does not cover the roles (4.16). |
| Two applies in one environment at the same time | The lease of the environment (4.7) | A lost run on a dead node can still run until its node stops it (4.8). |
| The runner node is taken | One runner node. The SSH key has the mode 0600. The host keys are pinned. No agent forwarding (7.8). | This node reaches all hosts. Protect it as a key vault. |
| A log shows hidden control codes or HTML | The page shows logs as plain text with visible control characters (5.22). | None known. |
| A person learns that a unit exists | Unknown and forbidden give the same `403` (3.14). The options route lists only what the subject can see (3.16). | The time of a PDP question can differ. |
| A command parameter becomes a shell command | Closed types and ranges (4.33). A text parameter holds no template marker (4.33). The parameters go in a variable file with the tag `!unsafe`, never on the command line (4.12a, 4.17). | A role that puts a value in a shell with no quotes stays unsafe. |
| A person changes the address of a host to the address of a machine of another environment | SSH finds the host key by `<environment>.<host>`, not by the address (4.12, 4.15). The key of the other machine does not match, and the connection stops. | The file of pinned keys must give each name the key of its own machine (7.11). |
| Two people save one document | The etag and `409 conflict` (2.20) | The second person merges by hand. |
| A page of another site uses the browser of a person | The host check and the write guard (7.4) and the strict policy of the page (7.5) | None known. |
| A grant is revoked | Each write and each run asks the PDP with no kept answer (3.17). | A read route can use a kept answer for `access.pdp.cache_ttl`. |
| The PDP is down | Pupitre fails closed with `503 pdp_unavailable` (3.18). | No edit and no run until it answers. |
| Pupitre is down | The hosts keep their services. A run in progress continues (4.19). | No edit and no new run until it is up. |
| A copy of the PostgreSQL database leaks | The secrets are encrypted with a key that is not in the database (2.39). | The documents and the run output are in clear text. |

### 7.4 The host check and the write guard

An attacker can make a name that he owns point to the address of the main listener (a rebound DNS name). The browser of a person then sends requests with the name of the attacker in `Host` and in `Origin`. Those two headers agree, so the cross-site check below passes. The host check stops this attack. It runs first, on each request of the main listener, also a `GET`, before the route and before the token.

- The key `server.allowed_hosts` lists the values of `Host` that the listener serves. An entry is a host name, an IPv4 address or an IPv6 address in brackets, with an optional port. An entry has no wildcard.
- The request needs exactly one header `Host`. Its value must equal an entry, with no difference of case. An entry with no port matches each port. An entry with a port matches a request with the same explicit port.
- A request target in the absolute form must also have an authority that equals an entry.
- Else the answer is `421 misdirected_request`, and no later step runs. A request with a body also closes the connection.
- When the key is absent and `server.listen` is a loopback address, the default is `localhost`, `127.0.0.1`, `[::1]` and the address of `server.listen`. The key is necessary when `server.listen` has any other address, and its absence stops the start (6.13).
- A reverse proxy MUST send the public name in `Host`, and the key MUST list that name. A proxy that rewrites `Host` to the name of the upstream needs that name in the key.
- The ops listener has no host check. It serves three read-only routes and no document (5.20).

A page of another site must not use the browser of a person to change a document or to start a run. Each `POST`, `PUT` and `DELETE` on the main listener also passes the guard below before Pupitre reads the body.

- When the request has `Sec-Fetch-Site`, its value must be `same-origin` or `none`. Else the answer is `403 cross_site_request`.
- When the request has no `Sec-Fetch-Site` but has `Origin`, the host and the port of `Origin` must equal the header `Host`. `Origin: null` fails. Else the answer is `403 cross_site_request`.
- A host without a port has the default port of the scheme of `Origin`: 80 for `http` and 443 for `https`.
- A request with a body must have `Content-Type: application/json`, with an optional parameter `charset=utf-8`. Else the answer is `415 unsupported_media_type`.
- No answer has an `Access-Control-Allow-*` header. Pupitre sets no cookie.

A client that is not a browser sends neither `Sec-Fetch-Site` nor `Origin`. It passes the guard. The token check and the proxy still apply to it.

### 7.5 Security headers

Each answer that Pupitre makes on the main listener has these headers:

| Header | Value |
|---|---|
| `Content-Security-Policy` | `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `Referrer-Policy` | `same-origin` |
| `Cross-Origin-Resource-Policy` | `same-origin` |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=(), payment=(), usb=()` |
| `X-Request-Id` | The id of the request |

The HTTP library answers a request that is not valid HTTP, or a head above 64 KiB, with a bare `400` or `431`. It does this before Pupitre sees the request. The answer has no body and no header of the table, and the connection closes. It holds no content, so the headers protect nothing in it.

The policy permits no inline script and no `style` attribute in the HTML. A script of the page can set a style property through the CSS object model.

### 7.6 Secrets in the configuration

A secret key of the configuration accepts only `_env` and `_file` (6.5). `print-config` and the start log show `<hidden>` for its value (6.20). The secret keys are `storage.secret_key`, `storage.postgres.password` and `access.pdp.token`.

### 7.7 Path safety

- Each path of the configuration is absolute and canonical: no `.` or `..` segment, no repeated `/` and no `/` at the end (6.6).
- Each path that the store or the runner builds comes from ids of 2.1. An id has no `/`, no `.` at the start and no `..`.
- The backend `files` opens each segment with `O_NOFOLLOW` (2.32).
- The runner writes the snapshot, then removes the write bits (4.10). A child can still read it.

### 7.8 The runner node

- At start, the runner node checks that `runner.ssh_key_file` is a regular file of its user with the mode 0600 or 0400. Another mode stops the start.
- It checks that `runner.known_hosts_file` is a regular file with one or more lines. The file `ansible.cfg` of each run sets `StrictHostKeyChecking=yes` and `CheckHostIP=no` with this file only (4.15). Each host gets `HostKeyAlias=<environment>.<host>`, so SSH finds the key by the host id, not by the address (4.12).
- At start, the runner node runs `runner.ansible_playbook --version`. The program starts with no shell, with an explicit environment as in 4.18 and with a private folder in `runner.work_dir`. The runner reads the line `ansible-playbook [core X.Y.Z]`. A version before `core 2.19`, a failure of the program and an output that the runner cannot parse stop the start. The error names the key `runner.ansible_playbook`, the expected range (`core 2.19 or later`) and the version found. Older versions lose the tag `!unsafe` when a playbook reads a value through `hostvars` (4.12a).
- Before the first child starts, the runner node sets `PR_SET_DUMPABLE` to 0 on its process. A child of the same user then cannot read `/proc/<pid>/environ` of Pupitre, where a secret of the form `_env` is (6.5). A child can still read each file that the user of Pupitre can read, for example a secret of the form `_file` (7.11).
- The child has an explicit environment with no `SSH_AUTH_SOCK` (4.18). `ansible.cfg` sets `ForwardAgent=no` and `IdentitiesOnly=yes`.
- At start, the runner node checks the folder of each playbook of the catalog. A folder with an entry `group_vars` or `host_vars` (a file, a folder or a symbolic link) stops the start. The error names the key `playbook "<id>".path`, the expected state and the entry found. The snapshot builder makes the same check for each run. Ansible reads these entries next to the playbook for each run. They win over the inventory of Pupitre (4.12).
- The runner starts no shell. The arguments hold paths and flags only (4.17).
- The runner checks that `runner.secrets_dir` is a folder of its user with the mode 0700 on `tmpfs` or `ramfs` (4.13).

### 7.9 Logs, metrics and traces

- A log line, a metric label and a span attribute never hold a secret, a token, a credential or a value of a document.
- A log line of a write holds the key, the subject, the old and new etag, and the key paths that changed. It never holds a value.
- The output and the events of a run are scrubbed before the store keeps them (4.21). The log of the server never holds the output.
- The error message of a configuration key never holds the value of a secret (6.14).
- A log line and a span show a standard method of HTTP by its name. They show any other method token as `other`, so a client cannot make a long line.

### 7.10 The limits

Each list, queue, cache and wait has a bound. A bound is a key of the configuration, except the bounds of the formats and of the protocol.

| Limit | Key or value | Answer at the bound |
|---|---|---|
| Open connections of the main listener, with the requests that still run after their client closed the connection | `server.max_connections` | `503 server_busy` with `Retry-After: 1`, then close |
| Time to read the request line and the headers | `server.header_read_timeout` | Close the connection |
| Time to read a body | `server.body_read_timeout` | `408 request_timeout` |
| Idle time of a kept connection | `server.idle_timeout` | Close the connection |
| Request line | 8,192 bytes | `414 uri_too_long` |
| Headers | 16,384 bytes and 100 headers | `431 headers_too_large` |
| Body of a request | `server.max_request_bytes` | `413 payload_too_large` |
| Bodies in flight at one time | `server.max_inflight_bodies` | `503 server_busy` with `Retry-After: 1` |
| Items of a list page | `server.max_page_items` | The request gets `400 bad_request` |
| JSON depth of a body | 16 (2.5) | `400 bad_request` |
| Calls to the PDP at one time | `access.pdp.max_connections`, wait `access.pdp.timeout` | `503 pdp_unavailable` |
| Kept answers of the PDP | `access.pdp.cache_entries` | No new answer is kept |
| Questions of one list or options request | `access.options_max_questions` | A page with `next` |
| Items of one options page | `access.options_page_items` | The request gets `400 invalid_options_request` |
| Connections to PostgreSQL | `storage.postgres.max_connections`, wait `storage.postgres.connect_timeout` | `503 store_unavailable` |
| Versions of one document | `storage.<backend>.history_keep` | The oldest versions go |
| Documents of the store | `limits.*` (2.42) | `422 limit_exceeded` |
| Queued runs | `runner.max_queued_runs` | `503 run_queue_full` with `Retry-After: 5` |
| Runs at one time on the runner node | `runner.max_parallel_runs` | The run waits in the queue |
| Hosts of one run | `runner.max_hosts` and the ceiling `max_hosts` | `403 ceiling_exceeded` |
| Time of one run | `runner.run_timeout` | The run fails with `timeout` |
| Stored output of one run | `runner.max_log_bytes` | `output_truncated` (4.22) |
| Events of one run, and bytes of one event | `runner.max_events`, `runner.max_event_bytes` | `events_truncated`, or an event `invalid` (4.20) |
| Finished runs in the store | `runner.runs_keep` | The oldest finished runs go (4.31) |
| Spans that wait for the export | `telemetry.max_queued_spans` | The new span is dropped and counted (6.28) |
| Size of the configuration file and of a `_file` value | 1 MiB and 64 KiB (6.25) | The start stops |
| Size of a schema file | 1 MiB (2.11) | The start stops |

### 7.11 What Pupitre does not protect

These limits are part of the design. They are not vulnerabilities.

1. **The power of the roles.** A person with `config.write` changes what a role does on a host. Her power is the power of the roles. Review the roles, not only the grants.
2. **Roles that use values in a shell.** Pupitre types each value and tags it `!unsafe` (4.12a). A role that puts a value in a shell with no quotes can run it. A role that evaluates a value as a template on purpose, for example with the filter `template` or a lookup, can also run it.
3. **The scrubber.** It finds exact copies of a secret value. It also finds the JSON form, the JSON form of the JSON form, and the Python `repr` form with its JSON form. The second JSON form is the one that an event line holds for a diff of the plugin. Another form, for example base64, stays in the output. Use `no_log` in each task that handles a secret.
4. **Secrets of the backend `files`.** They are in clear text on the disk, with the mode 0600, outside the documents folder. The history of the backend `files` is as trusted as the folder. A person who can write the folder can write the history. This includes the files of `.history`: a forged version can change `saved_by` and the time, hide an external change, or make `repair` write the document.
5. **The hash of a snapshot.** It covers the playbook file, not the roles and the other files of the playbook folder. A change of a role between a check and its apply is not found.
6. **Variables of the playbook folder.** The runner node refuses a playbook folder with an entry `group_vars` or `host_vars` (7.8). Other variable files that a role or a playbook loads, for example with `include_vars` or `vars_files`, are not checked. They can win over the inventory of Pupitre.
7. **The render and status playbooks.** Pupitre cannot check that a render playbook connects to no host, or that a status playbook only reads.
8. **Adoption.** A run survives a restart of Pupitre only when its child survives (4.29). Else it becomes `lost`, and the hosts can be in a half state.
9. **Lost runner node.** When the runner node loses the database, it stops its children and exits (4.8). A child that it cannot stop can still run.
10. **Rules across documents.** Two instances of `postgres` can accept two writes that break a rule of 2.16 together. The catalog then shows the error, and the runs of the unit stop.
11. **Approvals.** No route can wait for an approval (3.19). A key with a grant that has `confirm` cannot use that action through the API.
12. **The mode `open`.** It has no login. Pupitre trusts each request, and the subject is always `-`. The proxy is the only guard.
13. **Kept answers.** A revoked grant stays active on a read route for `access.pdp.cache_ttl` at most, on each instance.
14. **The time of a refusal.** The PDP can take another time for an unknown node and for a forbidden node.
15. **A cancel and the run record.** `Store::update_run` compares only the status (2.27). A `cancel` that the server writes between the read and the write of the runner can be lost. The runner reads the record again before each write, so the window is small. Cancel the run again.
16. **The fingerprints of the runner.** The runner has no store key. It reads the fingerprints of the secrets with `list_secrets` before and after `read_secrets`. It tries again 3 times when the two lists differ, then the run ends with `snapshot_failed`. A secret that changes from A to B and back to A between the two lists gives two equal lists. The runner then reads B, or A, under the fingerprint of A. The store has no version counter of a secret to find this change.
17. **Who adds a playbook or a command.** A person who edits the store cannot add a playbook, a command or a schema. They are in the configuration file of the operator (4.2).
18. **The poll of the backend `postgres`.** The poll uses a barrier of the database. It waits for each open writer (2.38). A transaction that stays open delays each poll and each load of all instances, up to `storage.postgres.statement_timeout`. A person with the account of `serve` can hold the barrier open and so stop the update of the catalogs. The barrier does not cover a row that a person writes with SQL and no barrier. The restart of the instance reads such a row.
19. **The file `output.log`.** It holds the output of the child before the scrubber, on the disk of the runner node, in the work folder (4.19). Only the operators of the node can read it.
20. **The mode `sslmode = "disable"`.** The connection to PostgreSQL has no TLS. A person on the network path can read the password of the database account and the data. Use `verify-full` for each database that is not on the same host.
21. **The files of events of the backend `files`.** A read of a page of events reads the whole file `events.jsonl` of the run. A file above 1 GiB gives an error at open and at read. The store never cuts it.
22. **Secrets in memory.** The type of a secret value and the type of the store key overwrite their bytes when the program frees them. This is a best effort. Copies in the buffers of a library, in the swap and in a core dump stay. Disable core dumps and swap on the runner node.
23. **The raw diff on the work disk.** A check run uses `--diff`, so the output shows the difference of each rendered file. The file `output.log` and the file `events.jsonl` of the run keep this diff as Ansible wrote it. The scrubber works on the bytes that go to the store, not on these two files. A rendered file can hold a secret, so these two files can hold it. They stay on the work disk of the runner node until the prune (4.31). Only the operators of the node can read them.
24. **The ops listener has no check of `Host`.** The host check of 7.4 covers the main listener only. An attacker with a rebound DNS name can use a browser that reaches the ops listener to read `/metrics`. Put the ops listener on a network that no browser reaches (5.20).
25. **A secret that holds a template marker.** A secret value can hold `{{`, `{%` or `{#` (2.12). It reaches Ansible as the content of a file. A playbook that uses a secret file as the source of `template`, or that loads it with `include_vars`, evaluates the markers. The operator MUST NOT do this.
26. **A label that equals a host id.** A label becomes a group of the inventory (4.11). When a host and a group have the same name, Ansible takes the host. A pattern `hosts: <name>` then reaches the host and not the hosts of the group. A playbook that uses a label in `hosts:` can miss hosts. The limit of a run names the group `pupitre_run`, so a run is not wider (4.17). Do not give a host a label that is the id of another host.
27. **The names in `runner.known_hosts_file`.** The alias `<environment>.<host>` is a valid DNS name. A line of another origin, for example `db.internal`, matches the environment `db` and the host `internal`. A deleted environment or host that is made again with the same id gets the pinned key of the old one. The file MUST hold only lines of the form `<environment>.<host>`. The operator MUST remove the lines of a host or an environment when it is deleted. Pupitre does not check the lines, because SSH also accepts hashed names and patterns.
28. **A panic.** The release build uses `panic = "abort"`. A panic in a handler stops the process, and it gives no `500` (5.4). A supervisor MUST restart Pupitre. A run in progress continues (4.19).
23. **The user of the child.** The child of a run has the user of Pupitre. It can read each file that Pupitre can read: the secrets of the form `_file`, the key of the store and the SSH key. A role that the operator does not trust can thus read them. A separate user for the child, with access to the SSH key and the run folders only, is the stronger setup. Pupitre does not make it.
24. **The file of pinned keys.** The check of 4.12 is as strong as the file. A line that gives one key to two names, or a pattern such as `*.host-a`, joins these names. A host can then reach each machine with that key.

### 7.12 Supply chain

- Each direct dependency has an exact version, `default-features = false` where the crate permits it, and one line with its reason in `Cargo.toml` (8.9).
- `deny.toml` permits only the crates.io registry and the repository `ProcurAuth/procurauth`. It forbids OpenSSL, native TLS, `aws-lc` and the bundled root certificates.
- The ProcurAuth crates are git dependencies at one full commit.
- The page has no third-party script. The binary embeds each static file.
- Each GitHub Action is pinned to a full commit SHA with its version in a comment.
