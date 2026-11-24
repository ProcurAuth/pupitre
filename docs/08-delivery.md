# Pupitre specification, part 8: delivery

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 8. Delivery

### 8.1 The toolchain

- The file `rust-toolchain.toml` fixes the Rust version, the components `rustfmt` and `clippy`, and the targets `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
- Each crate uses the edition 2024, except `hclconf`. `hclconf` uses the edition 2021, because its one reader of the environment calls `std::env::remove_var`. This function is `unsafe` from the edition 2024.
- Each crate forbids `unsafe` code. Clippy runs with `-D warnings`. The workspace denies `dbg!`, `todo!` and `unimplemented!`.
- The code of the program has no `panic!`, `unwrap` or `expect` on data of a request, a document, a file or a child. A test can use them.

### 8.2 The build in Docker

Each build and each test runs in Docker. Nothing is installed on the host.

| Script | Role |
|---|---|
| `scripts/cargo.sh <args>` | Run `cargo <args>` in the development image, as the user of the host. The cargo registry and the target folder are named Docker volumes. |
| `scripts/check.sh` | The checks of the CI jobs `rust` and `docs`: format, lint, tests, the ASD-STE100 check, `scripts/check-docs.py` and the tests of `tests/docs` |
| `scripts/static.sh` | The static build of 8.3 |
| `scripts/check-docs.py` | The checks of the documents and of the site: links, site, ste, hygiene, versions and examples |
| `scripts/build-site.py` | The build of the static website in `docs/` from the Markdown files. With `--check`, it changes nothing. |
| `tools/check-examples.sh` | The check that each complete configuration of the documents and of `examples/` passes `check-config` and `print-config` |
| `tools/third-party.sh` | The generation of `THIRD-PARTY-NOTICES.md` from the resolved crates |
| `tools/release-local.sh` | The release from a local machine (8.10) |
| `scripts/docker-env.sh` | The shared settings of the scripts |

The development image comes from `scripts/Dockerfile.dev`. Its base image is pinned by digest. The tag of the image is the start of the SHA-256 of the recipe, so a changed recipe gives a new image. The image is local, and no script pushes it.

### 8.3 The static binary

`scripts/static.sh` builds `pupitre` for the musl target of the architecture of the host, in the image of `scripts/Dockerfile.static`. It never crosses an architecture: an amd64 runner builds amd64, and an arm64 runner builds arm64.

The script checks the result: `file` tells that the binary is static, `ldd` finds no library, and `pupitre --version` runs. A failure stops the script with a status that is not zero.

`scripts/static.sh --out <dir>` builds the binary in the same way, then copies it to `<dir>/pupitre`. The acceptance tests use this option (8.12).

### 8.4 The archive

`scripts/static.sh --archive <version>` writes `dist/pupitre_<version>_linux_<arch>.tar.gz` and its `.sha256` file. The archive holds one folder `pupitre_<version>_linux_<arch>/` with:

| File | Content |
|---|---|
| `pupitre` | The static binary, mode 0755 |
| `LICENSE-MIT`, `LICENSE-APACHE` | The two licence texts |
| `THIRD-PARTY-NOTICES.md` | The notices of the copied code and of the crates |
| `README.md` | The README of the repository |

### 8.5 Reproducible builds

Two builds of one commit give the same bytes.

- The script sets `SOURCE_DATE_EPOCH` to the time of the last commit.
- `RUSTFLAGS` maps the paths of the build machine to fixed paths with `--remap-path-prefix`.
- The release profile uses one codegen unit, `lto = "fat"`, no debug data and stripped symbols.
- The archive has the files in name order, the time of the last commit, the owner 0 and no gzip time.
- `--archive` builds two times, in two target folders, and compares the two binaries. A difference stops the script.

### 8.6 Checksums

The release has the file `SHA256SUMS`, with one line for each archive in the form of `sha256sum`. The release workflow downloads its own draft and checks each line before it publishes (8.10).

### 8.7 No container image

The project never publishes a container image. The CI job `no-image` and the first step of the release check the files of `.github` and `scripts`. They fail on a registry login, a push of an image, or the permission `packages: write`. The Dockerfiles of `scripts/` make local build images only.

### 8.8 The CI

Each job is short: seconds to tens of seconds, and rarely more than one minute. The heavy checks run in the nightly workflow or in a local script.

| Workflow | Job | Content |
|---|---|---|
| `ci.yaml` | `no-image` | The check of 8.7, with no Docker |
| `ci.yaml` | `rust` | `scripts/check.sh --rust` |
| `docs.yml` | `docs` | `scripts/check.sh --docs` |
| `nightly.yaml` | `static` | `scripts/static.sh --archive` on amd64 and arm64 |
| `release.yaml` | `check`, `ci`, `build`, `release` | 8.10 |

Each action is pinned to a full commit SHA with its version in a comment. Each checkout has `persist-credentials: false`. Each workflow has `contents: read`. Only the job `release` has `contents: write`.

### 8.9 Dependencies

The workspace `Cargo.toml` lists each direct dependency with an exact version, `default-features = false` where the crate permits it, and one line with its reason. A new dependency needs a written reason in the same commit. Code of the project is the first choice.

The ProcurAuth crates `procurauth-pep` and `procurauth-core` are git dependencies of `https://github.com/ProcurAuth/procurauth` at the commit `058563cfbc3536b9b850255d953568ef0a5cabbe`. This is the commit of Hune 0.1.1. A move of the commit needs a decision and a run of the conformance series U.

The other direct dependencies are the dependencies of Hune 0.1.1 at the same versions, except `ring`. `ring` is already in the graph through `rustls`. Pupitre uses it directly for ChaCha20-Poly1305, HKDF and HMAC (2.26, 2.39).

### 8.10 The release

A person starts the workflow `release.yaml` by hand on `main`, with the input `version`.

1. The job `check` checks that `version` has the form `x.y.z`, that `Cargo.toml` has this version, and that `CHANGELOG.md` has a non-empty section `## x.y.z - YYYY-MM-DD`. It checks that the tag `vx.y.z` does not exist, and it runs the check of 8.7.
2. The job `ci` runs `ci.yaml` on the commit.
3. The job `build` runs `scripts/static.sh --archive` on an amd64 runner and on an arm64 runner. The input `arm64 = false` builds amd64 only.
4. The job `release` makes `SHA256SUMS`, makes a draft release with the section of the changelog as notes (`tools/changelog-section.sh`), downloads the draft, checks `SHA256SUMS`, and publishes.

A tag moves forward only. After a failed release, the next release uses the next patch version. When the hosted CI cannot run, the same scripts make the archives on a local machine. The notes of such a release tell which checks did not run.

### 8.11 Versions and the changelog

- The versions follow Semantic Versioning. The first release is 0.1.0. Before 1.0, a minor version can change a key, a route or a format.
- A change that a user can see, or that breaks compatibility, is a minor version. A fix is a patch.
- `CHANGELOG.md` has one section for each version, and the section `## Unreleased` for the changes that wait for a release. The groups are Added, Changed, Removed, Fixed, Security and Known limits. A change of a contract adds its line and its test in the same commit.
- `pupitre --version` prints `pupitre <version>` and the licence `MIT OR Apache-2.0`.

### 8.12 Tests

| Level | Where | Content |
|---|---|---|
| Unit | The modules and the `tests/` folder of each crate | The rules of each paragraph, with the number of the paragraph in the name or the comment of the test |
| Configuration | `crates/pupitre/src/config/tests.rs` and `tests/fixtures/config/` | Each rule of section 6 with a small file: each key, each contradiction, `print-config`, `print-reference` |
| Store contract | `crates/pupitre-store/tests/contract_files.rs` and `contract_postgres.rs`, feature `conformance` | The cases S01 to S20 of 2.43. The suite for `files` is `contract_files.rs`, and it runs with `cargo test --all-features`. The suite for `postgres` is `contract_postgres.rs`, and `tests/ci/store-pg.sh` runs it. |
| Store backends | `crates/pupitre-store/tests/` | The tests of `files` (`files_*.rs`: the same cases with no feature, layout, outside changes, transfer, rights, size) and of `postgres` (`pg_*.rs` and `transfer_postgres.rs`). `tests/ci/store-pg.sh` runs the second group on PostgreSQL 15 and 18 in a throwaway container on `127.0.0.1`. |
| Commands | `crates/pupitre/tests/import_cli.rs` and `store_cmds_pg.rs` | The command `import`, and the commands `migrate`, `export`, `import` and `copy-store` |
| Access | `crates/pupitre/tests/access_*.rs` and `tests/conformance/` | The rules of section 3 with a fake PDP. `tests/conformance/run.sh` runs the series U of the ProcurAuth conformance suite at the commit of 8.9. |
| Server | `crates/pupitre/tests/server_*.rs` | The routes, the errors, the limits, the start and the ops listener. `server_docs.rs` compares the tables of sections 3 and 5 with the code. |
| Runner | `crates/pupitre-runner/tests/` | The snapshot bytes and hash with golden files, the guard, the scrubber, the events reader, the cancel and the adoption with a fake `ProcessControl`. `crates/pupitre-runner/tests/ansible/run.sh` runs a real Ansible in Docker. |
| Acceptance | `tests/acceptance/`, started by `tests/ci/acceptance.sh` | A real `ansible-playbook` in a container against a throwaway host container with systemd and SSH. A check, an apply, a command, a cancel and a restart of Pupitre during a run. |
| Two instances | `tests/acceptance/` | Two instances on one PostgreSQL. The runner node restarts during a run. A second runner node stops at start. |
| Documents | `tools/ste_lint.py`, `scripts/check-docs.py` and `tests/docs/` | The rules of ASD-STE100 on each Markdown file, the links, the site, the hygiene, the versions and the examples |

The hosted CI runs only the jobs of 8.8. The `postgres` tests, the acceptance tests and the series U need Docker or a local checkout. A maintainer runs them with the scripts of the table.

The fixtures are synthetic. The large fixture has one environment with 1,000 units, 1,000 hosts and 100 hosts in each unit, and 64 small environments. This is 65 environments, which is more than the default `limits.max_environments` of 64 (2.42). So the acceptance profile `large` sets `limits.max_environments` to 100. A test makes the fixture with a seeded generator, so no large file is in the repository.

Each defect gets a test that fails on the old code. A test is never made weaker to pass.
