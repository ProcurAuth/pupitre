# Development

This page tells how to build, test and release Pupitre. Part 8 of the [specification](08-delivery.md) is the authority. Each build and each test runs in Docker. Nothing is installed on the host, except Docker, `git` and `python3` for the scripts of the documents.

## The repository

| Folder | Content |
| --- | --- |
| `crates/hclconf` | The strict HCL engine, a copy of the engine of Hune |
| `crates/pupitre-core` | Ids, the canonical form, the documents, the schemas, the layers and the snapshot manifest. It does no I/O. |
| `crates/pupitre-store` | The `Store` trait, the backends `files` and `postgres`, the cipher and the catalog |
| `crates/pupitre-runner` | The snapshot builder, the process control, the event reader and the scrubber |
| `crates/pupitre` | The command line, the configuration, the access guard, the HTTP server and the page |
| `docs` | The specification, the guides and the site |
| `examples` | An example deployment with synthetic schemas and playbooks |
| `scripts` | The build, the checks, the static build, the site builder and the documents check |
| `tools` | The ASD-STE100 lint, the third-party notices, the changelog reader and the local release |
| `tests` | The fixtures and the tests of the documents |

## Build and test

The script `scripts/cargo.sh` runs `cargo` in the development image. The image comes from `scripts/Dockerfile.dev`, and its base image is pinned by digest. The cargo registry and the target folder are named Docker volumes. A worktree has its own target volume.

```sh
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
scripts/cargo.sh test -p pupitre-core --locked
scripts/cargo.sh test --workspace --locked
scripts/cargo.sh test --workspace --all-features --locked
scripts/static.sh
```

`scripts/static.sh` builds the static musl binary for the architecture of the host. It checks that the file is static, that `ldd` finds no library, and that `pupitre --version` runs. It never crosses an architecture.

## The checks

| Script | Content |
| --- | --- |
| `scripts/check.sh` | Format, lint, tests and the checks of the documents |
| `scripts/check.sh --rust` | Format, lint and tests only |
| `scripts/check.sh --docs` | The checks of the documents only |
| `python3 -I scripts/check-docs.py` | The links, the site, ASD-STE100, the hygiene and the versions |
| `tools/check-examples.sh` | `pupitre check-config` on each complete configuration of the documents and of `examples` |
| `python3 -I tools/ste_lint.py <file>` | The ASD-STE100 rules on a Markdown file |
| `tools/third-party.sh --check` | The file `THIRD-PARTY-NOTICES.md` agrees with the crate graph |
| `tests/ci/store-pg.sh` | The tests of the backend `postgres` on PostgreSQL 15 and 18, in a throwaway container |
| `tests/ci/acceptance.sh` | The acceptance tests: a real Ansible and host containers. Docker is necessary. |
| `tests/conformance/run.sh` | The series U of the ProcurAuth conformance suite. It needs a local checkout of `ProcurAuth/procurauth`. |

Run `python3 -I scripts/check-docs.py links` to run one check. The option `--online` also asks each public URL with an HTTP request. Without it, the script makes no request and maps each public URL to a local file.

## The documents and the site

The Markdown files in `docs/` and the file `CHANGELOG.md` are the source of the website. The script `scripts/build-site.py` makes the HTML pages from them. The pages use no external request, and they work from `file://`. After you change a Markdown file, make the pages again:

```sh
python3 -I scripts/build-site.py
python3 -I scripts/build-site.py --check
```

The second command changes nothing. It exits with the code 1 when a page differs from its source. The check of the documents runs it.

Write all documents in ASD-STE100 Simplified Technical English. A procedural sentence has 20 words at most. A descriptive sentence has 25 words at most. A paragraph has six sentences at most. The rules are in the guide for contributors: [`CONTRIBUTING.md`](../CONTRIBUTING.md).

The pictures of the site are in `docs/assets`. The schemas are SVG files with their own dark background, so they read well in both themes. The screenshots are WebP files in `docs/assets/img`. A page of the site shows a screenshot only when its file exists.

## The configuration reference

The table of keys in `crates/pupitre/src/config/keys.rs` is the source of the reference. The command `pupitre print-reference --format markdown` generates `docs/configuration-reference.md`. A test compares the file with the output. Do not edit the file by hand.

## Dependencies

Each direct dependency has an exact version, `default-features = false` where the crate permits it, and one line with its reason in the workspace `Cargo.toml`. A new dependency needs a written reason in the same commit. Code of the project is the first choice. `deny.toml` lists the permitted licences, the permitted sources and the forbidden crates.

The file `THIRD-PARTY-NOTICES.md` lists each crate of the graph with its licence text. Make it again with `tools/third-party.sh` after a change of `Cargo.lock`. The script runs `cargo metadata` in Docker, with no network.

## The CI

Each job is short. The heavy checks run in the nightly workflow or in a local script.

| Workflow | Job | Content |
| --- | --- | --- |
| `ci.yaml` | `no-image` | The check that no script pushes an image |
| `ci.yaml` | `rust` | `scripts/check.sh --rust` |
| `docs.yml` | `docs` | `scripts/check.sh --docs` |
| `nightly.yaml` | `static` | `scripts/static.sh --archive` on amd64 and arm64 |
| `release.yaml` | `check`, `ci`, `build`, `release` | The release of the next section |

## Release

The project publishes static Linux binaries only: one archive for amd64 and one for arm64, with a file `SHA256SUMS`. It never publishes a container image. Two builds of one commit give the same bytes.

A release has these steps:

1. Set the version in `Cargo.toml`, and add the section `## x.y.z - YYYY-MM-DD` to `CHANGELOG.md`.
2. Run `scripts/check.sh` on the commit.
3. Start the workflow `release.yaml` by hand on `main` with the input `version`.

The workflow checks the version and the changelog, then runs `ci.yaml`. It builds the archives on a runner of each architecture and makes `SHA256SUMS`. It makes a draft release, checks the draft, and publishes it. A tag moves forward only. After a failed release, the next release uses the next patch version.

When the hosted CI cannot run, the script `tools/release-local.sh` makes the same release from a local machine. It runs `scripts/check.sh`, builds the archive of its architecture with `scripts/static.sh --archive`, and checks the content and the checksum of each archive. It then makes the draft with `gh`, checks it and publishes it. The notes of such a release say that the hosted CI did not run.

```sh
tools/release-local.sh 0.1.0 --dry-run
tools/release-local.sh 0.1.0
```

The option `--dry-run` makes each step that needs no network and prints the commands that follow. The option `--amd64-only` releases the amd64 archive only. The option `--partial-gate TEXT` skips `scripts/check.sh`. The text then goes in the notes, and it must say which checks ran and which did not. The script pushes no commit, no tag and no image.

## Versions

The versions follow Semantic Versioning. Before 1.0, a minor version can change a key, a route or a format. A change that a user can see, or that breaks compatibility, is a minor version. A fix is a patch. `CHANGELOG.md` has the section `## Unreleased` and one section for each version, with the groups Added, Changed, Removed, Fixed and Security. A change of a contract adds its line and its test in the same commit.
