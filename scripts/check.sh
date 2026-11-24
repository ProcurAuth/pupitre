#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The local gate of the CI job `rust` and of the job `docs` (8.8). Cargo runs in Docker; python3 runs on the host.
# Use: scripts/check.sh            format, lint, tests and the ASD-STE100 check of the documents
#      scripts/check.sh --rust     format, lint and tests only
#      scripts/check.sh --docs     the ASD-STE100 check of the documents only
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mode="${1:-all}"

run_rust() {
    "$root/scripts/cargo.sh" fmt --all -- --check
    "$root/scripts/cargo.sh" clippy --workspace --all-targets --all-features --locked -- -D warnings
    "$root/scripts/cargo.sh" test --workspace --locked
}

run_docs() {
    # Each Markdown file of the repository, except the code of conduct and the third-party notices.
    mapfile -t files < <(git -C "$root" ls-files --cached --others --exclude-standard '*.md' \
        | grep -vE '^(CODE_OF_CONDUCT|THIRD-PARTY-NOTICES)\.md$')
    (cd "$root" && python3 -I tools/ste_lint.py "${files[@]}")
    (cd "$root" && python3 -I scripts/check-docs.py)
    (cd "$root" && python3 -I -m unittest discover -s tests/docs)
    (cd "$root" && python3 -I tests/page/check_page.py)
    (cd "$root" && python3 -I tests/page/gen_assets.py --check)
}

case "$mode" in
    all) run_rust; run_docs ;;
    --rust) run_rust ;;
    --docs) run_docs ;;
    *) echo "use: scripts/check.sh [--rust | --docs]" >&2; exit 2 ;;
esac
echo "check: ok"
