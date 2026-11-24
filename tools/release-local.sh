#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Release of Pupitre from a local machine (8.10). The workflow release.yaml makes the same release on GitHub.
# A maintainer runs this script when the hosted CI is blocked. It needs no hosted CI.
#
# Use: tools/release-local.sh VERSION [--amd64-only] [--partial-gate TEXT] [--also-ran TEXT] [--dry-run]
#   VERSION            is x.y.z.
#   --amd64-only       expects and releases the amd64 archive only (a person with no arm64 machine).
#   --partial-gate TEXT  skips scripts/check.sh. TEXT MUST NOT be empty. It says which checks ran on the commit and
#                      which did not. The script writes it in the release notes under "Checks".
#   --also-ran TEXT    adds TEXT to the section "Checks" of the notes. Use it to name the checks that a person ran
#                      by hand (for example the tests of the stores) and the checks that did not run.
#   --dry-run          makes every step that needs no network: it checks the input and the archives, makes
#                      dist/SHA256SUMS and target/release-notes.md, and prints each gh command. It calls neither
#                      git ls-remote nor gh, and it publishes nothing.
#
# Steps:
#   1. Check the input: the branch is main and the commit is on origin; the work tree is clean; VERSION is the version
#      of Cargo.toml; CHANGELOG.md has a non-empty section `## VERSION - <date>`; the tag v<VERSION> does not exist.
#      Run scripts/check.sh on the commit, unless --partial-gate is given.
#   2. Make the archives. For the architecture of this machine, run `scripts/static.sh --archive VERSION` when the
#      archive is absent. The archive of the other architecture comes from a machine of that architecture: copy it
#      and its .sha256 file into dist/. Check the content of each archive and its checksum.
#   3. Make dist/SHA256SUMS and the notes. The notes are the section of the changelog and a section "Checks".
#   4. Make the draft release with gh (a draft has no tag), check its files, download them and check the sums.
#   5. Publish the draft with one call. The publish creates the tag.
#   When a step before the publish call fails, the script deletes the draft. After the publish call starts,
#   it never deletes the draft or the release: check the release and the tag by hand.
# Needs: git, tar, sha256sum, docker (scripts/check.sh, scripts/static.sh) and gh logged in (not with --dry-run).
# The script pushes no image, logs in to no registry and pushes no commit or tag.
set -euo pipefail

die() { echo "release-local: $*" >&2; exit 1; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

usage() { sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; }

version=""
arm64=true
partial=""
has_partial=false
also=""
dry_run=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --amd64-only) arm64=false ;;
        --dry-run) dry_run=true ;;
        --partial-gate)
            [ "$#" -ge 2 ] || die "--partial-gate needs a text"
            has_partial=true
            partial="$2"
            shift
            ;;
        --also-ran)
            [ "$#" -ge 2 ] || die "--also-ran needs a text"
            also="$2"
            shift
            ;;
        -h | --help) usage; exit 0 ;;
        -*) die "unknown option: $1" ;;
        *)
            [ -z "$version" ] || die "use: tools/release-local.sh VERSION [--amd64-only] [--partial-gate TEXT] [--also-ran TEXT] [--dry-run]"
            version="$1"
            ;;
    esac
    shift
done
[ -n "$version" ] || die "use: tools/release-local.sh VERSION [--amd64-only] [--partial-gate TEXT] [--also-ran TEXT] [--dry-run]"
if [ "$has_partial" = true ]; then
    [ -n "$(printf '%s' "$partial" | tr -d '[:space:]')" ] || die "--partial-gate needs a text that is not empty"
fi
if [ "$dry_run" = false ]; then
    command -v gh >/dev/null 2>&1 || die "gh is not installed"
fi

# ---- Step 1: check the input ----
printf '%s\n' "$version" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' || die "bad version: $version"
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "run this script on the branch main"
commit="$(git rev-parse HEAD)"
[ -z "$(git status --porcelain)" ] || die "the work tree is not clean"
cargo_v="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml)"
[ "$cargo_v" = "$version" ] || die "Cargo.toml has version $cargo_v, not $version"
grep -qE "^## $version - [0-9]{4}-[0-9]{2}-[0-9]{2}\$" CHANGELOG.md || die "CHANGELOG.md has no heading '## $version - <date>'"
tools/changelog-section.sh "$version" CHANGELOG.md >/dev/null || die "the section '## $version - <date>' of CHANGELOG.md is empty"
if git rev-parse --verify --quiet "refs/tags/v$version" >/dev/null; then die "the tag v$version exists"; fi
if [ "$dry_run" = false ]; then
    remote_main="$(git ls-remote origin refs/heads/main | cut -f1)"
    [ "$remote_main" = "$commit" ] || die "origin/main is at '$remote_main', not at $commit: push main first"
    rc=0
    git ls-remote --exit-code --tags origin "refs/tags/v$version" >/dev/null || rc=$?
    case $rc in
        0) die "the tag v$version exists on origin" ;;
        2) ;;
        *) die "git ls-remote failed with code $rc" ;;
    esac
else
    echo "release-local: dry run. The commit is not compared with origin, and no tag is looked up there."
fi
if grep -rnE 'ghcr''\.io|docker/login''-action|docker[[:space:]]+(pus''h|logi''n)\b|buildx[^#]*--pus''h|imagetools[[:space:]]+crea''te|packages: *wr''ite|push: *tr''ue' .github scripts; then
    die "workflows and scripts must not publish container images"
fi

if [ "$has_partial" = true ]; then
    echo "release-local: PARTIAL GATE. scripts/check.sh does not run. The notes will carry this text:" >&2
    printf '%s\n' "$partial" >&2
else
    scripts/check.sh
fi

# ---- Step 2: the archives ----
case "$(uname -m)" in
    x86_64 | amd64) host_arch=amd64 ;;
    aarch64 | arm64) host_arch=arm64 ;;
    *) die "unsupported architecture $(uname -m)" ;;
esac
if [ "$arm64" = true ]; then archs="amd64 arm64"; assets=3; else archs="amd64"; assets=2; fi
case " $archs " in *" $host_arch "*) ;; *) die "this machine is $host_arch, and --amd64-only needs an amd64 machine" ;; esac

mkdir -p dist
if [ ! -f "dist/pupitre_${version}_linux_${host_arch}.tar.gz" ]; then
    scripts/static.sh --archive "$version"
fi
for arch in $archs; do
    name="pupitre_${version}_linux_${arch}"
    [ -f "dist/$name.tar.gz" ] || die "dist/$name.tar.gz does not exist: run scripts/static.sh --archive $version on a machine of this architecture, and copy the files into dist/"
    [ -f "dist/$name.tar.gz.sha256" ] || die "dist/$name.tar.gz.sha256 does not exist"
    (cd dist && sha256sum --check --quiet "$name.tar.gz.sha256") || die "the checksum of dist/$name.tar.gz is wrong"
    listing="$(tar -tzf "dist/$name.tar.gz" | sort)"
    expected="$(printf '%s\n' "$name/" "$name/LICENSE-APACHE" "$name/LICENSE-MIT" "$name/README.md" "$name/THIRD-PARTY-NOTICES.md" "$name/pupitre" | sort)"
    [ "$listing" = "$expected" ] || die "the content of dist/$name.tar.gz is not the content of 8.4"
done

# ---- Step 3: SHA256SUMS and the notes ----
(cd dist && for arch in $archs; do sha256sum "pupitre_${version}_linux_${arch}.tar.gz"; done >SHA256SUMS && cat SHA256SUMS)
mkdir -p target
tools/changelog-section.sh "$version" CHANGELOG.md >target/release-notes.md
{
    printf '\n### Checks\n\n'
    printf 'The hosted CI did not run for this release. A local machine made the archives.\n\n'
    if [ "$has_partial" = true ]; then
        printf 'The full local gate (scripts/check.sh) did not run on this commit. A partial gate was chosen.\n\n'
        printf '%s\n' "$partial"
    else
        printf 'scripts/check.sh ran on this commit and passed.\n'
    fi
    if [ -n "$also" ]; then
        printf '\n%s\n' "$also"
    fi
    if [ "$arm64" = false ]; then
        printf '\nThe arm64 archive is not built (no arm64 machine).\n'
    fi
} >>target/release-notes.md

draft_files=""
for arch in $archs; do draft_files="$draft_files pupitre_${version}_linux_${arch}.tar.gz"; done
draft_files="$draft_files SHA256SUMS"

if [ "$dry_run" = true ]; then
    echo "release-local: dry run ends here. These steps would follow:"
    echo "  (cd dist && gh release create v$version --draft --target $commit --title 'Pupitre v$version' --notes-file ../target/release-notes.md$draft_files)"
    echo "  gh release download v$version, then sha256sum -c SHA256SUMS"
    echo "  gh release edit v$version --draft=false --latest"
    echo "release-local: dry run ok"
    exit 0
fi

# ---- Step 4: the draft, and its check ----
publishing=0
cleanup() {
    rc=$?
    if [ "$rc" -ne 0 ] && [ "$publishing" -eq 0 ] && gh release view "v$version" >/dev/null 2>&1; then
        gh release delete "v$version" --yes || true
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM
# draft_files holds the archives and SHA256SUMS, as release.yaml does. Its names have no space: the split on blanks is intended.
# shellcheck disable=SC2086
(cd dist && gh release create "v$version" --draft --target "$commit" --title "Pupitre v$version" \
    --notes-file ../target/release-notes.md $draft_files)
n="$(gh release view "v$version" --json assets --jq '.assets | length')"
[ "$n" = "$assets" ] || die "the draft has $n files, not $assets"
rm -rf target/release-check
mkdir -p target/release-check
gh release download "v$version" --dir target/release-check
(cd target/release-check && sha256sum --check SHA256SUMS)

# ---- Step 5: publish. After this call starts, the script never deletes the draft or the release. ----
publishing=1
gh release edit "v$version" --draft=false --latest
echo "release-local: v$version is published"
