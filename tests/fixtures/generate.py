#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Make the large synthetic fixture of the acceptance tests (8.12) from a seed.

Use: python3 -I tests/fixtures/generate.py --seed 7 --out DIR [--profile large|small] [--schema-dir ABS_PATH]
                                           [--no-marker] [--no-history]

The generator is pure. The same seed and the same profile give the same bytes: it reads no clock, no
environment variable and no file. No large file is in the repository.

The profile `large` is the fixture of 8.12:
  - one environment `large` with 1,000 units, 1,000 hosts and 100 hosts in each unit;
  - 64 small environments `env-00` to `env-63`, each with 3 hosts and 2 units.
The profile `small` has the same shape with 40 hosts, 4 hosts in each unit and 3 small environments. Tests use it.

A host MUST NOT be in two units of one environment with the same schema (2.9). Each unit of the large
environment has 100 hosts of 1,000, so each host is in 100 units. Thus the fixture has 100 schemas
`s000` to `s099`, and 10 units for each schema that split the 1,000 hosts into 10 groups.

The output folder holds:
  data/                   the documents folder of the backend `files` (2.31): the documents and their versions
  schemas/<id>.json       one schema file (2.11) for each schema
  schema-blocks.hcl       the blocks `schema "<id>"` for the configuration file (6.12)
  fixture.json            the seed, the counts and the digest of the tree

The documents are in the canonical form of 2.4. A version 1 with the kind `import` goes with each document (2.18).
"""
import argparse
import hashlib
import json
import os
import random
import sys
from dataclasses import dataclass

# The time of each version. A fixed time keeps the bytes the same for the same seed.
SAVED_AT = "2026-11-24T00:00:00.000Z"
SAVED_BY = "import"
MESSAGE = "Synthetic fixture of the acceptance tests."

LABELS = ["batch_a", "batch_b", "canary", "region_eu", "region_us"]
LEVELS = ["error", "warn", "info", "debug"]
MAX_SAFE_INTEGER = 9007199254740991


@dataclass(frozen=True)
class Profile:
    """The size of a fixture."""

    hosts: int  # hosts of the big environment
    hosts_per_unit: int  # hosts of each unit of the big environment
    small_environments: int  # the count of small environments
    small_hosts: int  # hosts of each small environment

    @property
    def units_per_schema(self) -> int:
        return self.hosts // self.hosts_per_unit

    @property
    def schemas(self) -> int:
        # A host is in one unit of each schema, so each host is in `hosts_per_unit` units.
        return self.hosts_per_unit

    @property
    def units(self) -> int:
        return self.schemas * self.units_per_schema

    def check(self) -> None:
        if self.hosts % self.hosts_per_unit != 0:
            raise ValueError("hosts must be a multiple of hosts_per_unit")
        if self.schemas > 256:
            raise ValueError("a configuration has 256 schemas at most (6.12)")
        if self.small_hosts < 1 or self.small_environments < 0 or self.hosts < 1:
            raise ValueError("the counts must be positive")


PROFILES = {
    "large": Profile(hosts=1000, hosts_per_unit=100, small_environments=64, small_hosts=3),
    "small": Profile(hosts=40, hosts_per_unit=4, small_environments=3, small_hosts=3),
}


# ---------------------------------------------------------------------------------------------------------------
# The canonical form (2.4)
# ---------------------------------------------------------------------------------------------------------------

_ESCAPES = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\f": "\\f", "\n": "\\n", "\r": "\\r", "\t": "\\t"}


def _string(text: str) -> str:
    out = ['"']
    for char in text:
        if char in _ESCAPES:
            out.append(_ESCAPES[char])
        elif ord(char) < 0x20:
            out.append("\\u%04x" % ord(char))
        else:
            out.append(char)
    out.append('"')
    return "".join(out)


def _write(value, level: int, out: list) -> None:
    pad = "  " * (level + 1)
    close = "  " * level
    if isinstance(value, bool):
        out.append("true" if value else "false")
    elif isinstance(value, int):
        if not 0 <= value <= MAX_SAFE_INTEGER:
            raise ValueError("an integer must be from 0 to 2^53-1")
        out.append(str(value))
    elif isinstance(value, str):
        out.append(_string(value))
    elif isinstance(value, list):
        if not value:
            out.append("[]")
            return
        out.append("[\n")
        for index, item in enumerate(value):
            out.append(pad)
            _write(item, level + 1, out)
            out.append(",\n" if index + 1 < len(value) else "\n")
        out.append(close + "]")
    elif isinstance(value, dict):
        if not value:
            out.append("{}")
            return
        out.append("{\n")
        names = sorted(value, key=lambda name: name.encode("utf-8"))
        for index, name in enumerate(names):
            out.append(pad + _string(name) + ": ")
            _write(value[name], level + 1, out)
            out.append(",\n" if index + 1 < len(names) else "\n")
        out.append(close + "}")
    else:
        raise TypeError("a document has no %s" % type(value).__name__)


def canonical(value) -> bytes:
    """The canonical bytes of a value (2.4): 2 spaces, members in byte order, one line end at the end."""
    out: list = []
    _write(value, 0, out)
    out.append("\n")
    return "".join(out).encode("utf-8")


def etag(data: bytes) -> str:
    """The etag of a document (2.6)."""
    return hashlib.sha256(data).hexdigest()


# ---------------------------------------------------------------------------------------------------------------
# The documents
# ---------------------------------------------------------------------------------------------------------------


def schema_id(index: int) -> str:
    return "s%03d" % index


def schema_document(index: int) -> dict:
    """A schema of 2.11 with 3 keys. Each schema has its own variable and its own group (6.14, C10)."""
    return {
        "format": 1,
        "group": "s%03d_hosts" % index,
        "keys": [
            {
                "default": "synthetic",
                "description": "The banner of the synthetic service.",
                "kind": "text",
                "max": 200,
                "min": 1,
                "path": "banner",
                "rule": "no_control",
            },
            {
                "default": 100,
                "description": "The most clients at one time.",
                "kind": "integer",
                "max": 100000,
                "min": 1,
                "path": "limits.max_clients",
            },
            {
                "default": "info",
                "description": "The lowest level of the log.",
                "kind": "enum",
                "path": "log.level",
                "values": LEVELS,
            },
        ],
        "variable": "s%03d_config" % index,
    }


def _values(rng: random.Random) -> dict:
    """A random document of values for a schema of this fixture: 1 to 3 keys."""
    candidates = ["banner", "limits.max_clients", "log.level"]
    rng.shuffle(candidates)
    values: dict = {}
    for path in sorted(candidates[: rng.randint(1, 3)]):
        if path == "banner":
            values[path] = "banner-%06d" % rng.randrange(1_000_000)
        elif path == "limits.max_clients":
            values[path] = rng.randint(1, 100000)
        else:
            values[path] = rng.choice(LEVELS)
    return values


def _values_document(rng: random.Random) -> dict:
    return {"format": 1, "values": _values(rng)}


def _host_document(rng: random.Random, name: str) -> dict:
    document: dict = {
        "address": "%s.fleet.example.invalid" % name,
        "format": 1,
        "labels": sorted(rng.sample(LABELS, rng.randint(0, 3))),
    }
    if rng.random() < 0.2:
        document["port"] = rng.choice([22, 2222, 2200])
    return document


class Tree:
    """The files of the fixture: a map from a path (with `/`) to bytes."""

    def __init__(self, history: bool = True) -> None:
        self.files: dict = {}
        self.history = history

    def document(self, key: str, value: dict) -> None:
        """Add one document with its version 1 (2.18, 2.31). Without `history`, add the document only."""
        data = canonical(value)
        self.files["data/%s.json" % key] = data
        if not self.history:
            return
        history = "data/.history/%s/%010d" % (key, 1)
        self.files[history + ".json"] = data
        meta = {"etag": etag(data), "kind": "import", "message": MESSAGE, "saved_at": SAVED_AT, "saved_by": SAVED_BY}
        self.files[history + ".meta.json"] = canonical(meta)

    def raw(self, path: str, data: bytes) -> None:
        self.files[path] = data


def build(seed: int, profile: Profile, marker: bool = True, history: bool = True) -> dict:
    """Make the whole fixture in memory. The result maps each relative path to its bytes."""
    profile.check()
    rng = random.Random(seed)
    tree = Tree(history)
    if marker:
        tree.raw("data/pupitre-store.json", canonical({"format": 1, "store_schema": 1}))

    for index in range(profile.schemas):
        tree.raw("schemas/%s.json" % schema_id(index), canonical(schema_document(index)))

    # The big environment.
    env = "large"
    tree.document("environments/%s/environment" % env, {"format": 1, "label": "Large environment"})
    tree.document("environments/%s/common" % env, {"format": 1, "values": {"log.level": rng.choice(LEVELS)}})
    host_ids = ["host-%04d" % number for number in range(profile.hosts)]
    for host in host_ids:
        tree.document("environments/%s/hosts/%s" % (env, host), _host_document(rng, host))
    for index in range(profile.schemas):
        order = host_ids[:]
        rng.shuffle(order)
        for part in range(profile.units_per_schema):
            members = sorted(order[part * profile.hosts_per_unit : (part + 1) * profile.hosts_per_unit])
            unit = "%s-u%02d" % (schema_id(index), part)
            base = "environments/%s/units/%s" % (env, unit)
            tree.document(
                base + "/unit",
                {
                    "format": 1,
                    "hosts": members,
                    "label": "Unit %s" % unit,
                    "playbooks": {"deploy": "deploy-%s" % schema_id(index)},
                    "schema": schema_id(index),
                },
            )
            tree.document(base + "/values", _values_document(rng))
            for host in members:
                # About 1 pair in 100 has a value on the layer `host`.
                if rng.random() < 0.01:
                    tree.document("%s/hosts/%s/values" % (base, host), {"format": 1, "values": {"log.level": rng.choice(LEVELS)}})

    # The small environments.
    for number in range(profile.small_environments):
        env = "env-%02d" % number
        tree.document("environments/%s/environment" % env, {"format": 1, "label": "Small environment %02d" % number})
        tree.document("environments/%s/common" % env, {"format": 1, "values": {"log.level": rng.choice(LEVELS)}})
        small_hosts = ["host-%02d" % host for host in range(profile.small_hosts)]
        for host in small_hosts:
            tree.document("environments/%s/hosts/%s" % (env, host), _host_document(rng, "%s-%s" % (env, host)))
        for index in range(min(2, profile.schemas)):
            unit = "%s-u00" % schema_id(index)
            base = "environments/%s/units/%s" % (env, unit)
            tree.document(
                base + "/unit",
                {
                    "format": 1,
                    "hosts": small_hosts,
                    "label": "Unit %s" % unit,
                    "playbooks": {"deploy": "deploy-%s" % schema_id(index)},
                    "schema": schema_id(index),
                },
            )
            tree.document(base + "/values", _values_document(rng))

    return tree.files


def hcl_blocks(profile: Profile, schema_dir: str) -> str:
    """The blocks `schema` of the configuration file (6.12). `schema_dir` is an absolute path."""
    blocks = []
    for index in range(profile.schemas):
        name = schema_id(index)
        blocks.append('schema "%s" {\n  path = "%s/%s.json"\n}\n' % (name, schema_dir, name))
    return "\n".join(blocks)


def tree_digest(files: dict) -> str:
    """The digest of a tree: the SHA-256 over the sorted lines `<sha256 of the file> <path>`."""
    lines = ["%s %s\n" % (hashlib.sha256(files[path]).hexdigest(), path) for path in sorted(files)]
    return hashlib.sha256("".join(lines).encode("utf-8")).hexdigest()


def counts(files: dict) -> dict:
    """The counts of the documents of a tree, for the report and for the tests."""
    result = {"environments": 0, "hosts": 0, "units": 0, "unit_values": 0, "host_values": 0, "common": 0}
    for path in files:
        if not path.startswith("data/environments/") or path.endswith(".meta.json"):
            continue
        parts = path[len("data/environments/") :].split("/")
        name = parts[-1]
        if name == "environment.json":
            result["environments"] += 1
        elif name == "common.json":
            result["common"] += 1
        elif name == "unit.json":
            result["units"] += 1
        elif name == "values.json" and len(parts) == 4:
            result["unit_values"] += 1
        elif name == "values.json" and len(parts) == 6:
            result["host_values"] += 1
        elif len(parts) == 3 and parts[1] == "hosts":
            result["hosts"] += 1
    return result


def write(files: dict, out: str) -> None:
    """Write the files below `out`. The folders are made with the mode 0755 and the files with 0644."""
    for path in sorted(files):
        target = os.path.join(out, path)
        os.makedirs(os.path.dirname(target), mode=0o755, exist_ok=True)
        with open(target, "wb") as handle:
            handle.write(files[path])
        os.chmod(target, 0o644)


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Make the large synthetic fixture of the acceptance tests.")
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--out", required=True, help="An empty folder, or a folder that does not exist.")
    parser.add_argument("--profile", choices=sorted(PROFILES), default="large")
    parser.add_argument("--schema-dir", help="The absolute path of the schema folder in the configuration file.")
    parser.add_argument("--no-marker", action="store_true", help="Do not write data/pupitre-store.json.")
    parser.add_argument("--no-history", action="store_true", help="Write the documents only, with no version 1 in data/.history.")
    args = parser.parse_args(argv)

    out = os.path.abspath(args.out)
    if os.path.exists(out) and os.listdir(out):
        print("the folder %s is not empty" % out, file=sys.stderr)
        return 1
    profile = PROFILES[args.profile]
    files = build(args.seed, profile, marker=not args.no_marker, history=not args.no_history)
    schema_dir = args.schema_dir or os.path.join(out, "schemas")
    if not schema_dir.startswith("/"):
        print("--schema-dir must be an absolute path", file=sys.stderr)
        return 2
    # The digest covers the documents and the schemas. It does not cover `schema-blocks.hcl`: this file holds the path of `--out`.
    digest = tree_digest(files)
    files["schema-blocks.hcl"] = hcl_blocks(profile, schema_dir).encode("utf-8")
    summary = {"counts": counts(files), "digest": digest, "profile": args.profile, "seed": args.seed, "schemas": profile.schemas}
    files["fixture.json"] = canonical(summary)
    write(files, out)
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
