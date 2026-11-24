# Contribute to Pupitre

Thank you for your interest. This document tells how to propose a change.

## Ask a question or report a defect

Open an issue in [the repository](https://github.com/ProcurAuth/pupitre/issues). Give the version of Pupitre (`pupitre --version`), the store, the access mode and the part of the configuration that matters. Remove each secret and each real host name. Do not report a vulnerability in an issue. Read [SECURITY.md](SECURITY.md).

## Change the code

1. Open an issue first for a large change or a new feature. State the problem and the idea.
2. Read the specification in [`docs/`](docs/). The file [`docs/01-overview.md`](docs/01-overview.md) lists its parts.
3. Write a test for each change of behavior. Rust tests are in the `tests/` folder of each crate or in the module.
4. Run the checks that cover your change. Cargo runs in Docker. Nothing is installed on the host.

```sh
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
scripts/cargo.sh test -p pupitre-core --locked
python3 -I tools/ste_lint.py docs/01-overview.md
python3 -I scripts/check-docs.py
```

The script `scripts/check.sh` runs all the checks of the CI jobs `rust` and `docs`. The script `tools/check-examples.sh` runs `pupitre check-config` on each complete configuration of the documents. Read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the details.

## Rules of the code

- The specification in `docs/` is the reference. A change of behavior changes the paragraph that describes it. The code and the tests cite the paragraph numbers.
- Each dependency has an exact version in `Cargo.toml` and one line with its reason. Add a dependency only when the code that it replaces is larger or less strict.
- The program reads its configuration from the HCL file only. It reads an environment variable only when the file names it with an `_env` key. A change of the configuration changes the table of keys in `crates/pupitre/src/config/keys.rs`. Make the reference again with `pupitre print-reference --format markdown`.
- The program never runs a playbook that the configuration file does not list. No document of the store names a program or a path.
- The page has no framework, no build step and no request to another origin.
- The code forbids `unsafe`. The code has no `todo!`, no `unimplemented!` and no `dbg!`.
- Add `SPDX-License-Identifier: MIT OR Apache-2.0` as the first comment of each new Rust, script or test file.

## Rules for the documents

- Write in ASD-STE100 Simplified Technical English. A procedural sentence has 20 words at most. A descriptive sentence has 25 words at most. A paragraph has six sentences at most. Use the active voice and the imperative in a procedure. Run `python3 -I tools/ste_lint.py <file>`.
- The text must describe the real behavior of the code. Check the code, not the old text.
- The Markdown files in `docs/` and `CHANGELOG.md` are the source of the website. After you change one, run `python3 -I scripts/build-site.py`.
- A configuration example that has a block `server` must pass `pupitre check-config`. A test checks it.
- Add each user-visible change to the section `## Unreleased` of `CHANGELOG.md`, in the same commit.
- Use `example.org`, `example.invalid`, the names `host-a` and `host-b`, and the address range `203.0.113.0/24` in examples. Never write a real host name, a real address or a real secret.
- Do not add a container image to the release or to a registry. A release holds binaries only.

## Pull requests

Open a pull request against `main`. The CI must pass. Your contribution is under the licence of the project: `MIT OR Apache-2.0`.

## Code of conduct

This project follows [the code of conduct](CODE_OF_CONDUCT.md).
