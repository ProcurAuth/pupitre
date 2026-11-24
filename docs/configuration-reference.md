# Pupitre configuration reference

This document is the reference of the configuration keys of Pupitre. It is part of section 6 of the specification. The command `pupitre print-reference --format markdown` generates it from the table of keys in the code. Do not edit it by hand.

Pupitre reads one HCL file. The file is the only source of the configuration. A key whose description gives a default can be absent: the default then applies, and `print-config` shows the source `default`. Each other key that applies is necessary, and each key that does not apply is forbidden.

The column Forms uses three letters. V is a value in the file. E is `<name>_env`, the name of an environment variable. F is `<name>_file`, the absolute path of a file. Give exactly one form for each key. A secret accepts only E and F.

A duration is a text with one whole number and one unit: `ms`, `s`, `m` or `h`. A size is a whole number of bytes. An integer is a whole number with no sign and no zero at the start. A text is in double quotes. A list is texts in brackets with commas.

The reference value is the value that the reference files use. It is not a default. Run `pupitre print-reference --format hcl --profile local` or `--profile distributed` to get a complete file. The two files hold no `command` block. The reference value of a key of a `command` block is a value for the example of 4.32.

Section 6 of the specification gives the checks after the table of keys, the contradictions and the warnings.

## Blocks

| Block | Count | Necessary when | Forbidden when |
|---|---|---|---|
| `server` | 1 | always | never |
| `log` | 1 | always | never |
| `telemetry` | 1 | always | never |
| `storage` | 1 | always | never |
| `storage.files` | 0 or 1 | `storage.backend = "files"` | `storage.backend = "postgres"` |
| `storage.postgres` | 0 or 1 | `storage.backend = "postgres"` | `storage.backend = "files"` |
| `storage.postgres.tls` | 0 or 1 | `storage.postgres.sslmode = "verify-full"` | `storage.postgres.sslmode = "disable"` |
| `access` | 1 | always | never |
| `access.identity` | 0 or 1 | `access.mode = "authzen"` | `access.mode = "open"` |
| `access.pdp` | 0 or 1 | `access.mode = "authzen"` | `access.mode = "open"` |
| `runner` | 1 | always | never |
| `limits` | 0 or 1 | never (the block is optional) | never |
| `schema "<id>"` | 0 to 256 | always | never |
| `playbook "<id>"` | 0 to 256 | always | never |
| `command "<id>"` | 0 to 256 | always | never |
| `command "<id>".param "<name>"` | 0 to 16 | always | never |

## Block `server`

The two listeners and the limits of the HTTP server (5.1, 7.10).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `server.listen` | an IP address and a port in canonical form | no | V | always | never | `"127.0.0.1:8080"` (local), `"0.0.0.0:8080"` (distributed) | The main listener (5.1). The canonical form of an IP address and a port. |
| `server.ops_listen` | an IP address and a port in canonical form | no | V | always | never | `"127.0.0.1:9090"` (local), `"0.0.0.0:9090"` (distributed) | The ops listener (5.20). Its port is not the port of `server.listen`. |
| `server.base_path` | a text of 1 to 128 bytes with no control character | no | V | always | never | `"/"` | The prefix of the routes of the main listener (5.2). `/`, or a path that starts and ends with `/`. A segment has 1 to 64 characters from `A-Z a-z 0-9 . _ ~ -` and is not `.` or `..`. |
| `server.allowed_hosts` | a list of 1 to 32 texts, each of 1 to 261 bytes with no space and no control character | no | V | `server.listen` is not a loopback address | never | absent (local), `["pupitre.example.invalid"]` (distributed) | The values of the header `Host` that the main listener serves (7.4). Each entry is a host name, an IPv4 address or an IPv6 address in brackets, with an optional port. An entry with no port matches each port. A request with another `Host` gets `421 misdirected_request`. Absent: the default is `localhost`, `127.0.0.1`, `[::1]` and the address of `server.listen`, and it applies only when `server.listen` is a loopback address. |
| `server.max_connections` | an integer from 16 to 65536 | no | V | never (the key is optional) | never | `1024` | The most open connections of the main listener. The default is 1024. |
| `server.idle_timeout` | a duration from 1s to 10m | no | V | never (the key is optional) | never | `"75s"` | Pupitre closes a kept connection that is idle for this time. The default is `75s`. |
| `server.shutdown_grace` | a duration from 5s to 10m | no | V | never (the key is optional) | never | `"30s"` | The longest graceful stop. The default is `30s`. |
| `server.header_read_timeout` | a duration from 1s to 1m | no | V | never (the key is optional) | never | `"10s"` | The longest read of the request line and the headers. The default is `10s`. |
| `server.body_read_timeout` | a duration from 1s to 5m | no | V | never (the key is optional) | never | `"30s"` | The longest read of a body. The default is `30s`. |
| `server.max_request_bytes` | a size in bytes from 65536 to 16777216 | no | V | never (the key is optional) | never | `1048576` | The largest request body. The default is 1048576. |
| `server.max_inflight_bodies` | an integer from 1 to 4096 | no | V | never (the key is optional) | never | `64` | The most request bodies in flight at one time. The default is 64. |
| `server.max_page_items` | an integer from 1 to 1000 | no | V | never (the key is optional) | never | `200` | The most items of one list page (5.6). The default is 200. |

## Block `log`

The log level (6.26).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `log.level` | "error", "warn", "info" or "debug" | no | V | always | never | `"info"` | The lowest level that Pupitre writes (6.26). |

## Block `telemetry`

The export of the spans with OTLP (6.29).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `telemetry.enabled` | a boolean | no | V | always | never | `false` | The value `true` exports the spans with OTLP (6.29). |
| `telemetry.otlp_endpoint` | a text of 1 to 2048 bytes with no control character | no | V, E, F | `telemetry.enabled = true` | `telemetry.enabled = false` | `"http://otel-collector.example.invalid:4318"` | A URL with the scheme `http` or `https`, a host and an optional port, with no path, query, fragment or user info. Pupitre sends to `<endpoint>/v1/traces`. |
| `telemetry.ca_file` | an absolute path of 1 to 4096 bytes | no | V | `telemetry.enabled = true` and an `https` endpoint | `telemetry.enabled = false`, or an `http` endpoint | absent | A PEM file of CA certificates for an `https` endpoint. |
| `telemetry.service_name` | a text of 1 to 64 bytes of printable ASCII | no | V | `telemetry.enabled = true` | `telemetry.enabled = false` | `"pupitre"` | The resource attribute `service.name`. |
| `telemetry.export_interval` | a duration from 1s to 5m | no | V | `telemetry.enabled = true` | `telemetry.enabled = false` | `"15s"` | The period of the export. |
| `telemetry.max_queued_spans` | an integer from 1 to 1000000 | no | V | never (the key is optional) | `telemetry.enabled = false` | `2048` | The most spans that wait for the export. Above it, a new span is dropped and counted. The default is 2048. |

## Block `storage`

The store of the documents, the secrets and the runs (section 2).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `storage.backend` | "files" or "postgres" | no | V | always | never | `"files"` (local), `"postgres"` (distributed) | The backend of the store (section 2). |
| `storage.secret_key` | a text of 64 to 64 bytes of printable ASCII | yes | E, F | always | never | `secret_key_file = "/run/secrets/pupitre/store-key"` | The key of the store: 64 lower-case hexadecimal characters (2.26). All instances of one store have the same key. |

## Block `storage.files`

The backend files: three folders and one writer process (2.31).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `storage.files.path` | an absolute path of 1 to 4096 bytes | no | V | always | never | `"/var/lib/pupitre/data"` (local), absent (distributed) | The folder of the documents (2.31). It can be a Git checkout. |
| `storage.files.secrets_path` | an absolute path of 1 to 4096 bytes | no | V | always | never | `"/var/lib/pupitre/secrets"` (local), absent (distributed) | The folder of the secrets, mode 0700, outside the folder of the documents (2.31, 2.34). |
| `storage.files.runs_path` | an absolute path of 1 to 4096 bytes | no | V | always | never | `"/var/lib/pupitre/runs"` (local), absent (distributed) | The folder of the runs, outside the folder of the documents (2.31, 2.35). |
| `storage.files.rescan_interval` | a duration from 1s to 1h | no | V | never (the key is optional) | `storage.backend = "postgres"` | `"5s"` | The period of the scan for changes outside Pupitre (2.33). The default is `5s`. |
| `storage.files.history_keep` | an integer from 1 to 10000 | no | V | never (the key is optional) | `storage.backend = "postgres"` | `100` | The versions kept for each key, and the events kept for each secret (2.18). The default is 100. |

## Block `storage.postgres`

The backend postgres (2.36).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `storage.postgres.host` | a DNS name of 1 to 253 bytes, or an IP address | no | V | always | never | absent (local), `"pg.example.invalid"` (distributed) | The host of the PostgreSQL server. |
| `storage.postgres.port` | an integer from 1 to 65535 | no | V | always | never | absent (local), `5432` (distributed) | The port of the PostgreSQL server. |
| `storage.postgres.database` | a text of 1 to 63 bytes with no control character | no | V | always | never | absent (local), `"pupitre"` (distributed) | The name of the database. |
| `storage.postgres.user` | a text of 1 to 63 bytes with no control character | no | V, E, F | always | never | absent (local), `user_file = "/run/secrets/pg/username"` (distributed) | The name of the account. |
| `storage.postgres.password` | a text of 1 to 1024 bytes | yes | E, F | always | never | absent (local), `password_file = "/run/secrets/pg/password"` (distributed) | The password of the account. |
| `storage.postgres.sslmode` | "verify-full" or "disable" | no | V | always | never | absent (local), `"verify-full"` (distributed) | The value `verify-full` checks the certificate and the name. |
| `storage.postgres.max_connections` | an integer from 2 to 64 | no | V | never (the key is optional) | `storage.backend = "files"` | `8` | The size of the connection pool (2.37). The default is 8. |
| `storage.postgres.connect_timeout` | a duration from 1s to 1m | no | V | never (the key is optional) | `storage.backend = "files"` | `"5s"` | The longest wait for a connection (2.37). The default is `5s`. |
| `storage.postgres.statement_timeout` | a duration from 100ms to 1m | no | V | never (the key is optional) | `storage.backend = "files"` | `"10s"` | The longest statement (2.37). The default is `10s`. |
| `storage.postgres.poll_interval` | a duration from 1s to 5m | no | V | never (the key is optional) | `storage.backend = "files"` | `"5s"` | The period of the poll (2.38). The default is `5s`. |
| `storage.postgres.history_keep` | an integer from 1 to 10000 | no | V | never (the key is optional) | `storage.backend = "files"` | `100` | The versions kept for each key, and the events kept for each secret (2.18). The default is 100. |

## Block `storage.postgres.tls`

The TLS link to PostgreSQL.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `storage.postgres.tls.ca_file` | an absolute path of 1 to 4096 bytes | no | V | always | never | absent (local), `"/run/secrets/pg-ca/ca.crt"` (distributed) | A PEM file of CA certificates. |
| `storage.postgres.tls.server_name` | a DNS name of 1 to 253 bytes | no | V | always | never | absent (local), `"pg.example.invalid"` (distributed) | The name that the certificate must hold. |

## Block `access`

The access mode (section 3).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `access.mode` | "open" or "authzen" | no | V | always | never | `"open"` (local), `"authzen"` (distributed) | The access mode (3.2). The mode `open` writes a warning at each start. |
| `access.options_max_questions` | an integer from 1 to 10000 | no | V | never (the key is optional) | `access.mode = "open"` | `200` | The most questions of one list request or options request (3.15, 3.16). The default is 200. |
| `access.options_page_items` | an integer from 1 to 50 | no | V | never (the key is optional) | `access.mode = "open"` | `50` | The most items of one page of the options route (3.16). The default is 50. |

## Block `access.identity`

The identity token of the edge (3.4).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `access.identity.header` | a text of 1 to 64 bytes with no control character | no | V | always | never | absent (local), `"X-Identity-Token"` (distributed) | The name of the header that holds the token: 1 to 64 token characters (RFC 9110). It is not `Authorization`, `Proxy-Authorization`, `Cookie`, `Host`, `Content-Type` or `Content-Length`, in any case. |
| `access.identity.issuer` | a text of 1 to 1024 bytes with no control character | no | V | always | never | absent (local), `"https://edge.example.invalid"` (distributed) | The value that the claim `iss` must have. |
| `access.identity.audience` | a text of 1 to 256 bytes with no control character | no | V | always | never | absent (local), `"pupitre"` (distributed) | The service id (3.6): 1 to 256 bytes of UTF-8. |
| `access.identity.public_keys` | a text of 1 to 65536 bytes | no | V, E, F | always | never | absent (local), `public_keys_file = "/run/secrets/edge/jwks.json"` (distributed) | The public keys of the edge: PEM text or a JWK Set with 1 to 16 keys. |
| `access.identity.max_age` | a duration from 1s to 24h | no | V | always | never | absent (local), `"5m"` (distributed) | The longest life of a token. A whole number of seconds. |
| `access.identity.clock_skew` | a duration from 0s to 5m | no | V | always | never | absent (local), `"1m"` (distributed) | The skew that applies to `iat`, `exp` and `nbf`. A whole number of seconds. |

## Block `access.pdp`

The link to the policy decision point (3.5).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `access.pdp.url` | a text of 1 to 2048 bytes with no control character | no | V, E, F | always | never | absent (local), `"https://pdp.example.invalid:4182"` (distributed) | A URL with the scheme `http` or `https`, a host, an optional port and an optional path, with no user info, query or fragment. |
| `access.pdp.token` | a text of 1 to 4096 bytes with no space and no control character | yes | E, F | always | never | absent (local), `token_file = "/run/secrets/pdp/token"` (distributed) | The credential of Pupitre at the PDP, sent in `Authorization: Bearer`. |
| `access.pdp.ca_file` | an absolute path of 1 to 4096 bytes | no | V | `access.pdp.plain_http = false` | `access.pdp.plain_http = true`, or `access.mode = "open"` | absent (local), `"/run/secrets/pdp/ca.pem"` (distributed) | A PEM file of CA certificates. |
| `access.pdp.plain_http` | a boolean | no | V | always | never | absent (local), `false` (distributed) | The value `true` accepts an `http` URL, with a warning at start. It must agree with the scheme of `access.pdp.url`. |
| `access.pdp.timeout` | a duration from 10ms to 1m | no | V | always | never | absent (local), `"500ms"` (distributed) | The longest wait for the PDP. |
| `access.pdp.cache_ttl` | a duration from 0s to 1h | no | V | always | never | absent (local), `"30s"` (distributed) | The longest life of a kept answer (3.17). |
| `access.pdp.max_connections` | an integer from 1 to 32 | no | V | always | never | absent (local), `4` (distributed) | The most requests to the PDP at one time. |
| `access.pdp.cache_entries` | an integer from 1 to 1000000 | no | V | never (the key is optional) | `access.mode = "open"` | `16384` | The most kept answers (3.17). The default is 16384. |

## Block `runner`

The runs (section 4). The keys of the runner node need `runner.execute = true`.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `runner.enabled` | a boolean | no | V | always | never | `true` | The value `true` serves the runs (section 4). The value `false` serves no run and publishes no run action (3.3). |
| `runner.execute` | a boolean | no | V | `runner.enabled = true` | `runner.enabled = false` | `true` | The value `true` makes this instance the runner node (4.1). With the backend `files`, it is `true`. |
| `runner.allow_apply_in_open_mode` | a boolean | no | V | `access.mode = "open"` and `runner.enabled = true` | `access.mode = "authzen"`, or `runner.enabled = false` | `false` (local), absent (distributed) | The value `true` accepts the applies and the commands in the mode `open` (3.22). |
| `runner.check_max_age` | a duration from 1m to 24h | no | V | `runner.enabled = true` | `runner.enabled = false` | `"30m"` | The oldest check that an apply can use (4.24). |
| `runner.max_hosts` | an integer from 1 to 10000 | no | V | `runner.enabled = true` | `runner.enabled = false` | `100` | The most hosts of one run (3.10). It is also the `max` of the ceiling `max_hosts`. |
| `runner.max_queued_runs` | an integer from 1 to 10000 | no | V | never (the key is optional) | `runner.enabled = false` | `100` | The most runs with the status `queued` (4.5). The default is 100. |
| `runner.work_dir` | an absolute path of 1 to 4096 bytes | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/var/lib/pupitre/runner"` | The folder of the runs on the runner node (4.10). |
| `runner.secrets_dir` | an absolute path of 1 to 4096 bytes | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/dev/shm/pupitre"` | The folder of the private folders of the runs, on `tmpfs` or `ramfs`, mode 0700 (4.13). |
| `runner.ansible_playbook` | an absolute path of 1 to 4096 bytes | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/usr/bin/ansible-playbook"` | The one program that the runner starts (4.17). It MUST be ansible-core 2.19 or later (7.8). |
| `runner.path` | a text of 1 to 4096 bytes with no space and no control character | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/usr/local/bin:/usr/bin:/bin"` | The value of `PATH` for the child (4.18). |
| `runner.ssh_user` | a text of 1 to 32 bytes with no space and no control character | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"deploy"` | The SSH account on the hosts (4.15). |
| `runner.ssh_key_file` | an absolute path of 1 to 4096 bytes | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/run/secrets/ssh/id_ed25519"` | The SSH private key, a file of the user of the process with the mode 0600 or 0400 (7.8). |
| `runner.known_hosts_file` | an absolute path of 1 to 4096 bytes | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"/run/secrets/ssh/known_hosts"` | The pinned host keys: one line for each `<environment>.<host>`, and no other line (4.12, 7.11). |
| `runner.releases_dir` | an absolute path of 1 to 4096 bytes | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `"/srv/releases"` | The local folder of the release files, given to the playbooks as `pupitre_releases_dir` (4.14). The key is optional and has no default. |
| `runner.run_timeout` | a duration from 1m to 48h | no | V | `runner.enabled = true` and `runner.execute = true` | `runner.enabled = false`, or `runner.execute = false` | `"2h"` | The longest run (4.28). |
| `runner.forks` | an integer from 1 to 100 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `5` | The value of `forks` of `ansible.cfg` (4.15). The default is 5. |
| `runner.max_parallel_runs` | an integer from 1 to 64 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `1` | The most runs at one time on the runner node (4.6). The default is 1. |
| `runner.cancel_grace` | a duration from 1s to 10m | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `"30s"` | The time between `SIGTERM` and `SIGKILL` (4.19). The default is `30s`. |
| `runner.lease_ttl` | a duration from 5s to 10m | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `"30s"` | The time to live of the leases (4.7, 4.8). The default is `30s`. |
| `runner.max_log_bytes` | a size in bytes from 65536 to 1073741824 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `16777216` | The most stored output of one run (4.22). The default is 16777216. |
| `runner.max_events` | an integer from 100 to 10000000 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `100000` | The most stored events of one run (4.22). The default is 100000. |
| `runner.max_event_bytes` | a size in bytes from 1024 to 1048576 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `65536` | The longest event line (4.20). The default is 65536. |
| `runner.runs_keep` | an integer from 10 to 1000000 | no | V | never (the key is optional) | `runner.enabled = false`, or `runner.execute = false` | `10000` | The most final runs that the store keeps (4.31). The default is 10000. |

## Block `limits`

The limits of the store (2.42). The block and each key are optional.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `limits.max_environments` | an integer from 1 to 1024 | no | V | never (the key is optional) | never | `64` | The most environments (2.42). The default is 64. |
| `limits.max_units_per_environment` | an integer from 1 to 10000 | no | V | never (the key is optional) | never | `1000` | The most units of one environment (2.42). The default is 1000. |
| `limits.max_hosts_per_environment` | an integer from 1 to 10000 | no | V | never (the key is optional) | never | `1000` | The most hosts of one environment, and of one unit (2.42). The default is 1000. |
| `limits.max_document_bytes` | a size in bytes from 4096 to 1048576 | no | V | never (the key is optional) | never | `262144` | The largest canonical document (2.42). The default is 262144. |
| `limits.max_store_bytes` | a size in bytes from 1048576 to 17179869184 | no | V | never (the key is optional) | never | `268435456` | The most canonical bytes of all current documents (2.42). The default is 268435456. |

## Block `schema "<id>"`

One schema of a unit: a file of 2.11. The label is the schema id.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `schema "<id>".path` | an absolute path of 1 to 4096 bytes | no | V | always | never | `"/srv/ansible/schemas/relay.json"` | The schema file (2.11). Pupitre reads it at start. |

## Block `playbook "<id>"`

One playbook of the catalog of capabilities (4.2). The label is the playbook id. Only with `runner.enabled = true` (6.14).

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `playbook "<id>".kind` | "deploy", "render", "status" or "command" | no | V | always | never | `"deploy"` | The kind of the playbook (4.2, 4.3). |
| `playbook "<id>".path` | an absolute path of 1 to 4096 bytes | no | V | always | never | `"/srv/ansible/playbooks/deploy-relay.yml"` | The playbook file. The runner node reads it at start and at each run (4.16). |

## Block `command "<id>"`

One command (4.32). The label is the command id. Only with `runner.enabled = true` (6.14). The reference files hold no command block.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `command "<id>".label` | a text of 1 to 256 bytes with no control character | no | V | always | never | `"Restart the service"` | The text of the page: 1 to 64 characters (4.32). |
| `command "<id>".playbook` | a text of 1 to 63 bytes with no control character | no | V | always | never | `"restart-service"` | The id of a block `playbook` with the kind `command` (4.32). |
| `command "<id>".target` | "environment", "unit" or "host" | no | V | always | never | `"unit"` | The type of the node of a launch (4.32). |
| `command "<id>".check` | a boolean | no | V | always | never | `false` | The value `true` makes an apply need a check of the same snapshot (4.24). |

## Block `command "<id>".param "<name>"`

One parameter of a command (4.33). The label is the parameter name.

| Key | Type and range | Secret | Forms | Necessary when | Forbidden when | Reference value | Description |
|---|---|---|---|---|---|---|---|
| `command "<id>".param "<name>".label` | a text of 1 to 256 bytes with no control character | no | V | always | never | `"Grace period"` | The label of the form: 1 to 64 characters (4.33). |
| `command "<id>".param "<name>".type` | "text", "integer", "boolean", "duration" or "choice" | no | V | always | never | `"duration"` | The type of the parameter (4.33). |
| `command "<id>".param "<name>".required` | a boolean | no | V | always | never | `true` | The value `true` makes the parameter necessary in a request (4.33). |
| `command "<id>".param "<name>".min` | an integer from 0 to 9007199254740991 | no | V | `type = "text"` or `type = "integer"` | another type | `1` | The lowest value of an `integer`, or the fewest characters of a `text`. |
| `command "<id>".param "<name>".max` | an integer from 0 to 9007199254740991 | no | V | `type = "text"` or `type = "integer"` | another type | `64` | The highest value of an `integer`, or the most characters of a `text`. |
| `command "<id>".param "<name>".min_duration` | a duration from 0s to 8760h | no | V | `type = "duration"` | another type | `"1s"` | The lowest value of a `duration`. |
| `command "<id>".param "<name>".max_duration` | a duration from 0s to 8760h | no | V | `type = "duration"` | another type | `"5m"` | The highest value of a `duration`. |
| `command "<id>".param "<name>".choices` | a list of 1 to 32 texts, each of 1 to 64 bytes with no control character | no | V | `type = "choice"` | another type | `["soft", "hard"]` | The closed list of a `choice`, each text one time. |

