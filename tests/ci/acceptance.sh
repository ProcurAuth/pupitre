#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The acceptance tests of 8.12: a real ansible-playbook in a container against throwaway host containers with systemd
# and sshd, and two instances of Pupitre on one PostgreSQL. Docker runs everything. The host needs Docker and python3.
#
# Use: tests/ci/acceptance.sh [--bin PATH] [OPTION...]
#
# Without --bin, the script builds the static binary with scripts/static.sh and takes it from the target volume.
# The other options go to tests/acceptance/run.py:
#   --stage images|configs|smoke|scenarios   a stage to run (repeat for more; the default is all)
#   --scenario NAME                          a scenario to run (repeat for more)
#   --require-server                         fail, do not skip, when `pupitre serve` is a stub
#   --remove-images                          remove the local images of the harness at the end
#   --seed N                                 the seed of the large fixture
#
# The script makes containers and one network with the name prefix `pupitre-acc-<run>-`, and it removes them at the end,
# also after an error. It checks that no container, volume or network of the run is left. It never pushes an image.
# `--cleanup RUN` removes what a run left after a kill of the script.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

bin=""
args=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --bin) bin="${2:?use: --bin PATH}"; shift 2 ;;
        *) args+=("$1"); shift ;;
    esac
done

# The unit tests of the harness and of the generator need no Docker daemon and no binary.
python3 -I -m unittest discover -s "$root/tests/acceptance/unit"

# A run of --cleanup or --list needs no binary.
for arg in "${args[@]:-}"; do
    case "$arg" in
        --cleanup | --list) exec python3 -I "$root/tests/acceptance/run.py" "${args[@]}" ;;
    esac
done

if [ -z "$bin" ]; then
    "$root/scripts/static.sh"
    # shellcheck source=../../scripts/docker-env.sh
    source "$root/scripts/docker-env.sh"
    case "$(uname -m)" in
        x86_64 | amd64) triple=x86_64-unknown-linux-musl ;;
        aarch64 | arm64) triple=aarch64-unknown-linux-musl ;;
        *) echo "unsupported architecture: $(uname -m)" >&2; exit 2 ;;
    esac
    out="$(mktemp -d)"
    trap 'rm -rf "$out"' EXIT
    # A throwaway container copies the binary out of the target volume.
    docker run --rm --name "pupitre-acc-extract-$$" --label pupitre-acc.extract=1 \
        --user "$uid:$gid" --volume "$target_volume:/t:ro" --volume "$out:/out" \
        "$image" cp "/t/$triple/release/pupitre" /out/pupitre
    bin="$out/pupitre"
fi

python3 -I "$root/tests/acceptance/run.py" --bin "$bin" "${args[@]}"
