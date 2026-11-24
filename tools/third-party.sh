#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Make THIRD-PARTY-NOTICES.md of the archive (8.4) from `cargo metadata --locked`.
# Use: tools/third-party.sh            write THIRD-PARTY-NOTICES.md
#      tools/third-party.sh --check    write nothing; exit 1 when the file differs from the generated text
#
# The crates are the resolved graph of the two musl targets, outside the workspace. For each crate the file gives
# the name, the version, the licence expression and the text of its licence files (LICENSE*, LICENCE*, COPYING*,
# UNLICENSE*, NOTICE*). Crates with equal texts share one block. The notice of the copied code comes first.
# Cargo and python3 run in the development image, with no network: the sources are in the cargo volume that a
# build filled. Run `scripts/cargo.sh fetch --locked` first on a new checkout.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/../scripts/docker-env.sh"

mode="${1:-write}"
case "$mode" in
    write | --check) ;;
    *) echo "use: tools/third-party.sh [--check]" >&2; exit 2 ;;
esac

ensure_image
ensure_volumes

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

docker run --rm --interactive --network none \
    --user "$uid:$gid" \
    --volume "$root:/work:ro" \
    --volume "$registry_volume:/cargo-home" \
    --volume "$target_volume:/cargo-target" \
    --workdir /work \
    --env CARGO_HOME=/cargo-home \
    --env CARGO_TARGET_DIR=/cargo-target \
    --env HOME=/cargo-home \
    "$image" sh -c '
set -e
dir=$(mktemp -d)
for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    cargo metadata --locked --offline --format-version 1 --filter-platform "$t" >"$dir/$t.json"
done
python3 -I - "$dir"
' >"$tmp/crates.txt" <<'PY'
import glob, hashlib, json, os, sys

pkgs = {}
for f in sorted(glob.glob(os.path.join(sys.argv[1], "*.json"))):
    d = json.load(open(f))
    ids = {n["id"] for n in d["resolve"]["nodes"]}
    for p in d["packages"]:
        # A workspace crate has no source. Its text is the project licence, so it is not a third-party crate.
        if p["id"] in ids and p.get("source") is not None:
            pkgs[(p["name"], p["version"])] = p
blocks = {}
for (name, ver), p in sorted(pkgs.items()):
    root = os.path.dirname(p["manifest_path"])
    texts = []
    for fn in sorted(os.listdir(root)):
        if fn.upper().startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE")) and os.path.isfile(os.path.join(root, fn)):
            body = open(os.path.join(root, fn), encoding="utf-8", errors="replace").read().rstrip()
            texts.append("--- %s ---\n%s" % (fn, body))
    text = "\n\n".join(texts) if texts else "(no licence file in the crate; the licence expression above applies)"
    key = hashlib.sha256(text.encode()).hexdigest()
    blocks.setdefault(key, {"licenses": [], "crates": [], "text": text})
    lic = p.get("license") or p.get("license_file") or "unknown"
    if lic not in blocks[key]["licenses"]:
        blocks[key]["licenses"].append(lic)
    blocks[key]["crates"].append("%s %s" % (name, ver))
print("## Rust crates")
print()
print("The binary holds the crates below. The table gives the licence expression of each crate. The licence texts follow it.")
print()
print("| Crate | Version | Licence |")
print("| --- | --- | --- |")
for (name, ver), p in sorted(pkgs.items()):
    print("| `%s` | %s | %s |" % (name, ver, (p.get("license") or p.get("license_file") or "unknown").replace("|", "/")))
print()
print("## Licence texts")
print()
for key in sorted(blocks, key=lambda k: blocks[k]["crates"][0]):
    b = blocks[key]
    print("### " + b["crates"][0] + (" and %d more" % (len(b["crates"]) - 1) if len(b["crates"]) > 1 else ""))
    print()
    print("Crates: " + ", ".join(b["crates"]))
    print()
    print("Licence: " + "; ".join(b["licenses"]))
    print()
    print("```text")
    print(b["text"])
    print("```")
    print()
PY

{
    cat <<'HEAD'
# Third-party notices

Pupitre is under the licence `MIT OR Apache-2.0` (see `LICENSE-MIT` and `LICENSE-APACHE`). The binary also holds the code and the Rust crates below. Each one keeps its own licence.

No component has a licence that forbids `MIT OR Apache-2.0`. Each one is under a permissive licence. The file `deny.toml` lists the permitted licences, and `cargo deny check licenses` checks each crate of the graph.

## Copied code

| Component | Origin | Licence | Where |
| --- | --- | --- | --- |
| Configuration engine `hclconf` | Hune 0.1.1, [ProcurAuth/hune](https://github.com/ProcurAuth/hune) | MIT OR Apache-2.0 | `crates/hclconf/`, with its `PROVENANCE` file |
| Checks of the URLs and of the CA files | Hune 0.1.1 | MIT OR Apache-2.0 | `crates/pupitre/src/config/url.rs`, `crates/pupitre/src/config/ca.rs` |

The ProcurAuth crates `procurauth-pep` and `procurauth-core` are git dependencies of [ProcurAuth/procurauth](https://github.com/ProcurAuth/procurauth). They are under the licence Apache-2.0.

HEAD
    # The section of the files of the interface. The text of the font licence comes from the file of the font.
    python3 -I - "$root/tools/notices-interface.md" "$root/web/fonts/inter/LICENSE" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8").read()
licence = open(sys.argv[2], encoding="utf-8").read().rstrip("\n")
sys.stdout.write(text.replace("@@OFL@@", licence))
PY
    cat "$tmp/crates.txt"
} >"$tmp/notices.md"

if [ "$mode" = "--check" ]; then
    if ! cmp -s "$tmp/notices.md" "$root/THIRD-PARTY-NOTICES.md"; then
        echo "THIRD-PARTY-NOTICES.md differs from the generated text. Run: tools/third-party.sh" >&2
        exit 1
    fi
    echo "third-party: THIRD-PARTY-NOTICES.md is current"
else
    cp "$tmp/notices.md" "$root/THIRD-PARTY-NOTICES.md"
    echo "third-party: wrote THIRD-PARTY-NOTICES.md ($(grep -c '^| `' "$tmp/crates.txt") crates)"
fi
