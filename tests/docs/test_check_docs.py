# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of scripts/check-docs.py and scripts/build-site.py (8.12, level Documents).

Run: python3 -I -m unittest discover -s tests/docs
"""
import importlib.util
import os
import stat
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent


def load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


check = load("check_docs", "check-docs.py")
site = load("build_site", "build-site.py")

FRAGMENT = 'command "x" {\n  label = "X"\n}\n'
COMPLETE = (
    'server {\n  listen = "127.0.0.1:8080"\n  base_path = "/"\n}\n'
    'storage {\n  secret_key_file = "/etc/p/key"\n}\n'
    'schema "relay" {\n  path = "/etc/p/schemas/relay.json"\n}\n'
)


class Blocks(unittest.TestCase):
    def test_hcl_blocks_are_found_and_classified(self):
        text = f"text\n\n```hcl\n{FRAGMENT}```\n\n```sh\nserver {{\n```\n\n```hcl\n{COMPLETE}```\n"
        blocks = check.hcl_blocks(text)
        self.assertEqual(len(blocks), 2)
        self.assertFalse(check.is_complete(blocks[0]))
        self.assertTrue(check.is_complete(blocks[1]))


class Prepare(unittest.TestCase):
    def test_paths_move_under_the_root_and_files_exist(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            text, env = check.prepare_example(COMPLETE, root)
            self.assertIn(f'"{root}/etc/p/key"', text)
            self.assertIn('base_path = "/"', text)  # a base path is not a file path
            self.assertEqual((root / "etc/p/key").read_text().strip(), check.STORE_KEY)
            self.assertTrue((root / "etc/p/schemas/relay.json").is_file())
            self.assertEqual(env, {})

    def test_env_attributes_give_variables(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, env = check.prepare_example('storage {\n  secret_key_env = "PUPITRE_KEY"\n  x_env = "OTHER"\n}\n', Path(tmp))
            self.assertEqual(env, {"PUPITRE_KEY": check.STORE_KEY, "OTHER": "placeholder"})

    def test_unknown_schema_stops(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                check.prepare_example('schema "z" {\n  path = "/etc/p/z.json"\n}\n', Path(tmp))


class Run(unittest.TestCase):
    def fake_binary(self, directory: Path) -> str:
        path = directory / "fake-pupitre"
        # The fake program fails when the configuration holds the word BAD, as a real check fails on a bad key.
        path.write_text('#!/bin/sh\nif grep -q BAD "$3"; then echo "bad key" >&2; exit 1; fi\necho ok\n')
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return str(path)

    def test_a_valid_example_passes_and_an_invalid_one_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = self.fake_binary(Path(tmp))
            good = check.Report()
            check.run_example(binary, "good", COMPLETE, good)
            self.assertEqual(good.problems, [])
            bad = check.Report()
            check.run_example(binary, "bad", COMPLETE + "# BAD\n", bad)
            self.assertEqual(len(bad.problems), 1)
            self.assertIn("bad key", bad.problems[0])

    def test_the_repository_examples_are_found(self):
        names = [name for name, _ in check.example_sources()]
        self.assertIn("examples/local/pupitre.hcl", names)


class Hygiene(unittest.TestCase):
    def test_unfinished_words(self):
        for text in ("This is planned.", "Coming soon.", "TODO later", "not built in this version"):
            self.assertIsNotNone(check.UNFINISHED.search(text), text)
        self.assertIsNone(check.UNFINISHED.search("Pupitre starts a run."))

    def test_patterns(self):
        hits = {what: pattern for pattern, what in check.HYGIENE}
        self.assertTrue(hits["a local path"].search("/" + "home/someone/x"))
        self.assertTrue(hits["an e-mail address"].search("a@real-host.org"))
        self.assertIsNone(hits["an e-mail address"].search("a@example.org"))
        self.assertTrue(hits["an IP address"].search("8.8.8.8"))
        self.assertTrue(hits["an image registry"].search("docker " + "push x"))


class Site(unittest.TestCase):
    def test_the_site_is_current(self):
        pages = site.build()
        for name, text in pages.items():
            self.assertEqual((ROOT / "docs" / name).read_text(encoding="utf-8"), text, name)

    def test_markdown_basics(self):
        md = site.Markdown("docs")
        out = md.render("# T\n\n## Head\n\ntext `a|b` **bold**\n\n| a | b |\n|---|---|\n| 1 | 2 |\n")
        self.assertIn('<h2 id="head">', out)
        self.assertIn("<code>a|b</code>", out)
        self.assertIn('<th scope="col">a</th>', out)

    def test_no_external_request_in_pages(self):
        for name, text in site.build().items():
            if name.endswith(".html") and name != "404.html":
                for tag in ("<script src=\"http", "<link rel=\"stylesheet\" href=\"http", "<img src=\"http"):
                    self.assertNotIn(tag, text, name)


if __name__ == "__main__":
    unittest.main()
