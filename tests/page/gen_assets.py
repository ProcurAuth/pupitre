#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Write the list ASSETS of `crates/pupitre/src/web_assets.rs` from the files of `web/`.

A development tool. Run it after a file of `web/` is added or removed. `check_page.py` fails when the list and the folder differ.

Use:
    python3 -I tests/page/gen_assets.py            rewrite the block in the Rust file
    python3 -I tests/page/gen_assets.py --check    exit 1 when the block is not up to date
"""

import os
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WEB = os.path.join(ROOT, "web")
RUST = os.path.join(ROOT, "crates", "pupitre", "src", "web_assets.rs")
BEGIN = "    // ASSETS BEGIN (written by tests/page/gen_assets.py)\n"
END = "    // ASSETS END\n"
TYPES = {".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".svg": "image/svg+xml", ".woff2": "font/woff2"}
SHELL = "shell.html"


def web_files():
    files = []
    for dirpath, _, names in os.walk(WEB):
        for name in names:
            rel = os.path.relpath(os.path.join(dirpath, name), WEB).replace(os.sep, "/")
            if rel != SHELL:
                files.append(rel)
    return sorted(files, key=lambda s: s.encode())


def content_type(rel):
    return TYPES.get(os.path.splitext(rel)[1], "text/plain; charset=utf-8")


def block():
    out = [BEGIN]
    for rel in web_files():
        out.append("    Asset { path: %s, content_type: %s, bytes: include_bytes!(%s) },\n" % (
            '"%s"' % rel, '"%s"' % content_type(rel), '"../../../web/%s"' % rel))
    out.append(END)
    return "".join(out)


def main():
    text = open(RUST, encoding="utf-8").read()
    a, b = text.find(BEGIN), text.find(END)
    if a < 0 or b < 0:
        sys.exit("The markers of the block are not in the Rust file.")
    new = text[:a] + block() + text[b + len(END):]
    if "--check" in sys.argv:
        if new != text:
            print("The list ASSETS is not up to date. Run tests/page/gen_assets.py.")
            return 1
        return 0
    open(RUST, "w", encoding="utf-8").write(new)
    return 0


if __name__ == "__main__":
    sys.exit(main())
