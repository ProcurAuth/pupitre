# Security policy

This document tells how to report a vulnerability in Pupitre. It also tells what the project covers and the known limits. For the design of the protections, read the [security model](docs/SECURITY-MODEL.md) and [section 7](docs/07-security-and-limits.md) of the specification.

## Report a vulnerability

Do not open a public issue for a vulnerability. Use the private reports of GitHub.

1. Open [the page to report a vulnerability](https://github.com/ProcurAuth/pupitre/security/advisories/new).
2. Describe the problem. Name the version, the configuration that you used, and the steps to repeat it.
3. Add a test or a proof of concept if you have one. Remove each real secret and each real host name from it.

Only the maintainers of the repository can read your report. We never ask for an e-mail address or for a secret. If the page is not available, open a public issue with the title "Security contact needed". Write no detail in it. A maintainer then starts a private channel.

## Response times

These times are targets for a project that has no paid staff.

| Step | Target |
| --- | --- |
| We acknowledge your report. | 5 business days |
| We tell you if we accept it as a vulnerability. | 15 business days |
| We publish a fix and an advisory. | 90 days after the report, or sooner |

## Supported versions

| Version | Supported |
| --- | --- |
| The latest release (0.1.x) | Yes |
| The branch `main` | Yes |
| Older releases | No. Update to the latest release. |

## What the project covers

- **The runner.** A way to make Pupitre start a program other than `runner.ansible_playbook`, or a playbook outside the configuration file, is a vulnerability. So is a value of a document or a parameter that reaches a command line or a shell. So is such a value that Ansible evaluates as a template when a playbook only reads it. So is a run that reaches a machine with the host key of another host id.
- **The access policy.** A bypass of the check of the identity token is a vulnerability. So is a route that skips the question to the PDP, or a decision that Pupitre applies in the wrong way.
- **The secrets.** A secret value in an answer of the API, in the log, in a metric or in `print-config` is a vulnerability. So is a secret value in the stored output of a run, except the limits below.
- **The browser.** A script that runs from a document, a value or a log is a vulnerability. So is a write route that a page of another site can use.
- **The stores.** A read or a write outside the folders of the backend `files` is a vulnerability. So is a path traversal.
- **The release.** An archive that does not match its checksum, or a binary that is not static, is a defect.

## What the project does not cover

- A deployment that does not follow 7.2 of the specification. Examples are the mode `open` with no proxy, and an ops listener that the edge reaches.
- A weakness of a role, a playbook, Ansible, an edge or a PDP. Report it to that project.
- A person with `config.write` who changes what a role does. Her power is the power of the roles.

## Known limits

These limits are part of the design. Section 7.11 of the specification gives the full list.

1. A role that puts a value in a shell with no quotes can run it. A role that evaluates a value as a template on purpose can also run it. Review the roles.
2. The scrubber of the run output finds exact copies of a secret value and their JSON form. Another form, for example base64, stays in the output. Use `no_log` in each task that handles a secret.
3. The backend `files` keeps the secret values in clear text on the disk, with the mode 0600. The history of the backend `files` is as trusted as the folder. A person who can write the folder can write the history.
4. The snapshot hash covers the playbook file, not the roles. A change of a role between a check and its apply is not found.
5. A run survives a restart of Pupitre only when its child process survives it.
6. No route can wait for an approval of ProcurAuth. A key with a grant that has `confirm` cannot use that action through the API.
7. The mode `open` has no login. Pupitre trusts each request, and the proxy is the only guard.
8. `Store::update_run` compares only the status. A `cancel` that the server writes between the read and the write of the runner can be lost. Cancel the run again.
9. The runner has no store key. It reads the fingerprints of the secrets with `list_secrets` before and after `read_secrets`, and it tries again when they differ. A secret that changes from A to B and back to A between the two lists is not found.
10. A person who edits the store cannot add a playbook, a command or a schema. They are in the configuration file of the operator.
11. The child of a run has the user of Pupitre. It can read the secrets of the form `_file` and the key of the store. A separate user for the child is the stronger setup.

**RUSTSEC-2026-0387.** The crate `procurauth-core` uses `p256` 0.13 on `crypto-bigint` 0.5. Some operations of `crypto-bigint` 0.5 on secret values do not run in constant time. Pupitre uses `p256` only to check ES256 signatures with public keys, and it never signs. `deny.toml` ignores the advisory for this reason, until `procurauth-core` moves to `p256` 0.14.

## Test values

The repository holds test values that protect nothing. They are the two self-signed certificates of `tests/fixtures/config/`, with no private key, and the key `000102...1e1f` of the unit tests. The tests of the access layer also hold throwaway keys that protect nothing: `crates/pupitre/tests/access_data/server.pk8`, the two files of `tests/conformance/keys/` and `tests/conformance/pdp-credential.txt`. No service uses them.
