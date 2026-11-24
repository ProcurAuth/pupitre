# Changelog

This file follows the layout of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and uses ASD-STE100 Simplified Technical English. The versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version can change a key, a route or the format of a document.

Each version has a section `## <version> - <YYYY-MM-DD>`. The newest version is first. The release workflow reads the section of the version as the notes of the release. The section `## Unreleased

## 0.1.0 - 2026-11-24

First release.

### Added

- One static binary `pupitre` with a web page inside. It edits the configuration of services on many hosts, and it starts Ansible runs and named commands from the page.
- The tree service, environment, unit and host. A unit gets its values from four layers: the default of its schema, the environment, the unit and the host. The page shows the layer of each value.
- Closed schemas of the unit that the operator gives in the configuration file. Each key has a type, a range and a default. A value that breaks its schema is refused.
- Two stores. The backend `files` keeps documents in folders, and it fits a review in Git. The backend `postgres` serves two instances or more, with TLS and an account that cannot change a table. Both keep a history of each document, with etags and a conflict answer.
- Secrets. A secret value never leaves the store through the API. The page shows its fingerprint only. The backend `files` keeps secrets in a separate folder, and the backend `postgres` encrypts them with a key file.
- The runner. A run is a check or an apply of one playbook on a snapshot of the values. An apply needs a check that passed, with the same snapshot hash, not older than `runner.check_max_age`. One runner node holds a lease for each environment. A run survives a restart of Pupitre.
- Commands. The operator lists named commands and their playbooks in the configuration file. A person starts a command by hand, with typed parameters that have ranges. No command runs by itself, and the product has no schedule.
- Two access modes. The mode `open` has no login and relies on a proxy. The mode `authzen` asks a ProcurAuth PDP for each request and publishes the action document.
- The commands `serve`, `check-config`, `print-config`, `print-reference`, `migrate`, `import`, `export`, `copy-store` and `version`. `import` reads an Ansible inventory as JSON. `copy-store` moves all data from one backend to the other.
- A configuration with a closed schema, fail-fast errors, no coercion, and the forms `_env` and `_file` for secrets. The reference of the keys is generated from the table of keys.
- The specification in eight parts, the guides, the website, and the example in `examples/local`.
- Static `musl` release archives with a `SHA256SUMS` file. Two builds give the same bytes. No container image exists.

### Security

- The runner node needs ansible-core 2.19 or later. It runs `runner.ansible_playbook --version` at start and stops with an older version. Before 2.19, the tag `!unsafe` is lost when a playbook reads a value through `hostvars`.
- The variable files of a snapshot are YAML. Each object and array of the values of documents and parameters has the tag `!unsafe`, so Ansible never evaluates a template in them. The server and the runner also refuse `{{`, `{%` and `{#` in a text value, a list item and a text parameter.
- Each run sets `HostKeyAlias=<environment>.<host>` and `CheckHostIP=no`. A host that gets the address of another host fails the key check. Each line of `runner.known_hosts_file` names a host by `<environment>.<host>`.
- The main listener serves a request only when its header `Host` is in `server.allowed_hosts`, else the answer is `421 misdirected_request`. A loopback listener has a default list. Any other listener needs the key.
- A request runs in its own task, so a client that closes the connection does not cancel a write after the store committed it.
- The runner node refuses a playbook whose folder holds an entry `group_vars` or `host_vars`. A schema cannot name its variable with a name that Ansible reserves. A host id or a host label cannot be `all`, `ungrouped` or `localhost`.
- A label, a note, a subject and the message of a run refuse the Unicode format characters, for example the bidirectional controls.
- The render phase writes its files in the private folder of the run on `tmpfs`. The runner node sets `PR_SET_DUMPABLE` to 0, so a child cannot read the environment of Pupitre.
- The scrubber replaces copies of a secret that overlap with one `[secret]`. It also finds a secret with a quote or a backslash after two layers of escapes.
- The fingerprint of a secret is part of the associated data of its encryption in the backend `postgres`. The poll of the backend `postgres` uses a barrier, so it does not miss a commit that arrives late.
- When the record of a run refuses a write, the runner stops the child and removes the private folder before the environment is free.

### Known limits

- The hosted CI runs the format, lint, unit tests and documents checks. The tests of the stores, the series U of the ProcurAuth conformance suite and the acceptance tests run by hand.
- The page checks in a browser run by hand, in Chrome only.
- `Store::update_run` compares the status only, so a cancel that arrives between two writes of the runner can be lost.
- See `SECURITY.md` for the limits that touch security.
