# Configuration

Pupitre reads one HCL file. The file is the only source of the configuration. A mistake stops the start before Pupitre opens a port, and the message names the key. This page tells the rules. [The reference](configuration-reference.md) lists each key. Part 6 of the [specification](06-configuration.md) is the authority.

## The nine rules

| Rule | What Pupitre does |
| --- | --- |
| Closed schema | An unknown block, an unknown attribute, a duplicate attribute and a duplicate label stop the start. |
| Fail-fast | The error message names the key, the expected type or range, and the value. |
| No coercion | Pupitre never changes a value. The text `"5"` is not the integer `5`. |
| Typed once | Pupitre parses the file one time at start into one typed structure. |
| Single source | Pupitre reads an environment variable only when the file names it. No `OTEL_*` variable has an effect. |
| `_env` and `_file` | A key can take its value from a variable or a file, but only when the file says so. |
| Documented defaults | A key that has a default can be absent. Each other key that applies is necessary. |
| Effective configuration | `print-config` prints the value and the source of each key. `serve` logs the same lines. |
| Contradictions | Two keys that contradict each other stop the start (see "Contradictions"). |

## Syntax

The file is a small subset of HCL. It has blocks, labelled blocks and attributes. A value is a literal: a text in double quotes, a whole number, `true`, `false` or a list in brackets. The engine refuses expressions, functions, variables, heredocs, objects and `null`.

```hcl
# A comment
log {
  level = "info"
}

schema "relay" {
  path = "/srv/ansible/schemas/relay.json"
}
```

| Kind | Syntax | Examples | Refused |
| --- | --- | --- | --- |
| Duration | A text with one whole number and one unit: `ms`, `s`, `m` or `h` | `"500ms"`, `"30s"`, `"5m"` | `"1m30s"`, `"1.5s"`, `30` |
| Size | A whole number of bytes | `1048576` | `"1MiB"` |
| Integer | A whole number with no sign and no zero at the start | `16` | `16.0`, `"16"` |
| Boolean | `true` or `false` | `true` | `"true"`, `yes` |
| Text | Double quotes | `"deploy"` | `deploy` |
| List | Texts in brackets | `["soft", "hard"]` | |

## Values from the environment or from a file

A key that accepts the indirection has three forms. Give exactly one of them.

```hcl
storage {
  backend         = "files"
  secret_key_file = "/run/secrets/pupitre/store-key"
}
```

- `<name> = "..."`: the value is in the file.
- `<name>_env = "VAR"`: the value is in the environment variable `VAR`.
- `<name>_file = "/path"`: the value is the content of the file, without the last line end.

A variable that is not set, an empty value, or a file that Pupitre cannot read stops the start. The error gives the key and the variable or the path, never the value. A secret key accepts only `_env` and `_file`. A secret in clear text stops the start.

| Key | Forms | Secret |
| --- | --- | --- |
| `storage.secret_key` | `_env`, `_file` | yes |
| `storage.postgres.password` | `_env`, `_file` | yes |
| `access.pdp.token` | `_env`, `_file` | yes |
| `storage.postgres.user`, `access.identity.public_keys`, `access.pdp.url`, `telemetry.otlp_endpoint` | value, `_env`, `_file` | no |

Each other key accepts only the value in the file.

## The blocks

| Block | Content |
| --- | --- |
| `server` | The two listeners, the base path, the allowed `Host` values and the limits of the HTTP server |
| `log` | The lowest log level |
| `telemetry` | The OTLP export of the spans. Pupitre sets each value from the file. |
| `storage` | The backend `files` or `postgres`, the key of the store, and the block of the backend |
| `access` | The mode `open` or `authzen`. The mode `authzen` needs the blocks `identity` and `pdp`. |
| `runner` | The runs: the limits, and the keys of the runner node |
| `limits` | The counts of the store and the size of a document. The block is optional. |
| `schema "<id>"` | The path of the schema file of a unit |
| `playbook "<id>"` | The kind and the path of a playbook |
| `command "<id>"` | A named action with typed parameters (see "Commands") |

The block `access` has no default mode. So no instance gets the mode `open` by error. A key that sets exposure, identity, storage, access or the power of the runner has no default.

## The profiles

The program writes a complete file for each profile. The file holds each key that applies, with a reference value.

```sh
pupitre print-reference --format hcl --profile local
pupitre print-reference --format hcl --profile distributed
```

- The profile `local` has the backend `files`, the mode `open`, the runner node, one schema and one playbook.
- The profile `distributed` has the backend `postgres` with TLS, the mode `authzen`, the runner node, one schema and one playbook.

The reference values are examples, not defaults. The folder `examples/local` has a complete file with synthetic schemas and playbooks.

## Schemas, playbooks and commands

The configuration file holds the catalog of capabilities. The store never holds a part of it. A person who edits values cannot add a key, a playbook or a command.

- A block `schema` names a schema file. The file lists the keys of a unit, their kinds and their ranges. It also names the Ansible variable and the inventory group of the unit.
- A block `playbook` has the kind `deploy`, `render`, `status` or `command`, and the absolute path of the playbook file.
- A block `command` has a `label`, a `playbook`, a `target` (`environment`, `unit` or `host`), `check`, and up to 16 blocks `param`.

```hcl
command "restart-relay" {
  label    = "Restart the relay"
  playbook = "restart-relay"
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

A parameter has the type `text`, `integer`, `boolean`, `duration` or `choice`. The type decides the other keys of the block: `min` and `max`, `min_duration` and `max_duration`, or `choices`. A key that the type does not take stops the start. The playbook of a command has the kind `command`. A command never runs a program that is not the configured `ansible-playbook`.

With `check = true`, an apply of the command needs a check run of the same snapshot. With `check = false`, the command runs after the person confirms it.

## The checks

The checks run in three stages. A stage runs only when the stage before it has no error.

1. Stage 1 checks the file against the table of keys: the blocks, the types, the ranges and the forms. It reads each `_env` and `_file` value.
2. Stage 2 checks the values: the paths, the URLs, the CA files, the schema files and the ids.
3. Stage 3 checks the contradictions.

The runner node runs more checks at the start of `serve`. They cover the SSH key file, the known hosts file, the folder of the secrets on `tmpfs` and the work folder. They also cover the program `ansible-playbook`, which MUST be ansible-core 2.19 or later. Each line of the known hosts file names a host by `<environment>.<host>`, for example `production.host-a`, not by its address.

## Allowed host names

`server.allowed_hosts` lists the values of the header `Host` that the main listener serves. It stops an attack that points a name of the attacker to the listener. A request with another `Host` gets `421 misdirected_request`.

- An entry is a host name, an IPv4 address or an IPv6 address in brackets, with an optional port. It has no wildcard. An entry with no port matches each port.
- With a loopback `server.listen`, the key is optional. The default is `localhost`, `127.0.0.1`, `[::1]` and the address of `server.listen`. `print-config` shows it with the source `default`.
- With any other address, the key is necessary. The absence of the key stops the start. List the public name of the service, for example `["pupitre.example.invalid"]`.
- The reverse proxy MUST send the public name in `Host`.

## Contradictions

These pairs stop the start:

- `server.ops_listen` has the port of `server.listen`.
- `storage.backend = "files"` with `runner.enabled = true` and `runner.execute = false`.
- A block `playbook` or `command` with `runner.enabled = false`.
- The `playbook` of a command is not a block `playbook` with the kind `command`.
- Two blocks `playbook` with the same `path`.
- A folder of the store or of the runner is inside another one.
- `access.pdp.plain_http` does not agree with the scheme of `access.pdp.url`.
- Two schemas give one key path two kinds, or two schemas have the same `variable` or `group`.
- `access.options_max_questions` is below 8 with `runner.enabled = true`, or below 4 with `runner.enabled = false`, in the mode `authzen`.

Section 6.14 of the specification has the full list.

## The commands

| Command | Function |
| --- | --- |
| `serve --config <path>` | Start the server. |
| `check-config --config <path>` | Run the three stages. It sends nothing on the network. |
| `print-config --config <path>` | Print the effective configuration with the source of each value. |
| `print-reference --format hcl --profile local` or `--format markdown` | Print a reference file or the reference document. |
| `migrate --config <path>` | Make or change the store schema. It never runs at start. |
| `import --config <path> --inventory-json <path> --environment <id> [--write]` | Make the documents of a new environment from an Ansible inventory. |
| `export --config <path> --to <folder>` | Write the documents and their versions into an empty folder. |
| `copy-store --from <path> --to <path>` | Copy a store to an empty store. Each option names a configuration file. |
| `version`, `help` | Print the version or the usage. |

Each path of an option is absolute. A relative path, an unknown option and an unknown command give the exit code 2. A configuration error or a failure gives the exit code 1.

## Read the effective configuration

`print-config` prints one line for each key that has a value:

```text
server.listen = "127.0.0.1:8080"  # file
server.max_connections = 1024  # default
storage.secret_key = <hidden>  # file:/run/secrets/pupitre/store-key
```

`serve` writes the same values in the log line `server.config`, with the secrets hidden. A change of the file has an effect only after a restart.
