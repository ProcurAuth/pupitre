# shellcheck shell=bash disable=SC2034
# SPDX-License-Identifier: MIT OR Apache-2.0
# Shared settings of the scripts that run Docker. Source this file. Do not run it.
# It sets: root, image, registry_volume, target_volume, uid and gid.
# The scripts that source this file use these variables, so SC2034 does not apply.

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The tag is the start of the SHA-256 of Dockerfile.dev. A changed recipe gives a new image.
image="pupitre-dev:$(sha256sum "$root/scripts/Dockerfile.dev" | cut -c1-12)"
registry_volume="pupitre-cargo-registry"
# One target volume for each checkout: a worktree never shares build files with another.
target_volume="pupitre-target-$(printf %s "$root" | sha1sum | cut -c1-8)"
uid="$(id -u)"
gid="$(id -g)"

# Build the development image one time for each version of Dockerfile.dev.
# The image is local. No script of this repository pushes an image.
ensure_image() {
    if ! docker image inspect "$image" >/dev/null 2>&1; then
        docker build --quiet --tag "$image" --file "$root/scripts/Dockerfile.dev" "$root/scripts" >/dev/null
    fi
}

# Create each volume one time, and give it to the host user with a root container.
ensure_volumes() {
    for volume in "$registry_volume" "$target_volume"; do
        if ! docker volume inspect "$volume" >/dev/null 2>&1; then
            docker volume create "$volume" >/dev/null
        fi
        owner="$(docker run --rm --user 0:0 --volume "$volume:/v" "$image" stat -c '%u:%g' /v)"
        if [ "$owner" != "$uid:$gid" ]; then
            docker run --rm --user 0:0 --volume "$volume:/v" "$image" chown -R "$uid:$gid" /v
        fi
    done
}
