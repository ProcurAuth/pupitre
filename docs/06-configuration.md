# Pupitre specification, part 6: configuration

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document. The paragraphs 6.1 to 6.25 keep the numbers of the engine paragraphs of Hune. So the comments of the crate `hclconf` cite them.

## 6. Configuration

### 6.1 One file, one source

Pupitre reads its configuration from one HCL file. Each command that needs a configuration takes `--config <path>` (short form `-c`). The path is absolute. There is no default path. `version`, `help` and `print-reference` read no configuration. `copy-store` takes `--from` and `--to` (6.24).

The file is the only source of the configuration. Pupitre reads an environment variable only when an attribute of the file names it with `_env` (6.5). It reads a file other than the configuration file only when a key names it. It reads the file one time, at start, into one typed structure. A change of the file has an effect only after a restart.

The engine is the crate `hclconf` of the workspace, with its `PROVENANCE` file. It has three built-in behaviours:

1. A duration has exactly one integer and one unit (6.4).
2. A block can depend on a mode key, as an attribute can.
3. A boolean key can be a mode key.

### 6.2 The nine rules

| Rule | What Pupitre does |
|---|---|
| 1. Closed schema | An unknown block, an unknown attribute, a duplicate attribute, a duplicate block and a duplicate label stop the start. |
| 2. Fail-fast | Each configuration error stops the program before it opens a port. The message names the key, the expected type or range, and the value (6.14). |
| 3. No coercion | Pupitre never changes a value. The text `"5"` is not the integer `5`. A value out of range stops the start. |
| 4. Typed once | Only the module `crates/pupitre/src/config/` uses `hclconf`. It builds one typed `Config`. The rest of the code reads only this structure. |
| 5. Single source | `main` captures and clears the environment first (6.23). Only `crates/hclconf/src/env.rs` reads the environment. No dependency reads `OTEL_*`, a proxy variable or a CA store variable. The child of a run gets an explicit environment (4.18). |
| 6. `_env` and `_file` | Exactly one form for each key. A secret accepts only `_env` and `_file` (6.5). |
| 7. Documented defaults | The keys whose description in the reference gives a default can be absent. Each other key that applies is necessary. The reference document comes from the table of keys (6.21). |
| 8. Effective configuration | `print-config` prints the value and the source of each key, with the secrets hidden. `serve` writes the same lines in its start log (6.20, 6.22). |
| 9. Contradictions | Two keys that contradict each other stop the start (6.14). |

A key that sets exposure, identity, storage, access or the power of the runner has no default. Examples: `server.listen`, `storage.backend`, `access.mode`, `runner.enabled`, `runner.max_hosts` and `runner.check_max_age`. A key that only bounds a resource can have a default.

### 6.3 Syntax of the file

The file is a subset of HCL:

- Blocks, labelled blocks (`playbook "deploy-relay" { }`) and attributes (`name = value`). A labelled block can hold one level of labelled blocks: `command "<id>" { param "<name>" { } }`.
- Values are literals only: a text in double quotes, a whole number, `true`, `false`, or a list in brackets.
- A text has the escapes `\"`, `\\`, `\n`, `\r`, `\t` and `\uXXXX`. A text ends on its line.
- Comments start with `#` or `//`, or sit between `/*` and `*/`.
- The engine refuses expressions, operators, functions, variables, templates, heredocs, objects and `null`.

A syntax error and a duplicate attribute stop the check at once. Each other error is in the list, in the order of the file (6.25).

### 6.4 Values

| Kind | Syntax | Examples | Refused |
|---|---|---|---|
| Duration | A text with one whole number and one unit: `ms`, `s`, `m` or `h`. No sign, no fraction, no space, no zero at the start except `0`. | `"500ms"`, `"30s"`, `"5m"`, `"2h"` | `"1m30s"`, `"1.5s"`, `"30"`, `30`, `"1d"` |
| Size | A whole number of bytes, with no unit | `1048576` | `"1MiB"`, `"1048576"` |
| Integer | A whole number in decimal, with no sign, no zero at the start, no fraction and no exponent | `16`, `0` | `16.0`, `"16"`, `+16`, `016` |
| Boolean | `true` or `false` | `true` | `"true"`, `1`, `yes` |
| Text | A text in double quotes | `"deploy"` | `deploy` |
| List | Texts in brackets with commas | `["soft", "hard"]` | |

`access.identity.max_age` and `access.identity.clock_skew` are whole numbers of seconds: `"1500ms"` stops the start.

### 6.5 `_env` and `_file`

An attribute `<name>` that accepts the indirection has three forms:

- `<name> = "..."`: the value is in the file (form V).
- `<name>_env = "VAR"`: the value is in the environment variable `VAR` (form E).
- `<name>_file = "/path"`: the value is the content of the file, without one last line end (form F).

The rules:

- Give exactly one form. Two forms stop the start, and Pupitre then reads neither the variable nor the file. No form of a necessary key stops the start.
- A secret accepts only E and F. A secret in clear text stops the start, also together with another form. The error never shows the value.
- A variable that is not set, or that is empty, stops the start. A value that starts or ends with a space or a line end stops the start. A value that is not UTF-8 stops the start.
- A file that Pupitre cannot read stops the start. An empty file, a content that is not UTF-8, a content of more than 65,536 bytes, or a byte order mark also stops the start.
- A variable name has `A-Z`, `a-z`, `0-9` and `_`, and does not start with a digit. The names `SSL_CERT_FILE` and `SSL_CERT_DIR` stop the start (6.23).
- The error gives the attribute and the variable or the path, never the value.

| Key | Forms | Secret |
|---|---|---|
| `storage.secret_key` | E, F | yes |
| `storage.postgres.user` | V, E, F | no |
| `storage.postgres.password` | E, F | yes |
| `access.identity.public_keys` | V, E, F | no |
| `access.pdp.url` | V, E, F | no |
| `access.pdp.token` | E, F | yes |
| `telemetry.otlp_endpoint` | V, E, F | no |

Each other key accepts only the form V.

### 6.6 Paths

- A path has 1 to 4,096 bytes and no NUL byte. It is absolute.
- Each path of a key is canonical: no `.` or `..` segment, no repeated `/` and no `/` at the end. Stage 2 checks it (6.13).
- A CA file, a schema file and a `_file` value are regular files. Pupitre opens their last part with `O_NOFOLLOW`.

### 6.7 The table of keys

The table of keys is `crates/pupitre/src/config/keys.rs`. Each key has a path, a type with its range, its forms, the rule of the modes, a reference value and a description. The engine checks the file against it (stage 1), and `print-reference` writes the reference files from it (6.21).

- A key that applies is necessary, except a key whose description gives a default. A key that does not apply is forbidden. So a key of a mode is necessary in that mode, and the file must not give it in another mode.
- The default of a key applies only when the key applies. `print-config` shows it with the source `default`.
- The document `docs/configuration-reference.md` lists each block and each key, with the columns Type and range, Secret, Forms, Necessary when, Forbidden when, Reference value and Description. It is part of this specification. A test of the crate `pupitre` compares it with the output of `print-reference --format markdown`.

### 6.8 The block `server`

The block holds the two listeners (5.1), the base path (5.2), the list of the host names (7.4) and the limits (7.10). The reference document gives each key.

`server.allowed_hosts` is a closed list of the values of `Host` that the main listener serves (7.4). It is optional when `server.listen` is a loopback address, and necessary for any other address.

### 6.9 The blocks `log` and `telemetry`

`log.level` sets the lowest level of the log (6.26). `telemetry.enabled` turns the export of the spans on (6.29). With `telemetry.enabled = true`, the keys of the endpoint are necessary. Pupitre sets each value of the OpenTelemetry export from the file, never from an `OTEL_*` variable.

### 6.10 The block `storage`

`storage.backend` chooses the backend: `files` (2.31) or `postgres` (2.36). The block of the other backend is forbidden. `storage.secret_key` is necessary for both backends (2.26). With `storage.postgres.sslmode = "verify-full"`, the block `storage.postgres.tls` is necessary. With `disable`, it is forbidden.

### 6.11 The block `access`

`access.mode` is `open` or `authzen` (3.2). It has no default. With `authzen`, the blocks `access.identity` and `access.pdp` are necessary, and the keys `access.options_max_questions` and `access.options_page_items` apply. With `open`, they are forbidden.

### 6.12 The blocks `runner`, `limits`, `schema`, `playbook` and `command`

- `runner.enabled` turns the runs on (3.3, 4.1). With `true`, `runner.execute`, `runner.check_max_age` and `runner.max_hosts` are necessary. The key `runner.allow_apply_in_open_mode` is necessary in the mode `open` and forbidden in the mode `authzen` (3.22).
- With `runner.execute = true`, the keys of the runner node are necessary: `work_dir`, `secrets_dir`, `ansible_playbook`, `path`, `ssh_user`, `ssh_key_file`, `known_hosts_file` and `run_timeout`. With `runner.execute = false`, they are forbidden.
- The block `limits` and each of its keys are optional (2.42).
- `schema "<id>"`: 0 to 256 blocks. `path` names the schema file (2.11).
- `playbook "<id>"`: 0 to 256 blocks. `kind` is `deploy`, `render`, `status` or `command`. `path` is the playbook file (4.2).
- `command "<id>"`: 0 to 256 blocks, with `label`, `playbook`, `target`, `check`, and 0 to 16 blocks `param "<name>"` (4.32). A block `param` has `label`, `type` and `required`. Its type gives its other keys: `min` and `max` for `text` and `integer`, `min_duration` and `max_duration` for `duration`, `choices` for `choice`, and none for `boolean` (4.33).

### 6.13 Stage 2: the checks of the values

Stage 2 runs after the table of keys. It checks what the table cannot express:

1. Each path is canonical (6.6).
2. `server.base_path` is `/`, or a path that starts and ends with `/`. A segment has 1 to 64 characters from `A-Z a-z 0-9 . _ ~ -` and is not `.` or `..`.
3. `storage.secret_key` has exactly 64 lower-case hexadecimal characters. The error never shows the value.
4. `telemetry.otlp_endpoint` is a URL with the scheme `http` or `https`, a host and an optional port, with no path, query, fragment or user info. `access.pdp.url` has the same form, and it can have a path.
5. `access.identity.header` has 1 to 64 token characters of RFC 9110, and it does not name a reserved header.
6. `access.identity.public_keys` is PEM text or a JWK Set with 1 to 16 keys of `Ed25519` or `ES256`.
7. `access.identity.max_age` and `access.identity.clock_skew` are whole numbers of seconds.
8. Each CA file is a regular file of 1 MiB at most, with 1 to 64 PEM blocks `CERTIFICATE` and no other text. Each block is an X.509 certificate.
9. Each schema file is a regular file of 1 MiB at most that passes 2.11.
10. Each label of a block `schema`, `playbook` and `command` follows its id rule (2.1). Each label of a block `param` follows the rule of a parameter name.
11. `label` of a command and of a parameter has 1 to 64 characters. `playbook` of a command follows the id rule.
12. The keys of a block `param` follow its type (6.12). `min` is not above `max`, and `min_duration` is not above `max_duration`. Each item of `choices` occurs one time.
13. Each entry of `server.allowed_hosts` is a host name, an IPv4 address or an IPv6 address in brackets, with an optional port from 1 to 65535. An entry occurs one time. The key is necessary when `server.listen` is not a loopback address (7.4).

The runner node runs more checks at the start of `serve`, before it takes the lease `runner` (4.8, 7.8). They check the SSH key file, the known hosts file, the folder of the secrets on `tmpfs`, the work folder and the program `ansible-playbook`.

The runner runs the program with `--version`, and the line `ansible-playbook [core X.Y.Z]` MUST show `core 2.19` or later. An older version stops the start. An output that the runner cannot read also stops the start. The error names the key `runner.ansible_playbook`, the expected range and the version found. A failure stops the start with the key and the cause.

Each line of `runner.known_hosts_file` names a host by `<environment>.<host>`, not by its address (4.12). The runner does not check the names in the file. The operator removes the line of a host or an environment that is deleted (7.11).

### 6.14 Errors and contradictions

An error has the form `<key>: <message> (line <n>)`. The key is in the form of `print-config`, for example `command "restart".param "grace".min`. The message gives the expected type or range, and the value, never a secret value.

Stage 3 checks the contradictions. Each one stops the start:

| Id | Contradiction |
|---|---|
| C1 | `server.ops_listen` has the port of `server.listen`. |
| C2 | `storage.backend = "files"` with `runner.enabled = true` and `runner.execute = false`. The backend `files` has one instance, so it is the runner node. |
| C3 | A block `playbook` or `command` with `runner.enabled = false`. |
| C4 | `playbook` of a command is not the id of a block `playbook` with the kind `command`. |
| C5 | Two blocks `playbook` with the same `path`. |
| C6 | One of `storage.files.path`, `storage.files.secrets_path`, `storage.files.runs_path`, `runner.work_dir` and `runner.secrets_dir` is inside another one. |
| C7 | `access.pdp.plain_http` does not agree with the scheme of `access.pdp.url`. |
| C8 | `telemetry.ca_file` is absent with an `https` endpoint, or present with an `http` endpoint. |
| C9 | A key of a block `param` that its type does not take, or a key that its type needs and that is absent (6.12). |
| C10 | Two schemas give one key path two kinds or two secret flags, or two schemas have the same `variable` or the same `group` (2.11). |
| C11 | `access.mode = "authzen"` with `access.options_max_questions` below 8 and `runner.enabled = true`, or below 4 and `runner.enabled = false`. One item of the options route needs that many questions (3.16). |

### 6.15 Warnings

A warning never stops the start. `serve` writes each one in the log at the level `warn`, at each start. `check-config` writes each one on the standard error.

| Event | Cause |
|---|---|
| `access.mode_open` | `access.mode = "open"` |
| `access.pdp_plain_http` | `access.pdp.plain_http = true` |
| `runner.apply_in_open_mode` | `runner.allow_apply_in_open_mode = true` |
| `telemetry.plain_http` | An `http` endpoint for OTLP |

### 6.16 The profile local

`print-reference --format hcl --profile local` writes a complete file: the backend `files`, the mode `open`, the runner node, one schema and one playbook, and no telemetry.

### 6.17 The profile distributed

`print-reference --format hcl --profile distributed` writes a complete file: the backend `postgres` with TLS, the mode `authzen`, the runner node, one schema and one playbook, and no telemetry. An instance that is not the runner node has `runner.execute = false` and no key of the runner node.

### 6.18 Read modes

| Mode | Commands | What it reads |
|---|---|---|
| All | `serve`, `check-config`, `print-config` | Each `_env` and `_file` value |
| Storage | `migrate`, `import`, `export`, `copy-store` | The `_env` and `_file` values of the block `storage` only |

A value that a mode does not read shows as `<not read>`. Its form and its source are still checked. `serve` starts only from the mode All.

### 6.19 The three stages

1. Stage 1: the table of keys, the forms and the reads of `_env` and `_file` (6.3 to 6.7).
2. Stage 2: the checks of the values (6.13).
3. Stage 3: the contradictions (6.14).

A stage runs only when the stage before it has no error. The error list is the list of the first stage that fails.

### 6.20 `print-config`

`print-config` prints one line for each key that has a value, in the order of the table of keys:

```text
server.listen = "127.0.0.1:8080"  # file
server.max_connections = 1024  # default
storage.secret_key = <hidden>  # file:/run/secrets/pupitre/store-key
```

- The value has the form of the file. A text is in double quotes with the escapes of JSON. An integer is in decimal. A duration is in its canonical form in quotes. A list is in brackets.
- A secret shows as `<hidden>`.
- The source is `file`, `env:VAR`, `file:/path` or `default`.
- A key of a labelled block shows its labels: `playbook "deploy-relay".path`.

### 6.21 `print-reference`

`print-reference --format hcl --profile local|distributed` writes a complete file of the profile (6.16, 6.17). `print-reference --format markdown` writes the document `docs/configuration-reference.md`. The two outputs come from the table of keys only. They read no file and no variable.

### 6.22 The start log

`serve` writes the line `server.config` at the level `info` after stage 3. Its field `config` holds one member for each line of `print-config`, with the value as JSON. Its field `sources` holds the source of each key whose source is not `file`. A secret is `"<hidden>"`.

### 6.23 The environment

`main` calls `CapturedEnvironment::capture_and_clear` first, before any other thread starts. It copies each variable, then removes it from the process, except `SSL_CERT_FILE` and `SSL_CERT_DIR`. No code of Pupitre reads these two variables. The engine reads the variables that `_env` attributes name from the copy. A test of the crate `hclconf` checks the capture in its own process.

### 6.24 The command line

| Command | Options | Function |
|---|---|---|
| `serve` | `--config <path>` | Start the server (section 5). |
| `check-config` | `--config <path>` | Run the three stages and print `the configuration is valid`. It sends nothing on the network. |
| `print-config` | `--config <path>` | Print the effective configuration (6.20). |
| `print-reference` | `--format hcl --profile local\|distributed`, or `--format markdown` | Print a reference (6.21). |
| `migrate` | `--config <path>` | Make or change the store schema (2.30). It never runs at start. |
| `import` | `--config <path> --inventory-json <path> --environment <id> [--write]` | Read the output of `ansible-inventory --list --export` and make the documents of a new environment (6.24a). |
| `export` | `--config <path> --to <folder>` | Write the documents and their versions into an empty folder (2.40). |
| `copy-store` | `--from <path> --to <path>` | Copy a store into an empty store (2.40). Each option names a configuration file. |
| `version` | none | Print `pupitre <version>` and the licence. `--version` and `-V` are the same. |
| `help` | none | Print the usage text. `--help` and `-h` are the same. |

- Each path of an option is absolute. A relative path, a repeated option, an unknown option and an unknown command give the exit code 2. The usage text then goes to the standard error.
- The exit code 0 is a success. The exit code 1 is a configuration error or a failure. The exit code 2 is a command line that is not valid.

### 6.24a The command `import`

`import` reads one JSON file: the output of `ansible-inventory --list --export` of one inventory. Ansible itself reads the YAML of the inventory, so Pupitre needs no YAML reader.

1. The environment of `--environment` MUST NOT exist in the store. Else the command stops with exit code 1.
2. For each host of the inventory, `import` makes a document `host`. `ansible_host` gives `address`, and `ansible_port` gives `port`. Each group of the host that is a host label of 2.1, and that is not the group of a schema, becomes a label.
3. A schema of the configuration whose `group` is a group of the inventory gives one unit. The unit has the id of the schema, the hosts of the group, and its playbooks. The playbooks are the blocks `playbook` with the same id as the schema and the prefixes `deploy-`, `render-` and `status-`. A schema with no such block `deploy-<id>` stops the import.
4. The variable `variable` of the schema in the vars of the group becomes the document `unit_values`. In the vars of a host, it becomes a document `host_values`. Each member of the nested object becomes a key path.
5. Each value MUST pass 2.10. A value that fails stops the command with the list of the errors. Pupitre never changes a value.
6. A secret key of a schema in the inventory stops the import. A secret goes into the store only through the API.
7. Each other variable is not imported. The report lists each one with its group or host.
8. Without `--write`, the command prints the report and writes nothing. With `--write`, it writes each document with the subject `import`. The call is `put_import` of the trait `Store` (2.18), which makes the version kind `import`.

### 6.25 Limits of the file

| Limit | Value |
|---|---|
| Size of the configuration file | 1 MiB |
| Size of the content of a `_file` value | 65,536 bytes |
| Errors for one key | 10, then one line `and <n> more errors` |
| Errors in one list | 100, then one line `and <n> more errors` |

### 6.26 The log

Pupitre writes the log on the standard error: one JSON object on each line. Each line has `t` (the time of 5.3), `level` (`error`, `warn`, `info` or `debug`), `event` and the fields of the event. A line of a request also has `request_id`. A line never holds a secret, a token, a credential or a value of a document (7.9). `log.level` is the lowest level that Pupitre writes.

### 6.27 The log events

| Event | Level | Fields |
|---|---|---|
| `server.start` | info | `version`, `listen`, `ops_listen` |
| `server.config` | info | 6.22 |
| `server.stop` | info | `reason` |
| `access.mode_open`, `access.pdp_plain_http`, `runner.apply_in_open_mode`, `telemetry.plain_http` | warn | `message` (6.15) |
| `access.decision` | info | `subject`, `owner`, `action`, `resource`, `match`, `decision`, `cached` |
| `access.no_rule` | error | `method`, `route` |
| `access.pdp_failed`, `access.pdp_invalid_answer`, `access.ceiling_unenforceable` | warn | `action`, `resource`, `error` |
| `http.request` | info | `method`, `route`, `status`, `duration_ms`, `subject` |
| `document.write` | info | `key`, `kind`, `version`, `etag_before`, `etag_after`, `paths`, `subject` |
| `secret.write` | info | `scope`, `name`, `action`, `fingerprint`, `subject` |
| `store.entry_ignored`, `store.external_change`, `store.repaired` | warn | 2.33, 2.19 |
| `catalog.limit_exceeded` | error | `limit` |
| `run.queued`, `run.started`, `run.finished` | info | `run`, `kind`, `mode`, `node`, `hosts`, `status`, `reason`, `snapshot`, `subject` |
| `run.cancel` | info | `run`, `subject` |
| `run.adopted`, `run.lost` | warn | `run`, `reason` |
| `runner.lease_lost`, `runner.store_error` | error | `error` |

### 6.28 The metrics

`GET /metrics` of the ops listener gives these metrics in the Prometheus text format. A label never holds a subject, an id of a node or a value.

| Metric | Type | Labels |
|---|---|---|
| `pupitre_http_requests_total` | counter | `route`, `status` |
| `pupitre_http_request_seconds` | histogram | `route` |
| `pupitre_pdp_requests_total` | counter | `result` (`allow`, `deny`, `error`) |
| `pupitre_pdp_cache_hits_total` | counter | |
| `pupitre_store_errors_total` | counter | `method` |
| `pupitre_catalog_documents` | gauge | `kind` |
| `pupitre_runs_total` | counter | `kind`, `mode`, `status` |
| `pupitre_runs_queued` | gauge | |
| `pupitre_runs_running` | gauge | |
| `pupitre_runs_oldest_queued_seconds` | gauge | |
| `pupitre_telemetry_spans_dropped_total` | counter | |

### 6.29 The spans

With `telemetry.enabled = true`, Pupitre exports spans with OTLP over HTTP to `<endpoint>/v1/traces`, at the period `telemetry.export_interval`. The resource has `service.name` equal to `telemetry.service_name`. The spans are `http.request`, `access.decide`, `store.<method>`, `run.snapshot` and `run.child`. A span attribute never holds a secret or a value of a document. A queue of `telemetry.max_queued_spans` spans waits for the export. Above it, Pupitre drops the new span and counts it (6.28).
