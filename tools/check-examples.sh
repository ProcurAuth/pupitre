#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Check that each complete configuration of the documents and of examples/ passes `pupitre check-config` and
# `pupitre print-config`. Cargo and the check run in Docker, in a container with no network.
# Use: tools/check-examples.sh
# A complete configuration is a file examples/*/pupitre.hcl, or a block ```hcl of a Markdown file that has a block
# `server`. The check moves each absolute path of a file under a temporary folder and makes the files that the
# stages 1 and 2 read (scripts/check-docs.py, check `examples`).
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/../scripts/docker-env.sh"

"$root/scripts/cargo.sh" build -p pupitre --locked
docker run --rm --network none \
    --user "$uid:$gid" \
    --volume "$root:/work:ro" \
    --volume "$target_volume:/cargo-target:ro" \
    --workdir /work \
    --env HOME=/tmp \
    "$image" python3 -I scripts/check-docs.py examples --binary /cargo-target/debug/pupitre
