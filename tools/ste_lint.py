#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check Markdown files against the machine-checkable rules of ASD-STE100.

Use: python3 -I tools/ste_lint.py [--strict20] FILE...

The tool cannot check the whole standard. It checks these rules:
  S25   a sentence of more than 25 words (error)
  S20   a procedural sentence of more than 20 words (warning; an error with --strict20)
  PARA  a paragraph of more than 6 sentences (error)
  ING   a word that ends in "ing" and is not on the list of permitted words (error)
  WORD  a word that the standard replaces: ensure, allow, provide, and the like (error)
  PHR   a phrasal verb of the list (error)

The tool skips code blocks, tables, headings, link targets and the text of inline code.
An inline code span counts as one word. Technical names are permitted: put them in code spans.
The tool checks the text of a block quote as a paragraph. A bold run-in label at the start of a paragraph, as in
"**Test values.** The repository ...", does not count as a sentence of the paragraph. The label must be one sentence
of 8 words or fewer. The tool checks its words as the words of a sentence. A longer bold run is a usual sentence.

A procedural sentence starts with a verb in the imperative, or with "Do not" or "Never". It can also start with
a clause that ends with a comma, as in "To stop the server, press Ctrl+C". It can also start with a word of sequence,
as in "Then add the height". The limit of a descriptive sentence is 25 words.
"""
import re
import sys

PERMITTED_ING = {
    # nouns and technical names that end in "ing"
    "string", "strings", "thing", "nothing", "something", "anything", "everything", "during", "bring", "ring", "spring",
    "morning", "evening", "king", "ping", "pings", "sibling", "siblings", "sibling's", "pudding", "wing", "sing", "swing",
    "railing", "ceiling", "ceilings", "building", "buildings", "heading", "headings", "padding", "mapping", "mappings",
    "routing", "caching", "logging", "pooling", "tracing", "sampling", "hashing", "encoding", "decoding", "scheduling",
    "polling", "listing", "listings", "setting", "settings", "timing", "binding", "bindings", "warning", "warnings",
    "filtering", "sorting", "parsing", "lexing", "shaping", "naming", "versioning", "proxying", "rendering",
    "querying", "testing", "tokenizing", "monitoring", "billing", "tooling", "trailing", "leading", "missing",
    "existing", "following", "remaining", "pending", "matching",
}
# Words that are technical names and end in "ing" are written in code spans. The list above is short on purpose:
# the writer MUST rewrite the sentence when the word is a verb form.
STRICT_ING = {  # of the permitted list, these are verb forms in most uses: flagged anyway
    "following", "remaining", "existing", "missing", "matching", "leading", "trailing", "pending",
}
PERMITTED_ING -= STRICT_ING

REPLACE = {
    "ensure": "make sure", "ensures": "make sure", "ensured": "made sure",
    "allow": "let", "allows": "let", "allowed": "permitted", "allowing": "let",
    "provide": "give", "provides": "give", "provided": "gave", "providing": "give",
    "utilize": "use", "utilise": "use", "utilizes": "use", "leverage": "use", "facilitate": "help",
    "in order to": "to", "prior to": "before", "subsequent": "next", "commence": "start", "terminate": "stop",
    "additionally": "also", "furthermore": "also", "however": "but", "therefore": "so", "approximately": "about",
}
# The verbs that start a procedural sentence. A sentence that starts with one of these words is an instruction.
IMPERATIVE = {
    "add", "apply", "ask", "build", "call", "change", "check", "choose", "click", "close", "compare", "configure",
    "copy", "create", "define", "delete", "do", "download", "edit", "enter", "export", "fill", "find", "fix", "follow",
    "get", "give", "go", "import", "install", "keep", "let", "load", "look", "make", "mark", "move", "name", "never",
    "open", "press", "put", "read", "remove", "rename", "replace", "report", "restart", "restore", "run", "save",
    "select", "send", "set", "show", "sign", "start", "stop", "tag", "tell", "test", "try", "type", "update",
    "upgrade", "use", "wait", "write",
    "accept", "assign", "attach", "avoid", "bind", "clear", "compute", "connect", "convert", "declare", "deny",
    "deploy", "disable", "draw", "enable", "extract", "hide", "include", "mount", "omit", "pass", "pin",
    "print", "protect", "pull", "push", "refuse", "reject", "reload", "repeat", "require", "reuse", "review",
    "rewrite", "rotate", "see", "serve", "skip", "specify", "split", "store", "submit", "switch", "treat", "trust",
    "turn", "unpack", "verify",
}
# A sentence that starts with one of these words can hold an instruction after its first comma.
CLAUSE_START = {"to", "if", "when", "before", "after", "for", "in", "on", "with", "without", "then", "first", "next"}
# A word of sequence can come just before the verb of an instruction, with no comma: "Then add the height".
SEQUENCE = {"then", "first", "next", "finally", "also", "always", "only", "now", "second", "third"}
# A bold run-in label of this length or less is a label, not a sentence of the paragraph.
LABEL_WORDS = 8

PHRASAL = [
    "set up", "log in", "log out", "carry out", "turn on", "turn off", "start up", "shut down", "look up",
    "build up", "fill in", "clean up", "point out", "run out", "come back", "go back", "back up", "pick up",
    "hand over", "roll back", "write back", "read back", "fall back", "break down", "check out", "find out",
]


def strip_inline(text):
    text = re.sub(r"`[^`]*`", " CODE ", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", " ", text)
    text = re.sub(r"[*_]{1,3}", "", text)
    return text


def blocks(lines):
    """Yield (first_line_number, kind, text) for each paragraph or list item outside code and tables."""
    in_code = False
    buf, start = [], 0
    def flush():
        nonlocal buf, start
        if buf:
            yield (start, "para", " ".join(buf))
        buf = []
    for number, raw in enumerate(lines, 1):
        line = raw.rstrip("\n")
        if line.strip().startswith("```"):
            yield from flush()
            in_code = not in_code
            continue
        if in_code:
            continue
        stripped = line.strip()
        if stripped.startswith(">"):
            # A block quote is text: check its content as a paragraph.
            stripped = stripped.lstrip("> ").strip()
            line = stripped
        if not stripped or stripped.startswith("#") or stripped.startswith("|") or re.match(r"^[-*_]{3,}$", stripped):
            yield from flush()
            continue
        item = re.match(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$", line)
        if item:
            yield from flush()
            buf, start = [item.group(3)], number
            continue
        if not buf:
            start = number
        buf.append(stripped)
    yield from flush()


def sentences(text):
    text = strip_inline(text)
    parts = re.split(r"(?<=[.!?:;])\s+(?=[A-Z0-9(])", text)
    return [p.strip() for p in parts if p.strip()]


def procedural(sentence):
    """Return True when the sentence is an instruction: an imperative verb starts it or follows its first clause."""
    if sentence.lstrip().startswith(('"', "\u201c")):
        return False  # A quoted label, such as "Show data", is a name and not a verb.
    words = re.findall(r"[A-Za-z][A-Za-z'-]*", sentence)
    if not words:
        return False
    first = words[0].lower()
    if first in IMPERATIVE:
        return True
    if first in SEQUENCE and len(words) > 1 and words[1].lower() in IMPERATIVE:
        return True
    if first in CLAUSE_START and "," in sentence:
        after = re.findall(r"[A-Za-z][A-Za-z'-]*", sentence.split(",", 1)[1])
        return bool(after) and after[0].lower() in IMPERATIVE
    return False


def run_in_label(text):
    """Return the bold run-in label that opens the text, as in "**Test values.** ", or an empty string.

    The label is one sentence of LABEL_WORDS words or fewer. A longer bold run is not a label.
    """
    match = re.match(r"\*\*([^*]+[.:])\*\*\s+", text)
    if not match:
        return ""
    inner = sentences(match.group(1))
    if len(inner) != 1 or len(inner[0].split()) > LABEL_WORDS:
        return ""
    return match.group(0)


def check(path, strict20):
    errors, warnings = [], []
    with open(path, encoding="utf-8") as handle:
        lines = handle.readlines()
    for number, kind, text in blocks(lines):
        label = run_in_label(text)
        if label:
            text = text[len(label):]
        sents = sentences(text)
        if len(sents) > 6:
            errors.append((number, "PARA", f"{len(sents)} sentences in one paragraph or item"))
        # The label does not count in the paragraph, but its words get the same checks as a sentence.
        for sentence in ([strip_inline(label).strip()] if label else []) + sents:
            words = [w for w in re.split(r"\s+", sentence) if re.search(r"[A-Za-z0-9]", w)]
            count = len(words)
            if count > 25:
                errors.append((number, "S25", f"{count} words: {sentence[:90]}..."))
            elif count > 20 and procedural(sentence):
                (errors if strict20 else warnings).append((number, "S20", f"{count} words: {sentence[:90]}..."))
            low = sentence.lower()
            for word in re.findall(r"[a-z][a-z'-]*", low):
                if word.endswith("ing") and len(word) > 4 and word not in PERMITTED_ING:
                    errors.append((number, "ING", word))
                if word in REPLACE and " " not in word:
                    errors.append((number, "WORD", f"{word} -> {REPLACE[word]}"))
            for phrase in [k for k in REPLACE if " " in k]:
                if phrase in low:
                    errors.append((number, "WORD", f"{phrase} -> {REPLACE[phrase]}"))
            for phrase in PHRASAL:
                if re.search(r"\b" + phrase + r"\b", low):
                    errors.append((number, "PHR", phrase))
    return errors, warnings


def main(argv):
    strict20 = "--strict20" in argv
    files = [a for a in argv if not a.startswith("--")]
    if not files:
        print(__doc__)
        return 2
    total_errors = 0
    for path in files:
        errors, warnings = check(path, strict20)
        for number, code, message in errors:
            print(f"{path}:{number}: {code} {message}")
        counts = {}
        for _, code, _ in errors:
            counts[code] = counts.get(code, 0) + 1
        print(f"{path}: {len(errors)} errors {counts}, {len(warnings)} warnings (S20)")
        total_errors += len(errors)
    return 1 if total_errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
