# Operations

This page tells how to run Pupitre in production. It covers the service, the network, the probes, the logs, the backup and the upgrade. It also lists the steps to find a problem. The deployment rules of 7.2 of the [specification](07-security-and-limits.md) are part of the security model. Follow them.

## Deploy

A deployment has these parts:

- the binary `pupitre` and the configuration file, with the mode 0600;
- the store: three folders for the backend `files`, or a PostgreSQL database;
- the key of the store, in a file or an environment variable that the configuration names;
- on the runner node: `ansible-playbook` of ansible-core 2.19 or later (the start checks the version);
- on the runner node: the SSH key, the file of pinned host keys, the work folder and a folder on `tmpfs` for the secrets;
- a reverse proxy or an edge in front of the main listener.

Make the store key with `openssl rand -hex 32`. All instances of one store MUST have the same key. Keep a copy in a safe place. A copy of the database without the key gives no secret value, and a different key stops the start.

The runner node is a dedicated machine or container. Only its operators have a shell on it. It reaches each managed host, so protect it as a key vault.

The file of pinned host keys names each host `<environment>.<host>`, not by its address: `production.host-a ssh-ed25519 AAAA...`. Add one line when you add a host to an environment. A new address of a host needs no new line. Give each name the key of its own machine only.

The child of a run has the user of Pupitre, so it can read each file of this user. A separate user for the child is the stronger setup.

## Run as a service

This unit file starts Pupitre with systemd. The key `KillMode=process` is important. It stops only the main process. A child `ansible-playbook` then lives on, and the next start adopts it.

```ini
[Unit]
Description=Pupitre
After=network-online.target
Wants=network-online.target

[Service]
User=pupitre
ExecStart=/usr/local/bin/pupitre serve --config /etc/pupitre/pupitre.hcl
KillMode=process
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

The exit code 1 shows a configuration error or a failure. The runner node also exits with the code 1 when it loses the lease `runner`. `Restart=on-failure` then starts it again. A restart of a container kills the children, and the runs of the runner node become `lost`.

## Network

Pupitre opens two listeners. Both use plain HTTP. Pupitre has no TLS on its listeners: the edge or the proxy does TLS.

| Listener | Key | Routes |
| --- | --- | --- |
| Main | `server.listen` | The page, the API and the two ProcurAuth routes |
| Ops | `server.ops_listen` | `/livez`, `/readyz` and `/metrics` |

The main listener serves a request only when its header `Host` is in `server.allowed_hosts`. This stops an attack with a rebound DNS name. Another value gets `421 misdirected_request`.

When `server.listen` is a loopback address, the key is optional. The default is `localhost`, `127.0.0.1`, `[::1]` and the address of the listener. For any other address, the key is necessary. List the public name of the service. The proxy MUST send that name in `Host`.

The edge MUST NOT reach the ops listener. A network rule must enforce this. The two listeners have different ports. Only the edge or the proxy reaches the main listener.

Pupitre connects out to these hosts only: the PDP, the PostgreSQL server, the OTLP collector and the managed hosts. It makes no request to the internet.

## Health probes

| Route | Answer |
| --- | --- |
| `GET /livez` | `200` with `ok` while the process serves |
| `GET /readyz` | `200` with `ok`, or `503` with the code of the first check that fails |

`/readyz` has three checks: `store` (the store is open), `catalog` (the last load is within the limits) and `runner_lease` (on the runner node, the node holds the lease). A PDP outage never changes `/readyz`.

The command `import --write` writes each document in its own transaction. On the backend `postgres`, each of them takes the barrier of 2.38 in shared mode, and a poll waits only for the transaction in flight. A poll that runs during an import can load some of the documents only. The catalog then shows the errors of the rules across documents until the next poll. Run `import` when nobody edits the environment.

When a host or an environment is deleted, remove its lines `<environment>.<host>` from the file `runner.known_hosts_file`. An id that is made again would get the old key.

## Logs, metrics and traces

Pupitre writes the log on the standard error: one JSON object on each line, with `t`, `level` and `event`. A line never holds a secret, a token or a value of a document. At each start, the line `server.config` holds the effective configuration with the secrets hidden. The warnings `access.mode_open`, `access.pdp_plain_http`, `runner.apply_in_open_mode` and `telemetry.plain_http` come at each start.

| Event | Meaning |
| --- | --- |
| `http.request` | One request, with the route, the status and the duration |
| `access.decision` | One question to the PDP and its answer |
| `document.write` | A write: the key, the version, the etags and the key paths that changed |
| `secret.write` | A write of a secret: the scope, the name and the fingerprint |
| `run.queued`, `run.started`, `run.finished`, `run.cancel` | The life of a run |
| `run.adopted`, `run.lost` | A run after a restart |
| `runner.lease_lost`, `runner.store_error` | A fault of the runner node |

The ops listener gives metrics in the Prometheus text format at `/metrics`. The main ones are `pupitre_http_requests_total`, `pupitre_pdp_requests_total`, `pupitre_store_errors_total`, `pupitre_runs_total`, `pupitre_runs_queued`, `pupitre_runs_running` and `pupitre_runs_oldest_queued_seconds`. A label never holds a subject, a node id or a value.

With `telemetry.enabled = true`, Pupitre exports spans with OTLP over HTTP to the endpoint of the file. It sets each value from the file, never from an `OTEL_*` variable.

## Several instances

The backend `postgres` serves two or more instances. They share the database and the same store key. Set `runner.execute = true` on exactly one of them: the runner node. A second instance with `runner.execute = true` stops at start with the message `another runner node holds the lease until <time>`. The other instances serve the API and write the runs as `queued`.

The backend `files` serves one instance. It is the runner node, so `runner.execute` must be `true` when `runner.enabled = true`.

## Back up

| Data | Backend `files` | Backend `postgres` |
| --- | --- | --- |
| Documents and history | The documents folder. It can be a Git checkout. | The database |
| Secrets | The secrets folder. Its values are in clear text. | The database. The values are encrypted. |
| Runs | The runs folder | The database |
| Key of the store | A file or a variable that the configuration names | The same |

When the documents folder is a Git checkout, commit `pupitre-store.json`, `environments/` and `.history/`. Put `.lock` and `.tmp/` in `.gitignore`. Only the process that holds the lock uses them.

The command `export` writes the documents and their versions into an empty folder for a review in Git. It writes no secret and no run. The command `copy-store` copies a whole store, with the secrets, into an empty store.

## Upgrade

1. Read the [changelog](../CHANGELOG.md). A line `BREAKING:` gives the step to take.
2. Stop the instances.
3. Replace the binary.
4. Run `pupitre migrate --config <path>`. It changes the store to the version of the binary. On a current store it changes nothing.
5. Start the instances.

A store with an older version stops the start with the message `run pupitre migrate`. A store with a newer version stops it with `the store is newer than this binary`. Before 1.0, a minor version can change a key, a route or a format.

## Find a problem

| Symptom | Cause and step |
| --- | --- |
| The start stops and names a key | A configuration error. Run `check-config` and read the message. |
| `the folder <path> is locked by another process` | A second process uses the folders of the backend `files`. Stop it. |
| `storage.secret_key does not match the key of the store` | The key is not the key that made the store. Use the right key. |
| `run pupitre migrate` | The store schema is old. Run `migrate`. |
| `another runner node holds the lease until <time>` | Another instance is the runner node. Set `runner.execute = false` here. |
| A run stays `queued` | No runner node is up, or the environment has a run. The metric `pupitre_runs_oldest_queued_seconds` shows the age. |
| `503 run_queue_full` | `runner.max_queued_runs` runs wait. Wait, or cancel runs. |
| `403 apply_disabled` | The mode is `open` and `runner.allow_apply_in_open_mode` is `false`. |
| `409 check_required`, `check_used`, `check_too_old` | The guard of an apply refuses. Start a new check run. |
| A run fails with `snapshot_changed` | A value or a playbook changed after the check. Start a new check run. |
| A host is `unreachable` with `Host key verification failed` | The file of pinned keys has no line `<environment>.<host>` with the key of this host, or the address is the address of another machine. Check the line and the address. |
| A run fails with `invalid_values` and `no template marker` | A stored text holds `{{`, `{%` or `{#`. Change the value. |
| A run is `lost` with `process_gone` | Pupitre restarted and the child did not live. Check the hosts by hand. |
| `503 pdp_unavailable` | The PDP gives no valid answer. The writes and the runs wait for it. |
| `/readyz` gives `503` | The code names the check: `store`, `catalog` or `runner_lease`. |
| `503 store_over_limit` | The store holds more than a limit, for example after a Git pull added a file. A `PUT` and a `POST` wait. Delete a visible document, or remove the extra files (backend `files`) or rows (backend `postgres`) outside the API. The next poll recovers. |
| A run record has `output_truncated` | The output is above `runner.max_log_bytes`. The full output stays on the runner node. |

The log line of each refusal has the request id. The answer has the same id in the header `X-Request-Id`.
