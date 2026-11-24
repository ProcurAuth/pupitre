#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build the static Linux binary with the musl target of the host architecture, inside Docker (8.3 to 8.6).
# The script accepts an x86_64 or aarch64 host. It stops on another architecture. It never crosses an architecture.
# Use: scripts/static.sh                      build, check that the file is static, and run --version
#      scripts/static.sh --archive VERSION    also build a second time in a clean target, compare the two files,
#                                             and write dist/pupitre_VERSION_linux_<arch>.tar.gz
#      scripts/static.sh --out DIR            build, then copy the binary to DIR/pupitre
# The binary is in the target volume, in /cargo-target/<triple>/release/pupitre.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/docker-env.sh"

case "$(uname -m)" in
    x86_64 | amd64) target=x86_64-unknown-linux-musl; arch=amd64 ;;
    aarch64 | arm64) target=aarch64-unknown-linux-musl; arch=arm64 ;;
    *) echo "unsupported architecture: $(uname -m) (use x86_64 or aarch64)" >&2; exit 2 ;;
esac

version=""
out_dir=""
if [ "${1:-}" = "--out" ]; then
    out_dir="${2:?use: scripts/static.sh --out DIR}"
    [ "$#" -eq 2 ] || { echo "use: scripts/static.sh --out DIR" >&2; exit 2; }
    mkdir -p "$out_dir"
    out_dir="$(cd "$out_dir" && pwd)"
elif [ "${1:-}" = "--archive" ]; then
    version="${2:?use: scripts/static.sh --archive VERSION}"
    printf '%s\n' "$version" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "bad version: $version" >&2; exit 2; }
elif [ "$#" -gt 0 ]; then
    echo "use: scripts/static.sh [--archive VERSION | --out DIR]" >&2
    exit 2
fi

ensure_image
ensure_volumes

# The tag is the start of the SHA-256 of the two recipes together. A change in one of them gives a new image.
static_image="pupitre-static:$(cat "$root/scripts/Dockerfile.dev" "$root/scripts/Dockerfile.static" | sha256sum | cut -c1-12)"
if ! docker image inspect "$static_image" >/dev/null 2>&1; then
    if ! grep -qE '^FROM pupitre-dev:' "$root/scripts/Dockerfile.static"; then
        echo "scripts/Dockerfile.static: no line 'FROM pupitre-dev:<tag>'" >&2
        exit 1
    fi
    sed -E "s|^FROM pupitre-dev:[^[:space:]]+|FROM $image|" "$root/scripts/Dockerfile.static" \
        | docker build --quiet --tag "$static_image" --file - "$root/scripts" >/dev/null
fi

# The time of the last commit is the time of each file of the archive (reproducible builds, 8.5).
epoch="$(git -C "$root" log -1 --format=%ct)"
# The paths of the build machine do not go into the binary.
rustflags="--remap-path-prefix=/work=/build/pupitre --remap-path-prefix=/cargo-home=/build/cargo"

build_in() {
    # $1: the target directory inside the volume.
    docker run --rm \
        --user "$uid:$gid" \
        --volume "$root:/work:ro" \
        --volume "$registry_volume:/cargo-home" \
        --volume "$target_volume:/cargo-target" \
        --workdir /work \
        --env CARGO_HOME=/cargo-home \
        --env CARGO_TARGET_DIR="$1" \
        --env HOME=/cargo-home \
        --env CARGO_TERM_COLOR=never \
        --env SOURCE_DATE_EPOCH="$epoch" \
        --env RUSTFLAGS="$rustflags" \
        "$static_image" bash -c "set -e
cargo build --release --locked --target $target -p pupitre
cd $1/$target/release
if ! file pupitre | grep -q 'static'; then echo 'not static: pupitre' >&2; exit 1; fi
if ldd pupitre 2>&1 | grep -q '=>'; then echo 'dynamic: pupitre' >&2; exit 1; fi
./pupitre --version
sha256sum pupitre"
}

first="$(build_in /cargo-target | tail -n 1 | cut -d' ' -f1)"
echo "pupitre $target sha256 $first"
if [ -n "$out_dir" ]; then
    docker run --rm --user "$uid:$gid" --volume "$target_volume:/cargo-target:ro" --volume "$out_dir:/out" \
        "$image" cp "/cargo-target/$target/release/pupitre" /out/pupitre
    exit 0
fi
[ -n "$version" ] || exit 0

second="$(build_in /cargo-target/second | tail -n 1 | cut -d' ' -f1)"
if [ "$first" != "$second" ]; then
    echo "the two builds differ: $first and $second" >&2
    exit 1
fi

name="pupitre_${version}_linux_${arch}"
mkdir -p "$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
docker run --rm --user "$uid:$gid" --volume "$target_volume:/cargo-target:ro" --volume "$stage/$name:/out" \
    "$image" cp "/cargo-target/$target/release/pupitre" /out/pupitre
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/THIRD-PARTY-NOTICES.md" "$root/README.md" "$stage/$name/"
chmod 0755 "$stage/$name/pupitre"
chmod 0644 "$stage/$name/"*.md "$stage/$name/LICENSE-"*
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner -C "$stage" -cf - "$name" \
    | gzip -n -9 >"$root/dist/$name.tar.gz"
(cd "$root/dist" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
cat "$root/dist/$name.tar.gz.sha256"
