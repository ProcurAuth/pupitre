# Specification

The specification of Pupitre has eight parts. Each part is a Markdown file in `docs/`. The specification and the code MUST agree. A difference is a defect. A fix changes the part or the code, and the paragraph numbers stay the same.

Each part uses ASD-STE100 Simplified Technical English. A numbered paragraph keeps its number in all later versions. The code and the tests cite the paragraph numbers.

## The parts

| Part | File | Content |
| --- | --- | --- |
| [Part 1: Overview](01-overview.md) | `01-overview.md` | Purpose, scope, architecture, crates, terms and fixed decisions |
| [Part 2: Model and storage](02-model-and-storage.md) | `02-model-and-storage.md` | Ids, documents, the canonical form, schemas, layers, the `Store` contract, the two backends, secrets and the catalog |
| [Part 3: Access](03-access.md) | `03-access.md` | The modes `open` and `authzen`, the resource tree, the actions, the questions of each route, the action document and the options route |
| [Part 4: Runner and commands](04-runner-and-commands.md) | `04-runner-and-commands.md` | Runs, the queue, the snapshot, the child process, events, the guard of an apply and commands |
| [Part 5: API and page](05-api.md) | `05-api.md` | The listeners, the routes, the bodies, the errors and the pages |
| [Part 6: Configuration](06-configuration.md) | `06-configuration.md` | The file, the blocks, the checks, the contradictions, the command line, the logs and the metrics |
| [Part 7: Security and limits](07-security-and-limits.md) | `07-security-and-limits.md` | Threats, controls, headers, the limits and what Pupitre does not protect |
| [Part 8: Delivery](08-delivery.md) | `08-delivery.md` | The build, the tests, the CI, the dependencies and the release |

The [configuration reference](configuration-reference.md) is part of section 6. The command `pupitre print-reference --format markdown` generates it from the table of keys in the code.

## Key words

The key words MUST, MUST NOT, SHOULD and MAY are as RFC 2119 and RFC 8174 define them, when they are in capital letters.

## Where to start

- To run Pupitre, read the [first guide](GETTING-STARTED.md) and [Operations](OPERATIONS.md).
- To write a configuration file, read [Configuration](CONFIGURATION.md) and part 6.
- To understand the values and the runs, read [Architecture](ARCHITECTURE.md), parts 2 and 4.
- To set the access, read [Access](ACCESS.md) and part 3.
- To review the security, read the [security model](SECURITY-MODEL.md) and part 7.
