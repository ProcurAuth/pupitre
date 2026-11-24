#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test the callback plugin pupitre_events (4.20), the tag !unsafe of the variable files (4.12,
# 7.3) and the limit of a run (4.17) with a real ansible-playbook.
#
# First the script runs the unit test of the runner that writes the variable files of the test of
# the tag to tests/ansible/out/unsafe. For each line of the matrix, the script then builds a local
# image from a pinned Python image with ansible-core installed by pip at a pinned version
# (2.19, 2.20 and 2.21: the versions that Pupitre supports, 7.8). It runs
# plugin_test.py, unsafe_test.py and limit_test.py in a container with no network. The events that Ansible wrote
# go to tests/ansible/out/<name>/events-*.jsonl. Then the script runs the Rust test
# real_ansible_events, which reads each line with the strict reader of the runner.
#
# The images are local. Nothing is pushed. If Docker, the base image or the network of the build
# is not available, the script says SKIPPED and exits with the code 0.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"

# Pupitre needs ansible-core 2.19 or later (7.3, 7.8): before 2.19 the tag !unsafe is lost when a
# playbook reads a value through hostvars. The matrix holds the supported lines.
# name | base image with its digest | ansible-core version
matrix=(
    "core-2.19|python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df|2.19.11"
    "core-2.20|python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df|2.20.11"
    "core-2.21|python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df|2.21.6"
)

# The control: a version that Pupitre refuses. Only the case of hostvars runs, and it must find the
# defect (the script unsafe_test.py with the word control). It proves that the case can fail.
# name | base image with its digest | ansible-core version
control=(
    "core-2.18|python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df|2.18.20"
)

skip() {
    echo "SKIPPED: the test of the plugin with a real ansible-playbook did not run: $1" >&2
    exit 0
}

command -v docker >/dev/null 2>&1 || skip "docker is not installed"
docker info >/dev/null 2>&1 || skip "the docker daemon does not answer"

rm -rf "$here/out"
mkdir -p "$here/out"

# The variable files of the test of the tag !unsafe, from the functions of the snapshot.
"$root/scripts/cargo.sh" test -p pupitre-runner --locked --lib -- --exact \
    snapshot::model::tests::the_files_of_the_test_with_a_real_ansible

image() {
    local name="$1" base="$2" core="$3"
    tag="pupitre-plugin-test:$name-$(sha256sum "$here/Dockerfile" | cut -c1-8)"
    if ! docker image inspect "$tag" >/dev/null 2>&1; then
        echo "build $tag ($base, ansible-core $core)" >&2
        docker build --quiet --tag "$tag" --build-arg "BASE=$base" --build-arg "ANSIBLE_CORE=$core" \
            --file "$here/Dockerfile" "$here" >/dev/null 2>&1 || skip "cannot build $tag: the base image or the network is not available"
    fi
}

for line in "${control[@]}"; do
    IFS='|' read -r name base core <<<"$line"
    image "$name" "$base" "$core"
    docker run --rm --network none --user "$(id -u):$(id -g)" --env HOME=/tmp \
        --volume "$root/crates/pupitre-runner:/pupitre-runner:ro" \
        "$tag" python3 -I /pupitre-runner/tests/ansible/unsafe_test.py /pupitre-runner/tests/ansible/out/unsafe control
done

for line in "${matrix[@]}"; do
    IFS='|' read -r name base core <<<"$line"
    image "$name" "$base" "$core"
    mkdir -p "$here/out/$name"
    docker run --rm --network none --user "$(id -u):$(id -g)" --env HOME=/tmp \
        --volume "$root/crates/pupitre-runner:/pupitre-runner:ro" \
        --volume "$here/out/$name:/out" \
        "$tag" python3 -I /pupitre-runner/tests/ansible/plugin_test.py /pupitre-runner/src/callback/pupitre_events.py /out
    docker run --rm --network none --user "$(id -u):$(id -g)" --env HOME=/tmp \
        --volume "$root/crates/pupitre-runner:/pupitre-runner:ro" \
        "$tag" python3 -I /pupitre-runner/tests/ansible/unsafe_test.py /pupitre-runner/tests/ansible/out/unsafe
    docker run --rm --network none --user "$(id -u):$(id -g)" --env HOME=/tmp \
        --volume "$root/crates/pupitre-runner:/pupitre-runner:ro" \
        "$tag" python3 -I /pupitre-runner/tests/ansible/limit_test.py
done

"$root/scripts/cargo.sh" test -p pupitre-runner --locked --test real_ansible_events
echo "plugin test, unsafe test and limit test: ok"
