# SPDX-License-Identifier: MIT OR Apache-2.0
"""Unit tests of the seeded generator of the large fixture (8.12, 2.1 to 2.10, 2.31)."""
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest

import support

g = support.generator()

ENV_ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
LABEL = re.compile(r"^[a-z][a-z0-9_]{0,62}$")
KEY_PATH = re.compile(r"^[a-z][a-z0-9_]{0,62}(\.[a-z][a-z0-9_]{0,62}){0,7}$")


def strict_load(data: bytes):
    """Read a document with the strict rules of 2.5 that matter here: no null, no float, no duplicate name."""

    def no_float(text):
        raise ValueError("a float is not valid: " + text)

    def no_constant(text):
        raise ValueError("a constant is not valid: " + text)

    def no_duplicate(pairs):
        names = [name for name, _ in pairs]
        if len(names) != len(set(names)):
            raise ValueError("duplicate name")
        return dict(pairs)

    value = json.loads(data.decode("utf-8"), parse_float=no_float, parse_constant=no_constant, object_pairs_hook=no_duplicate)

    def walk(node):
        if node is None:
            raise ValueError("null is not valid")
        if isinstance(node, dict):
            for item in node.values():
                walk(item)
        elif isinstance(node, list):
            for item in node:
                walk(item)

    walk(value)
    return value


class CanonicalForm(unittest.TestCase):
    """2.4: the canonical form."""

    def test_the_example_of_2_4(self):
        document = {"format": 1, "notes": {"log.level": "Debug for the incident of 2026-11-02."}, "values": {"limits.max_clients": 200, "log.level": "debug"}}
        expected = (
            '{\n  "format": 1,\n  "notes": {\n    "log.level": "Debug for the incident of 2026-11-02."\n  },\n'
            '  "values": {\n    "limits.max_clients": 200,\n    "log.level": "debug"\n  }\n}\n'
        )
        self.assertEqual(g.canonical(document).decode(), expected)

    def test_empty_containers_and_the_line_end(self):
        self.assertEqual(g.canonical({}), b"{}\n")
        self.assertEqual(g.canonical({"a": [], "b": {}}), b'{\n  "a": [],\n  "b": {}\n}\n')

    def test_the_members_are_in_byte_order(self):
        data = g.canonical({"b": 1, "a": 2, "B": 3})
        self.assertEqual(data, b'{\n  "B": 3,\n  "a": 2,\n  "b": 1\n}\n')

    def test_the_escapes_of_rule_7(self):
        data = g.canonical({"t": 'a"b\\c\n\t\x01\x7f/é'})
        self.assertEqual(data, '{\n  "t": "a\\"b\\\\c\\n\\t\\u0001\x7f/é"\n}\n'.encode("utf-8"))

    def test_an_integer_bound_and_no_float_no_null(self):
        self.assertEqual(g.canonical({"n": 9007199254740991}), b'{\n  "n": 9007199254740991\n}\n')
        with self.assertRaises(ValueError):
            g.canonical({"n": 9007199254740992})
        with self.assertRaises(ValueError):
            g.canonical({"n": -1})
        with self.assertRaises(TypeError):
            g.canonical({"n": 1.5})
        with self.assertRaises(TypeError):
            g.canonical({"n": None})

    def test_the_etag_is_the_sha256_of_the_bytes(self):
        self.assertEqual(g.etag(b"{}\n"), hashlib.sha256(b"{}\n").hexdigest())


class Determinism(unittest.TestCase):
    """8.12: the same seed gives the same bytes. A fixture needs no large file in the repository."""

    def test_the_same_seed_gives_the_same_bytes(self):
        first = g.build(7, g.PROFILES["small"])
        second = g.build(7, g.PROFILES["small"])
        self.assertEqual(first.keys(), second.keys())
        for path in first:
            self.assertEqual(first[path], second[path], path)
        self.assertEqual(g.tree_digest(first), g.tree_digest(second))

    def test_two_processes_with_two_hash_seeds_write_the_same_bytes(self):
        script = os.path.join(support.ROOT, "tests", "fixtures", "generate.py")
        with tempfile.TemporaryDirectory() as one, tempfile.TemporaryDirectory() as two:
            for index, base in enumerate((one, two)):
                env = {"PYTHONHASHSEED": str(index + 1), "PATH": os.environ.get("PATH", "")}
                subprocess.run(
                    [sys.executable, script, "--seed", "3", "--profile", "small", "--out", os.path.join(base, "o"), "--schema-dir", "/srv/schemas"],
                    check=True, env=env, stdout=subprocess.DEVNULL,
                )
            count = 0
            for base, _dirs, names in os.walk(os.path.join(one, "o")):
                for name in names:
                    left = os.path.join(base, name)
                    right = os.path.join(two, os.path.relpath(left, one))
                    with open(left, "rb") as a, open(right, "rb") as b:
                        self.assertEqual(a.read(), b.read(), left)
                    count += 1
            self.assertGreater(count, 100)

    def test_another_seed_gives_other_bytes(self):
        first = g.build(7, g.PROFILES["small"])
        second = g.build(8, g.PROFILES["small"])
        self.assertNotEqual(g.tree_digest(first), g.tree_digest(second))
        # The shape of the units stays the same.
        units = lambda files: sorted(p for p in files if p.endswith("/unit.json"))
        self.assertEqual(units(first), units(second))

    def test_the_option_no_history_writes_the_documents_only(self):
        with_history = g.build(7, g.PROFILES["small"])
        without = g.build(7, g.PROFILES["small"], history=False)
        self.assertFalse([p for p in without if "/.history/" in p])
        documents = {p: d for p, d in with_history.items() if "/.history/" not in p}
        self.assertEqual(documents, without)

    def test_the_digest_follows_each_byte(self):
        files = g.build(7, g.PROFILES["small"])
        before = g.tree_digest(files)
        path = sorted(files)[0]
        files[path] = files[path] + b" "
        self.assertNotEqual(before, g.tree_digest(files))

    def test_the_generator_refuses_a_folder_that_is_not_empty(self):
        with tempfile.TemporaryDirectory() as folder:
            with open(os.path.join(folder, "x"), "w") as handle:
                handle.write("x")
            self.assertEqual(g.main(["--seed", "1", "--profile", "small", "--out", folder]), 1)


class Shape(unittest.TestCase):
    """8.12: 1,000 units, 1,000 hosts, 100 hosts in each unit, and 64 small environments."""

    @classmethod
    def setUpClass(cls):
        cls.files = g.build(7, g.PROFILES["large"])
        cls.counts = g.counts(cls.files)

    def documents(self, suffix):
        prefix = "data/environments/large/"
        return {p: strict_load(d) for p, d in self.files.items() if p.startswith(prefix) and p.endswith(suffix)}

    def test_the_big_environment_has_1000_units_and_1000_hosts(self):
        prefix = "data/environments/large/"
        units = [p for p in self.files if p.startswith(prefix + "units/") and p.endswith("/unit.json")]
        hosts = [p for p in self.files if p.startswith(prefix + "hosts/") and p.endswith(".json")]
        self.assertEqual(len(units), 1000)
        self.assertEqual(len(hosts), 1000)

    def test_each_unit_has_100_hosts_in_byte_order_with_a_document_host(self):
        host_files = {p.split("/")[-1][:-5] for p in self.files if p.startswith("data/environments/large/hosts/") and p.endswith(".json")}
        count = 0
        for path, doc in self.documents("/unit.json").items():
            count += 1
            self.assertEqual(len(doc["hosts"]), 100, path)
            self.assertEqual(doc["hosts"], sorted(set(doc["hosts"])), path)
            self.assertTrue(set(doc["hosts"]) <= host_files, path)
        self.assertEqual(count, 1000)

    def test_no_host_is_in_two_units_with_one_schema(self):
        seen = {}
        for path, doc in self.documents("/unit.json").items():
            for host in doc["hosts"]:
                key = (host, doc["schema"])
                self.assertNotIn(key, seen, "2.9: %s and %s" % (seen.get(key), path))
                seen[key] = path

    def test_each_host_is_in_100_units(self):
        per_host = {}
        for doc in self.documents("/unit.json").values():
            for host in doc["hosts"]:
                per_host[host] = per_host.get(host, 0) + 1
        self.assertEqual(len(per_host), 1000)
        self.assertEqual(set(per_host.values()), {100})

    def test_there_are_64_small_environments_and_65_in_all(self):
        self.assertEqual(self.counts["environments"], 65)
        small = [p for p in self.files if re.match(r"^data/environments/env-\d\d/environment\.json$", p)]
        self.assertEqual(len(small), 64)

    def test_the_counts_agree_with_the_files(self):
        self.assertEqual(self.counts["units"], 1000 + 64 * 2)
        self.assertEqual(self.counts["hosts"], 1000 + 64 * 3)
        self.assertEqual(self.counts["common"], 65)
        self.assertGreater(self.counts["host_values"], 0)

    def test_the_schemas_are_closed_and_distinct(self):
        schemas = {p: strict_load(d) for p, d in self.files.items() if p.startswith("schemas/")}
        self.assertEqual(len(schemas), 100)
        self.assertEqual(len({s["variable"] for s in schemas.values()}), 100, "C10: one variable for each schema")
        self.assertEqual(len({s["group"] for s in schemas.values()}), 100, "C10: one group for each schema")
        for schema in schemas.values():
            paths = [k["path"] for k in schema["keys"]]
            self.assertEqual(paths, sorted(paths, key=lambda text: text.encode("utf-8")))
            self.assertTrue(re.match(r"^[a-z][a-z0-9_]{0,62}$", schema["variable"]))
            self.assertFalse(schema["variable"].startswith(("ansible_", "pupitre_")))
        self.assertLessEqual(len(schemas), 256)

    def test_each_unit_names_a_schema_of_the_fixture(self):
        names = {p[len("schemas/") : -5] for p in self.files if p.startswith("schemas/")}
        for path, doc in list(self.documents("/unit.json").items()):
            self.assertIn(doc["schema"], names, path)


class FormatRules(unittest.TestCase):
    """2.1 to 2.10: each document passes the format rules, and each file is canonical."""

    @classmethod
    def setUpClass(cls):
        cls.files = g.build(11, g.PROFILES["small"])

    def test_each_document_and_version_is_canonical(self):
        for path, data in self.files.items():
            if not path.endswith(".json"):
                continue
            value = strict_load(data)
            self.assertEqual(g.canonical(value), data, path)

    def test_the_layout_of_2_31(self):
        marker = strict_load(self.files["data/pupitre-store.json"])
        self.assertEqual(marker, {"format": 1, "store_schema": 1})
        for path in self.files:
            if path.startswith("data/environments/"):
                tail = path[len("data/environments/") :]
                self.assertTrue(
                    re.match(
                        r"^[a-z0-9][a-z0-9-]*/(environment|common)\.json$"
                        r"|^[a-z0-9][a-z0-9-]*/hosts/[a-z0-9][a-z0-9-]*\.json$"
                        r"|^[a-z0-9][a-z0-9-]*/units/[a-z0-9][a-z0-9-]*/(unit|values)\.json$"
                        r"|^[a-z0-9][a-z0-9-]*/units/[a-z0-9][a-z0-9-]*/hosts/[a-z0-9][a-z0-9-]*/values\.json$",
                        tail,
                    ),
                    path,
                )

    def test_each_document_has_version_1_with_its_metadata(self):
        for path, data in self.files.items():
            if not (path.startswith("data/environments/") and path.endswith(".json")):
                continue
            key = path[len("data/") : -5]
            version = self.files["data/.history/%s/0000000001.json" % key]
            self.assertEqual(version, data)
            meta = strict_load(self.files["data/.history/%s/0000000001.meta.json" % key])
            self.assertEqual(sorted(meta), ["etag", "kind", "message", "saved_at", "saved_by"])
            self.assertEqual(meta["etag"], hashlib.sha256(data).hexdigest())
            self.assertRegex(meta["saved_at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")

    def test_ids_and_members(self):
        for path, data in self.files.items():
            if not (path.startswith("data/environments/") and path.endswith(".json")) or ".history" in path:
                continue
            for segment in path[len("data/environments/") : -5].split("/"):
                if segment in ("hosts", "units", "environment", "common", "unit", "values"):
                    continue
                self.assertRegex(segment, ENV_ID)
            doc = strict_load(data)
            self.assertEqual(doc["format"], 1)
            if path.endswith("/environment.json"):
                self.assertEqual(sorted(doc), ["format", "label"])
                self.assertTrue(1 <= len(doc["label"]) <= 64)
            elif "/hosts/" in path and path.endswith("/values.json") is False and "/units/" not in path:
                self.assertTrue(set(doc) <= {"address", "format", "labels", "port"})
                self.assertEqual(doc["labels"], sorted(set(doc["labels"])))
                self.assertLessEqual(len(doc["labels"]), 16)
                for label in doc["labels"]:
                    self.assertRegex(label, LABEL)
                self.assertRegex(doc["address"], r"^[a-z0-9.-]+\.invalid$")
                if "port" in doc:
                    self.assertTrue(1 <= doc["port"] <= 65535)
            elif path.endswith("/unit.json"):
                self.assertEqual(sorted(doc), ["format", "hosts", "label", "playbooks", "schema"])
                self.assertIn("deploy", doc["playbooks"])
            elif path.endswith("/values.json") or path.endswith("/common.json"):
                self.assertTrue(set(doc) <= {"format", "notes", "values"})
                self.assertLessEqual(len(doc["values"]), 1024)
                for key_path in doc["values"]:
                    self.assertRegex(key_path, KEY_PATH)
                    self.assertLessEqual(len(key_path.encode()), 255)

    def test_each_value_passes_its_schema(self):
        schemas = {p[len("schemas/") : -5]: strict_load(d) for p, d in self.files.items() if p.startswith("schemas/")}
        units = {}
        for path, data in self.files.items():
            if path.endswith("/unit.json") and ".history" not in path:
                units[path[: -len("unit.json")]] = strict_load(data)
        for base, unit in units.items():
            schema = schemas[unit["schema"]]
            keys = {k["path"]: k for k in schema["keys"]}
            values = strict_load(self.files[base + "values.json"])["values"]
            for key_path, value in values.items():
                self.check_value(keys[key_path], value)

    def check_value(self, key, value):
        if key["kind"] == "integer":
            self.assertIsInstance(value, int)
            self.assertTrue(key["min"] <= value <= key["max"])
        elif key["kind"] == "enum":
            self.assertIn(value, key["values"])
        elif key["kind"] == "text":
            self.assertTrue(key["min"] <= len(value.encode()) <= key["max"])
        else:
            self.fail("kind of the generator: " + key["kind"])

    def test_the_profile_check(self):
        with self.assertRaises(ValueError):
            g.Profile(hosts=10, hosts_per_unit=3, small_environments=1, small_hosts=1).check()
        with self.assertRaises(ValueError):
            g.Profile(hosts=600, hosts_per_unit=300, small_environments=1, small_hosts=1).check()
        g.PROFILES["large"].check()


class DemoSchema(unittest.TestCase):
    """The schema file of the synthetic services is canonical and closed (2.11)."""

    def test_the_demo_schema_is_canonical(self):
        path = os.path.join(support.ACCEPTANCE, "ansible", "schemas", "demo.json")
        with open(path, "rb") as handle:
            data = handle.read()
        self.assertEqual(g.canonical(strict_load(data)), data)
        schema = json.loads(data)
        self.assertEqual(schema["variable"], "demo_config")
        self.assertEqual(schema["group"], "demo_hosts")
        secrets = [k["path"] for k in schema["keys"] if k.get("secret")]
        self.assertEqual(secrets, ["api.token", "storage.password"])
        for key in schema["keys"]:
            if key.get("secret"):
                self.assertNotIn("default", key, "2.11: a secret key has no default")


if __name__ == "__main__":
    unittest.main()
