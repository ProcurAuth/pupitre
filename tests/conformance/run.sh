#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The series U of the ProcurAuth conformance suite against the access layer of Pupitre (3.24, 8.12).
#
# Use: PROCURAUTH_DIR=<local checkout of ProcurAuth/procurauth> tests/conformance/run.sh
#
# The script never reads the network. It does these steps:
#   1. It reads the pinned commit of `procurauth-pep` from Cargo.toml (8.9).
#   2. It exports the source of that commit from the local checkout with `git archive`.
#      The checkout is only read. The export goes to a temporary folder that the script removes.
#   3. It builds the runner `access-conformance` in a container with no network. The cargo
#      registry of this build is a copy of the Docker volume `procurauth-cargo-registry`
#      (the variable CONFORMANCE_SEED_VOLUME names another volume). If the copy does not hold
#      each crate, the build stops: the script never fetches a crate.
#   4. It builds the service under test (the test `access_conformance_service`) in a container
#      with no network, then it runs the service and the runner in that container, on the
#      loopback address.
#   5. It compares the failed cases with `tests/conformance/known-failures.txt`. A required
#      case that fails and is not in the list stops the run with a non-zero code. A listed case
#      that passes also stops the run, so that the list never grows stale.
#
# No image is published. The development image is the local image of `scripts/Dockerfile.dev`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/scripts/docker-env.sh"
ensure_image

: "${PROCURAUTH_DIR:?set PROCURAUTH_DIR to a local checkout of the ProcurAuth repository}"
seed_volume="${CONFORMANCE_SEED_VOLUME:-procurauth-cargo-registry}"
conf_cargo="pupitre-conformance-cargo"
conf_target="pupitre-conformance-target"
listen="127.0.0.1:8000"

pin="$(grep -o 'rev = "[0-9a-f]\{40\}"' "$root/Cargo.toml" | head -1 | grep -o '[0-9a-f]\{40\}')"
if [ -z "$pin" ]; then
    echo "conformance: no pinned commit in Cargo.toml" >&2
    exit 2
fi
if ! git -C "$PROCURAUTH_DIR" cat-file -e "$pin^{commit}" 2>/dev/null; then
    echo "conformance: the checkout does not hold the pinned commit $pin" >&2
    exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/src" "$work/out"
git -C "$PROCURAUTH_DIR" archive "$pin" | tar -x -C "$work/src"
echo "conformance: pinned commit $pin exported from the local checkout"

# The registry of the build: a copy of the volume of the ProcurAuth repository, made one time.
for volume in "$conf_cargo" "$conf_target"; do
    docker volume inspect "$volume" >/dev/null 2>&1 || docker volume create "$volume" >/dev/null
done
if [ -z "$(docker run --rm --user 0:0 -v "$conf_cargo:/c:ro" "$image" ls -A /c)" ]; then
    if ! docker volume inspect "$seed_volume" >/dev/null 2>&1; then
        echo "conformance: the volume $seed_volume does not exist, so the runner cannot be built with no network" >&2
        exit 2
    fi
    docker run --rm --user 0:0 -v "$seed_volume:/from:ro" -v "$conf_cargo:/to" "$image" cp -a /from/. /to/
fi
docker run --rm --user 0:0 -v "$conf_cargo:/c" -v "$conf_target:/t" "$image" chown -R "$uid:$gid" /c /t
ensure_volumes

common=(--rm --network none --user "$uid:$gid" --env HOME=/cargo-home --env CARGO_TERM_COLOR=never)

echo "conformance: build the runner (no network)"
docker run "${common[@]}" \
    -v "$work/src:/src:ro" -v "$conf_cargo:/cargo-home" -v "$conf_target:/cargo-target" \
    --env CARGO_HOME=/cargo-home --env CARGO_TARGET_DIR=/cargo-target -w /src \
    "$image" cargo build --release --offline --locked -p access-conformance

echo "conformance: build the service under test, run it and run the series U (no network)"
docker run "${common[@]}" \
    -v "$root:/work" -v "$registry_volume:/cargo-home" -v "$target_volume:/cargo-target" \
    -v "$conf_target:/runner:ro" -v "$work/out:/out" \
    --env CARGO_HOME=/cargo-home --env CARGO_TARGET_DIR=/cargo-target -w /work \
    --env PUPITRE_CONFORMANCE_PROFILE=/work/tests/conformance/pep-profile.json \
    --env PUPITRE_CONFORMANCE_LISTEN="$listen" \
    "$image" bash -c '
        set -euo pipefail
        cargo test -p pupitre --test access_conformance_service --no-run --locked --offline 2>&1 | tail -3
        bin="$(ls -t /cargo-target/debug/deps/access_conformance_service-* | grep -v "\.d$" | head -1)"
        "$bin" --ignored --nocapture serve > /out/service.log 2>&1 &
        service=$!
        for _ in $(seq 1 100); do
            (exec 3<>/dev/tcp/127.0.0.1/8000) 2>/dev/null && break
            sleep 0.1
        done
        status=0
        /runner/release/access-conformance --role pep --profile /work/tests/conformance/pep-profile.json \
            --url http://'"$listen"' > /out/report.txt || status=$?
        kill "$service" 2>/dev/null || true
        echo "$status" > /out/status
    '

cat "$work/out/report.txt"
status="$(cat "$work/out/status")"
if [ "$status" -eq 2 ]; then
    echo "conformance: the runner stopped with a usage error or a target that it cannot reach" >&2
    cat "$work/out/service.log" >&2 || true
    exit 2
fi

# The failed cases against the list of the known failures.
known="$root/tests/conformance/known-failures.txt"
failed="$(grep -E '^U[0-9]+ +fail ' "$work/out/report.txt" | awk '{print $1}' | sort || true)"
listed="$(grep -oE '^U[0-9]+' "$known" 2>/dev/null | sort || true)"
unexpected="$(comm -23 <(printf '%s\n' "$failed" | grep . || true) <(printf '%s\n' "$listed" | grep . || true) || true)"
stale="$(comm -13 <(printf '%s\n' "$failed" | grep . || true) <(printf '%s\n' "$listed" | grep . || true) || true)"
code=0
if [ -n "$unexpected" ]; then
    echo "conformance: required cases that fail and are not known: $(echo "$unexpected" | tr '\n' ' ')" >&2
    code=1
fi
if [ -n "$stale" ]; then
    echo "conformance: cases in known-failures.txt that do not fail: $(echo "$stale" | tr '\n' ' ')" >&2
    code=1
fi
[ "$code" -eq 0 ] && echo "conformance: ok (known failures: $(echo "$listed" | grep -c . || true))"
exit "$code"
