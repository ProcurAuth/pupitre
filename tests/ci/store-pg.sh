#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Job `store-pg` (2.43, 8.12): the PostgreSQL tests of the store on PostgreSQL 15 and 18.
# Use: tests/ci/store-pg.sh [--versions "15 18"] [--tests "contract_postgres pg_writes ..."] [-- <test args>]
#   For each version, the script does these steps.
#   1. It starts one throwaway PostgreSQL container with TLS. The server listens on 127.0.0.1
#      inside the container only. The script publishes no port.
#   2. It makes the three accounts of the tests: an administrator, the owner of the tables
#      (the account of `migrate`) and the account of `serve` (the rights of 2.36 only).
#   3. It runs the tests of the crate `pupitre-store` that need a server, with `cargo test` in the
#      development image. The test container shares the network of the server container.
#   4. It removes the container and its volume.
#   The tests make one database for each test. The passwords are random and live in memory
#   and in one private temporary folder that the script removes.
#   The script touches only containers that it made. Their names start with `pupitre-test-pg`.
#   The images are pinned by digest. The script pulls an image only when the local cache lacks it.
#   Variables: PUPITRE_PG_VERSIONS (default "15 18").
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/scripts/docker-env.sh"

# The digests of the multi-architecture indexes. They are the pins of the Hune tests, checked again.
readonly IMAGE_PG_15="postgres:15-trixie@sha256:8cf673eb99f8b9042c9078ee4ca0e775428c77097e9d999bfc9ba4b4347f5733"
readonly IMAGE_PG_18="postgres:18.4-trixie@sha256:a02db8cac496f15b094798a38254f14d6e00741f709360e5e00bb6668ea31636"

versions="${PUPITRE_PG_VERSIONS:-15 18}"
tests="contract_postgres pg_writes pg_poll pg_secrets pg_schema pg_rights pg_tls pg_runs transfer_postgres"
extra=()
while [ $# -gt 0 ]; do
    case "$1" in
        --versions) versions="$2"; shift 2 ;;
        --tests) tests="$2"; shift 2 ;;
        --) shift; extra=("$@"); break ;;
        -h | --help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown option: $1" >&2; exit 2 ;;
    esac
done

image_of() {
    case "$1" in
        15) printf '%s\n' "$IMAGE_PG_15" ;;
        18) printf '%s\n' "$IMAGE_PG_18" ;;
        *) echo "error: no pinned image for PostgreSQL $1" >&2; return 1 ;;
    esac
}

run_id="$$"
work="$(mktemp -d "${TMPDIR:-/tmp}/pupitre-store-pg.XXXXXX")"
chmod 755 "$work"
containers=()

cleanup() {
    for c in "${containers[@]:-}"; do
        [ -n "$c" ] && docker rm --force --volumes "$c" >/dev/null 2>&1 || true
    done
    rm -rf "$work"
}
trap cleanup EXIT INT TERM

random_password() {
    head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n'
}

# One CA and one server certificate for `localhost`, and a second CA that signed nothing.
make_certs() {
    openssl req -x509 -newkey rsa:2048 -nodes -keyout "$work/ca.key" -out "$work/ca.pem" -days 2 \
        -subj "/CN=pupitre-test-ca" >/dev/null 2>&1
    openssl req -x509 -newkey rsa:2048 -nodes -keyout "$work/wrong-ca.key" -out "$work/wrong-ca.pem" -days 2 \
        -subj "/CN=pupitre-test-wrong-ca" >/dev/null 2>&1
    openssl req -newkey rsa:2048 -nodes -keyout "$work/server.key" -out "$work/server.csr" \
        -subj "/CN=localhost" >/dev/null 2>&1
    printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' \
        >"$work/server.ext"
    openssl x509 -req -in "$work/server.csr" -CA "$work/ca.pem" -CAkey "$work/ca.key" -CAcreateserial \
        -out "$work/server.crt" -days 2 -extfile "$work/server.ext" >/dev/null 2>&1
    chmod 644 "$work"/*
}

ensure_pg_image() {
    local ref="$1"
    if ! docker image inspect "$ref" >/dev/null 2>&1; then
        echo "pulling $ref"
        docker pull --quiet "$ref" >/dev/null
    fi
}

admin_pw="$(random_password)"
owner_pw="$(random_password)"
app_pw="$(random_password)"

# A script for the first start of the server: the image trusts 127.0.0.1 by default. The tests
# check the passwords, so the script makes the loopback ask for a password.
cat >"$work/10-hba.sh" <<'HBA'
#!/bin/sh
cat >"$PGDATA/pg_hba.conf" <<'RULES'
local all all trust
host all all 127.0.0.1/32 scram-sha-256
host all all ::1/128 scram-sha-256
RULES
HBA
chmod 755 "$work/10-hba.sh"

make_certs
ensure_image
ensure_volumes
# Run one cargo test command in the development image, in the network of the server container.
run_in_container() {
    docker run --rm --interactive --network "container:$name" \
        --user "$uid:$gid" \
        --volume "$root:/work" \
        --volume "$registry_volume:/cargo-home" \
        --volume "$target_volume:/cargo-target" \
        --volume "$work:/tls:ro" \
        --workdir /work \
        --env CARGO_HOME=/cargo-home --env CARGO_TARGET_DIR=/cargo-target --env HOME=/cargo-home \
        --env CARGO_TERM_COLOR=never \
        --env PUPITRE_TEST_PG_HOST=127.0.0.1 --env PUPITRE_TEST_PG_PORT=5432 \
        --env PUPITRE_TEST_PG_ADMIN_USER=postgres --env PUPITRE_TEST_PG_ADMIN_PASSWORD="$admin_pw" \
        --env PUPITRE_TEST_PG_OWNER_USER=pupitre_owner --env PUPITRE_TEST_PG_OWNER_PASSWORD="$owner_pw" \
        --env PUPITRE_TEST_PG_APP_USER=pupitre_app --env PUPITRE_TEST_PG_APP_PASSWORD="$app_pw" \
        --env PUPITRE_TEST_PG_CA_FILE=/tls/ca.pem --env PUPITRE_TEST_PG_WRONG_CA_FILE=/tls/wrong-ca.pem \
        --env PUPITRE_TEST_PG_SERVER_NAME=localhost --env PUPITRE_TEST_PG_VERSION="$v" \
        --env PUPITRE_TEST_CLI_DB_A=pupitre_cli_a --env PUPITRE_TEST_CLI_DB_B=pupitre_cli_b \
        "$image" cargo test --locked --offline "$@"
}

status=0
for v in $versions; do
    ref="$(image_of "$v")"
    ensure_pg_image "$ref"
    echo "== PostgreSQL $v"
    name="pupitre-test-pg$v-$run_id"
    containers+=("$name")
    # The server listens on 127.0.0.1 inside its own network namespace. The test container joins it.
    docker run --detach --name "$name" --env POSTGRES_PASSWORD="$admin_pw" \
        --volume "$work:/tls:ro" --volume "$work/10-hba.sh:/docker-entrypoint-initdb.d/10-hba.sh:ro" \
        --entrypoint bash "$ref" -c '
        mkdir -p /var/tls && cp /tls/server.crt /tls/server.key /var/tls/ &&
        chown postgres:postgres /var/tls/* && chmod 600 /var/tls/server.key &&
        exec docker-entrypoint.sh postgres -c listen_addresses=127.0.0.1 -c ssl=on \
            -c ssl_cert_file=/var/tls/server.crt -c ssl_key_file=/var/tls/server.key \
            -c fsync=off -c full_page_writes=off -c max_connections=300' >/dev/null
    i=0
    until docker exec "$name" pg_isready --host 127.0.0.1 --username postgres >/dev/null 2>&1; do
        i=$((i + 1))
        if [ "$i" -ge 90 ]; then
            echo "error: $name does not start" >&2
            docker logs "$name" >&2
            exit 1
        fi
        sleep 1
    done
    # The first server of the image stops after its init: wait for the final one.
    sleep 2
    until docker exec "$name" pg_isready --host 127.0.0.1 --username postgres >/dev/null 2>&1; do sleep 1; done
    docker exec "$name" psql --username postgres -v ON_ERROR_STOP=1 \
        -c "CREATE ROLE pupitre_owner LOGIN PASSWORD '$owner_pw'" \
        -c "CREATE ROLE pupitre_app LOGIN PASSWORD '$app_pw'" >/dev/null
    # The two databases of the test of the command line (`pupitre migrate`, `export`, `copy-store`).
    for db in pupitre_cli_a pupitre_cli_b; do
        docker exec "$name" psql --username postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE $db OWNER pupitre_owner" >/dev/null
        docker exec "$name" psql --username postgres --dbname "$db" -v ON_ERROR_STOP=1 \
            -c "ALTER DEFAULT PRIVILEGES FOR ROLE pupitre_owner IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO pupitre_app" \
            -c "ALTER DEFAULT PRIVILEGES FOR ROLE pupitre_owner IN SCHEMA public GRANT USAGE ON SEQUENCES TO pupitre_app" >/dev/null
    done
    args=()
    for t in $tests; do args+=(--test "$t"); done
    set +e
    run_in_container -p pupitre-store --features conformance "${args[@]}" \
        -- --include-ignored --test-threads=8 "${extra[@]}"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        run_in_container -p pupitre --test store_cmds_pg -- --include-ignored --test-threads=1 "${extra[@]}"
        rc=$?
    fi
    set -e
    [ "$rc" -eq 0 ] || status=1
    docker rm --force --volumes "$name" >/dev/null 2>&1 || true
done
exit "$status"
