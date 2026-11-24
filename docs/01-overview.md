# Pupitre specification

Version 0.1, 2026-11-24. This document uses ASD-STE100 Simplified Technical English.

The key words MUST, MUST NOT, SHOULD and MAY are as RFC 2119 and RFC 8174 define them. This applies when they are in capital letters.

The specification has eight files in `docs/`:

| File | Part | Content |
|---|---|---|
| `01-overview.md` | 1 Overview | Purpose, scope, architecture, crates, terms, fixed decisions |
| `02-model-and-storage.md` | 2 Model and storage | Ids, documents, canonical form, schemas, layers, the `Store` contract, the backends `files` and `postgres`, secrets, the catalog |
| `03-access.md` | 3 Access | The modes `open` and `authzen`, the resource tree, the actions, the questions of each route, the action document, the options route |
| `04-runner-and-commands.md` | 4 Runner and commands | Runs, the queue, the snapshot, the child process, events, the guard, commands |
| `05-api.md` | 5 API and page | The listeners, the routes, the bodies, the errors, the pages |
| `06-configuration.md` | 6 Configuration | The file, the blocks, the checks, the contradictions, the command line, the logs and the metrics |
| `07-security-and-limits.md` | 7 Security and limits | Threats, controls, headers, the limits and what Pupitre does not protect |
| `08-delivery.md` | 8 Delivery | The build, the tests, the CI, the dependencies and the release |

The file `configuration-reference.md` is part of section 6. The command `pupitre print-reference --format markdown` generates it from the table of keys in the code.

Rules of this document:

- A numbered paragraph `N.M` keeps its number in all later versions. A paragraph that leaves the document keeps its number with the word "Unused". A new paragraph between two others gets a letter, for example 2.13a.
- A cross-reference gives a paragraph number, for example "2.13" or "P 4.3". "P" means the ProcurAuth protocol, version 1.1.
- A code span holds an exact name: a key, a member, a route, a type, a status or a command.
- Each item is a rule. When the code and this document disagree, the document wins, and the code changes.

## 1. Overview

### 1.1 What Pupitre is

Pupitre is a web console in Rust. It is one static binary, `pupitre`. The binary serves a web page and an HTTP API, and it holds the command line tools.

Pupitre does three things:

1. It keeps the configuration values of services in layers: the schema default, the environment, the unit and the host (2.13). The page shows the value that applies and the layer that gives it.
2. It starts Ansible runs on a snapshot of these values: a check run first, then an apply run (section 4). An apply needs a check run with the status `passed` and the same snapshot hash (4.24).
3. It starts named commands that a person launches by hand (4.32). A command has typed parameters.

Pupitre has no scheduler. No run starts by itself. Each run starts from one HTTP request of a subject.

### 1.2 What Pupitre does not do

- Pupitre never writes Ansible code. The playbooks and the roles stay in a Git repository outside Pupitre, with their own review.
- Pupitre never runs a playbook that the configuration file does not list (4.2). No document of the store names a program, a path or a command.
- Pupitre has no cron, no schedule, no periodic run and no automatic run after another run.
- Pupitre decides no rollback. The roles do it.
- Pupitre has no login of its own. A reverse proxy guards the page in the mode `open`. ProcurAuth decides in the mode `authzen` (section 3).
- Pupitre makes no request to the internet. It connects only to the hosts of the configuration: the PDP, the PostgreSQL server, the OTLP collector, and the managed hosts through `ansible-playbook`.
- Pupitre has no MCP binding in this version. A key of an agent uses the HTTP API like a person (3.19).

### 1.3 The two choices

The configuration file sets two independent choices: the store and the access mode. Each of the four pairs is valid.

| | `storage.backend = "files"` | `storage.backend = "postgres"` |
|---|---|---|
| `access.mode = "open"` | One instance. A reverse proxy guards the page. | Two or more instances behind a proxy. Valid, but a folder is simpler. |
| `access.mode = "authzen"` | One instance. The folder can be a Git checkout. ProcurAuth decides. | Two or more instances. One instance is the runner node. ProcurAuth decides. |

- The backend `files` keeps the documents in a folder. One process holds a lock on the folder (2.32).
- The backend `postgres` keeps the documents in PostgreSQL. Two or more instances share the database. Exactly one instance executes the runs: the runner node (4.1, 4.8).

### 1.4 Architecture

```text
            Browser (plain HTML, CSS and JavaScript of the binary)
               |
            Edge or proxy  -------- PDP (mode authzen only)
               |                     ^
   +-----------v---------------------|-------------------------------------+
   | pupitre                         |                                     |
   |  HTTP server --> Access guard --+--> Catalog --> Store trait          |
   |  (main and ops listeners)            (memory)     |          |        |
   |                                                   files    postgres   |
   |  Runner (runner node only) --> Snapshot builder --> ansible-playbook  |
   +-----------------------------------------------------------|-----------+
                                                               | SSH
                                                             Hosts
```

| Part | Role |
|---|---|
| HTTP server | The main listener serves the page, the API and the two ProcurAuth routes (section 5). The ops listener serves `/livez`, `/readyz` and `/metrics` (5.20). |
| Access guard | In the mode `authzen`, it checks the identity token, asks the PDP and applies the ceilings (section 3). In the mode `open`, it asks nothing. |
| Catalog | It keeps the current documents in memory, with their etags and versions. It builds the chain of each node from its own data (P 4.5). It polls the store (2.41). |
| Store | One trait with two backends, `files` and `postgres`. Both pass the same contract suite (2.43). |
| Runner | Only on the runner node. It takes the queued runs, builds the snapshots, starts `ansible-playbook` and reads its events (section 4). |
| Page | Plain HTML, CSS and JavaScript in the binary. No framework, no build step, no CDN (5.21). |

### 1.5 Crates

The Cargo workspace has five crates:

| Crate | Kind | Content |
|---|---|---|
| `pupitre-core` | library, no I/O | Ids, strict JSON and canonical form, etag, documents, unit schemas, value kinds, layer resolution, nodes and actions, command parameters, run records, the snapshot manifest and its hash |
| `pupitre-store` | library | The `Store` trait, the enum `AnyStore`, the backends `files` and `postgres`, the secret cipher and the fingerprint, the catalog, the transfer of `export` and `copy-store`, the contract suite |
| `pupitre-runner` | library | The executor loop, the snapshot builder, the process control, the event reader, the scrubber and the callback plugin |
| `pupitre` | binary | The command line, the configuration, the access guard, the HTTP server, the log, the metrics and the telemetry |
| `hclconf` | library | The strict HCL engine and the table of keys (6.1) |

The dependency order is `hclconf`, `pupitre-core`, `pupitre-store`, `pupitre-runner`, then `pupitre`. No crate depends on a crate after it in this order. `pupitre-core` uses the value kinds of `hclconf` (2.12). Dispatch uses enums, not `dyn` trait objects.

### 1.6 Terms

| Term | Meaning |
|---|---|
| Environment | A set of hosts and units, for example `production`. The top node of the resource tree under the service (3.7). |
| Host | A machine of one environment, with an address. The inventory name of Ansible is its id. |
| Unit | One deployed service of one environment, on a list of hosts. A unit names a schema and its playbooks. |
| Schema | A closed list of keys of a unit, with types. A file that the configuration names (2.11). |
| Layer | One source of values for a key: `default`, `environment`, `unit` or `host` (2.13). |
| Effective value | The value that applies to a key on a host after the layers (2.13). |
| Secret | A value of a secret key of a schema. Pupitre writes it and never returns it (2.24). |
| Catalog of capabilities | The schemas, the playbooks and the commands of the configuration file (4.2). It is not in the store. |
| Run | One start of `ansible-playbook` on one snapshot, for one node and a list of hosts (section 4). |
| Snapshot | An immutable folder that the runner builds for a run, with its hash (4.9 to 4.16). |
| Runner node | The one instance with `runner.execute = true` (4.1). |
| Subject | The `sub` of the identity token in the mode `authzen`, or `-` in the mode `open`. |

### 1.7 Fixed decisions

These decisions are part of the product. A change needs a new version of this document.

1. The repository is `ProcurAuth/pupitre`, public, under `MIT OR Apache-2.0`.
2. Both stores, `files` and `postgres`, are in the first version, with the encrypted secrets of `postgres`.
3. Pupitre has no schedule. A person launches each run and each command.
4. The commands are in the configuration file, never in the store (4.32). A person who can launch a command cannot add one.
5. The runner never runs a playbook that is not in the catalog of capabilities (4.2).
6. The schemas of the units are files that the configuration names (2.11). A person who edits values cannot add a key.
7. The access mode is `open` or `authzen`, and the block `access` has no default (3.2).
8. The build and the tests run in Docker. The release is static musl archives with `SHA256SUMS`. No container image is published (section 8).

### 1.8 Sizes that the design supports

The design MUST support 64 environments, 1,000 units and 1,000 hosts in one environment, and 100 hosts in one unit. The limits of 2.42 bound each list. The test fixtures of 8.12 use these sizes.
