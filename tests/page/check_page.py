#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The static checks of the page (5.21 to 5.23, 7.5). Python standard library only. No browser, no server.

Use:
    python3 -I tests/page/check_page.py

Checks:
  1. The code builds no HTML from data: no innerHTML, outerHTML, insertAdjacentHTML, document.write, createContextualFragment,
     eval, new Function, srcdoc or setAttribute("style"). The checks read the code with the comments and the strings taken out.
  2. The shell has no inline script, no inline style, no style attribute and no event attribute, and it holds the two markers.
  3. No file names another origin. No file has a local path, an e-mail address or an IP address that is not a documentation address.
  4. Each file of web/ except shell.html is in the list ASSETS of crates/pupitre/src/web_assets.rs, in byte order, and the list has no other file.
  5. Each import of a module, each url() of a style and each static file of the shell exists.
  6. Each file that Pupitre copied has its origin in tests/page/origins.json. A byte copy has the sha256 of the record. A copy with edits has
     its origin in the first line.
  7. The sources are ASCII (the fonts and the notice of the font licence excepted).
Exit code: 0 when each check passes, 1 else.
"""

import hashlib
import json
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WEB = os.path.join(ROOT, "web")
RUST = os.path.join(ROOT, "crates", "pupitre", "src", "web_assets.rs")
ORIGINS = os.path.join(ROOT, "tests", "page", "origins.json")

failures = []


def fail(message):
    failures.append(message)


def files():
    out = []
    for dirpath, _, names in os.walk(WEB):
        for name in names:
            out.append(os.path.relpath(os.path.join(dirpath, name), WEB).replace(os.sep, "/"))
    return sorted(out, key=lambda s: s.encode())


def read(rel):
    with open(os.path.join(WEB, rel), "rb") as f:
        return f.read()


def text(rel):
    return read(rel).decode("utf-8")


# --- the code of a JavaScript file, without comments, strings and regular expressions ------------------------------------------------------

def strip_js(source):
    """Return the source with the comments, the strings, the template texts and the regular expressions taken out.

    The result keeps the line structure, so a message can name a line. A `${ ... }` of a template stays as code.
    """
    out = []
    i, n = 0, len(source)
    stack = []  # for template literals: the depth of the braces at each `${`
    last_sig = ""  # the last significant character, to tell a division from a regular expression
    brace = 0
    while i < n:
        c = source[i]
        nxt = source[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            while i < n and source[i] != "\n":
                i += 1
            continue
        if c == "/" and nxt == "*":
            j = source.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append("\n" * source.count("\n", i, j))
            i = j
            continue
        if c in "\"'":
            j = i + 1
            while j < n and source[j] != c:
                j += 2 if source[j] == "\\" else 1
            out.append(c + c)
            i = j + 1
            last_sig = c
            continue
        if c == "`":
            # a template: skip the text, keep the code of each ${ }
            j = i + 1
            out.append("``")
            while j < n and source[j] != "`":
                if source[j] == "\\":
                    j += 2
                    continue
                if source[j] == "$" and j + 1 < n and source[j + 1] == "{":
                    depth, k = 1, j + 2
                    while k < n and depth:
                        if source[k] == "{":
                            depth += 1
                        elif source[k] == "}":
                            depth -= 1
                        k += 1
                    out.append(" " + strip_js(source[j + 2:k - 1]) + " ")
                    j = k
                    continue
                if source[j] == "\n":
                    out.append("\n")
                j += 1
            i = j + 1
            last_sig = "`"
            continue
        if c == "/" and last_sig in "(,=:[!&|?{};+-*%<>~^" or (c == "/" and last_sig == ""):
            # a regular expression literal
            j = i + 1
            in_class = False
            while j < n and (source[j] != "/" or in_class):
                if source[j] == "\\":
                    j += 1
                elif source[j] == "[":
                    in_class = True
                elif source[j] == "]":
                    in_class = False
                j += 1
            j += 1
            while j < n and source[j].isalpha():
                j += 1
            out.append("/re/")
            i = j
            last_sig = ")"
            continue
        out.append(c)
        if not c.isspace():
            last_sig = c
        i += 1
    return "".join(out)


FORBIDDEN_JS = [
    (r"\binnerHTML\b", "innerHTML"), (r"\bouterHTML\b", "outerHTML"), (r"\binsertAdjacentHTML\b", "insertAdjacentHTML"),
    (r"\bdocument\s*\.\s*write(ln)?\b", "document.write"), (r"\bcreateContextualFragment\b", "createContextualFragment"),
    (r"\beval\s*\(", "eval"), (r"\bnew\s+Function\b", "new Function"), (r"\bsrcdoc\b", "srcdoc"),
    (r"setAttribute\s*\(\s*(\"\"|'')\s*,", ""),  # placeholder: the strings are taken out, see the check of the style below
    (r"\bdocument\s*\.\s*cookie\b", "document.cookie"), (r"\bsetTimeout\s*\(\s*(\"\"|'')", "setTimeout with a text"),
    (r"\bsetInterval\s*\(\s*(\"\"|'')", "setInterval with a text"), (r"\bimportScripts\b", "importScripts"), (r"\bXMLHttpRequest\b", "XMLHttpRequest"),
    (r"\bWebSocket\b", "WebSocket"), (r"\bEventSource\b", "EventSource"), (r"\bsendBeacon\b", "sendBeacon"),
]


def check_javascript():
    for rel in files():
        if not rel.endswith(".js"):
            continue
        source = text(rel)
        code = strip_js(source)
        for pattern, name in FORBIDDEN_JS:
            if not name:
                continue
            for m in re.finditer(pattern, code):
                line = code.count("\n", 0, m.start()) + 1
                fail("%s:%d uses %s." % (rel, line, name))
        # The attribute style is set with the object style of the CSS object model only (7.5): a `style` attribute of a string is not allowed.
        for m in re.finditer(r"setAttribute\(\s*[\"']style[\"']", source):
            fail("%s sets the attribute style with a string." % rel)
        for m in re.finditer(r"\.cssText\b", code):
            fail("%s uses cssText." % rel)
        for m in re.finditer(r"\bimport\s*\(\s*[\"'](?!\.)", source):
            fail("%s loads a module that is not a file of the page." % rel)
        for m in re.finditer(r"""\bfrom\s+["'](?!\.)""", source):
            fail("%s imports a module that is not a file of the page." % rel)
        for m in re.finditer(r"\bfetch\s*\(\s*[\"'](https?:|//)", source):
            fail("%s asks another origin." % rel)


# --- the shell ---------------------------------------------------------------------------------------------------------------------------------

def check_shell():
    s = text("shell.html")
    if s.count("{{base_path}}") != 1:
        fail("shell.html must have the marker {{base_path}} one time.")
    if s.count("{{build}}") < 2:
        fail("shell.html must have the marker {{build}}.")
    rest = s.replace("{{base_path}}", "").replace("{{build}}", "")
    if "{{" in rest:
        fail("shell.html has another marker.")
    for m in re.finditer(r"<script\b([^>]*)>", s):
        if "src=" not in m.group(1):
            fail("shell.html has an inline script.")
    if re.search(r"<style\b", s, re.I):
        fail("shell.html has an inline style.")
    if re.search(r"\sstyle\s*=", s, re.I):
        fail("shell.html has a style attribute.")
    if re.search(r"\son[a-z]+\s*=", s, re.I):
        fail("shell.html has an event attribute.")
    if re.search(r"javascript:", s, re.I):
        fail("shell.html has a javascript: address.")
    if re.search(r"<(link|script|img)[^>]+(src|href)=\"https?:", s):
        fail("shell.html asks another origin.")
    if "innerHTML" in s:
        fail("shell.html has innerHTML.")
    if "rel=\"icon\" href=\"static" in s:
        fail("shell.html has a favicon file (5.23).")
    if re.search(r"<title>[^<]*pupitre[^<]*</title>", s, re.I):
        fail("shell.html has the product name before the session answers (5.23).")
    for m in re.finditer(r"static/\{\{build\}\}/([^\"']+)", s):
        if not os.path.isfile(os.path.join(WEB, m.group(1))):
            fail("shell.html names %s, which is not a file." % m.group(1))


# --- the origins, the paths, the e-mail addresses ------------------------------------------------------------------------------------

DOC_NETS = ("192.0.2.", "198.51.100.", "203.0.113.", "127.0.0.1", "0.0.0.0")


def check_text_rules():
    for rel in files():
        if rel.endswith((".woff2",)):
            continue
        s = read(rel).decode("utf-8", "replace")
        vendor = rel.startswith("css/vendor/") or rel.endswith("LICENSE")
        if not vendor:
            for m in re.finditer(r"(https?:)?//[A-Za-z0-9.-]+\.[a-z]{2,}", s):
                if m.group(0).startswith(("http://www.w3.org", "//www.w3.org")):
                    continue
                if m.group(1) is None and re.match(r"//[A-Za-z0-9.-]+\.(js|css|svg|json|woff2)$", m.group(0)):
                    continue
                if m.group(1) is None:
                    continue
                fail("%s names another origin: %s" % (rel, m.group(0)))
            for m in re.finditer(r"@import\s+(url\()?[\"']?https?:", s):
                fail("%s imports a style from another origin." % rel)
        for m in re.finditer(r"/home/|/Users/|C:\\\\Users", s):
            fail("%s has a local path." % rel)
        for m in re.finditer(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}", s):
            if vendor:
                continue
            fail("%s has an e-mail address: %s" % (rel, m.group(0)))
        if not vendor and not rel.endswith(".svg"):
            for m in re.finditer(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b", s):
                ip = m.group(0)
                # A version text such as 0.1.0 has three parts, so it is not here. The CSS values have a unit and no four numbers.
                if not ip.startswith(DOC_NETS):
                    fail("%s has an IP address that is not a documentation address: %s" % (rel, ip))


def check_ascii():
    for rel in files():
        if rel.endswith(".woff2") or rel.endswith("LICENSE") or rel.startswith("css/vendor/"):
            continue
        data = read(rel)
        bad = [i for i, b in enumerate(data) if b > 127 or (b < 32 and b not in (9, 10))]
        if bad:
            line = data.count(b"\n", 0, bad[0]) + 1
            fail("%s has a character that is not ASCII, near the line %d." % (rel, line))


# --- the list ASSETS --------------------------------------------------------------------------------------------------------------------------

def check_assets():
    rust = open(RUST, encoding="utf-8").read()
    listed = re.findall(r'Asset \{ path: "([^"]+)", content_type: "([^"]+)", bytes: include_bytes!\("([^"]+)"\) \}', rust)
    paths = [p for p, _, _ in listed]
    if paths != sorted(paths, key=lambda s: s.encode()):
        fail("The list ASSETS is not in the byte order of the paths.")
    if len(set(paths)) != len(paths):
        fail("The list ASSETS has a path two times.")
    on_disk = [f for f in files() if f != "shell.html"]
    for f in on_disk:
        if f not in paths:
            fail("The file web/%s is not in the list ASSETS. Run tests/page/gen_assets.py." % f)
    for p, _, inc in listed:
        if p not in on_disk:
            fail("The list ASSETS names %s, which is not a file of web/." % p)
        if inc != "../../../web/" + p:
            fail("The path of include_bytes for %s is %s." % (p, inc))
    if "shell.html" in paths:
        fail("shell.html is the shell, not a static file.")


# --- the references ------------------------------------------------------------------------------------------------------------------------------

def join(frm, ref):
    parts = frm.split("/")[:-1]
    for seg in ref.split("/"):
        if seg in (".", ""):
            continue
        if seg == "..":
            if parts:
                parts.pop()
        else:
            parts.append(seg)
    return "/".join(parts)


def check_references():
    names = set(files())
    for rel in files():
        if rel.endswith(".js"):
            s = text(rel)
            for m in re.finditer(r"""(?:from\s+|import\s*\(\s*)["'](\.[^"']*)["']""", s):
                target = join(rel, m.group(1))
                if target not in names:
                    fail("%s imports %s, which is not a file (%s)." % (rel, m.group(1), target))
        elif rel.endswith(".css") and not rel.startswith("css/vendor/"):
            s = text(rel)
            for m in re.finditer(r"url\(\s*([^)]*?)\s*\)", s):
                ref = m.group(1).strip("\"'")
                if ref.startswith(("data:", "#")):
                    continue
                if join(rel, ref) not in names:
                    fail("%s refers to %s, which is not a file." % (rel, ref))


# --- the copies --------------------------------------------------------------------------------------------------------------------------------

def check_origins():
    data = json.load(open(ORIGINS, encoding="utf-8"))
    recorded = {e["path"]: e for e in data["files"]}
    names = set(files())
    for path, entry in recorded.items():
        if path not in names:
            fail("origins.json names %s, which is not a file." % path)
            continue
        if entry["kind"] == "byte copy":
            digest = hashlib.sha256(read(path)).hexdigest()
            if digest != entry["sha256"]:
                fail("%s is a byte copy, and its sha256 changed." % path)
        else:
            first = text(path).split("\n", 1)[0]
            if "Copied from Hune" not in first or data["hune"]["commit"] not in first:
                fail("%s must say in its first line where it comes from." % path)
    own = [p for p in data["written for pupitre"]]
    for rel in names:
        if rel in recorded:
            continue
        if rel in own or any(p.endswith("/*") and rel.startswith(p[:-1]) for p in own):
            if rel.endswith((".js", ".css", ".html")) and "Copied from" in text(rel).split("\n", 1)[0]:
                fail("%s says that it is a copy, and origins.json does not list it." % rel)
            continue
        fail("The file web/%s is neither in origins.json as a copy nor as written for Pupitre." % rel)


def self_test():
    """The reader of the code must find the forbidden names in code and must not find them in a comment, a string or a regular expression."""
    cases = {
        "a.innerHTML = 1;": True,
        "// innerHTML in a comment\nx = 1;": False,
        "/* innerHTML */ x = 1;": False,
        'x = "innerHTML";': False,
        "x = `a ${b.innerHTML} c`;": True,
        "x = `innerHTML ${1}`;": False,
        "const r = /innerHTML/g; y = 2;": False,
        "a = b / c; d.outerHTML = 1;": True,
        "document.write('x')": True,
        "el.insertAdjacentHTML('beforeend', x)": True,
        "const s = 'it\\'s'; q.innerHTML = s;": True,
    }
    for source, expected in cases.items():
        code = strip_js(source)
        found = any(re.search(pattern, code) for pattern, name in FORBIDDEN_JS if name)
        if found != expected:
            fail("The self-test of the reader of the code failed for %r." % source)


def main():
    self_test()
    check_javascript()
    check_shell()
    check_text_rules()
    check_ascii()
    check_assets()
    check_references()
    check_origins()
    if failures:
        for f in failures:
            print("FAIL " + f)
        print("%d checks failed." % len(failures))
        return 1
    print("check_page: ok (%d files in web/)" % len(files()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
