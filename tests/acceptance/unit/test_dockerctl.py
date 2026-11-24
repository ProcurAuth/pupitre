# SPDX-License-Identifier: MIT OR Apache-2.0
"""Unit tests of the Docker wrapper: the names, the labels and the check that nothing is left (8.12).

The tests that need the Docker daemon skip when the daemon does not answer.
"""
import os
import subprocess
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from harness.dockerctl import Docker, LABEL, PREFIX, wait_until  # noqa: E402


def daemon_up() -> bool:
    try:
        return subprocess.run(["docker", "info"], capture_output=True, timeout=20).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


class Names(unittest.TestCase):
    def test_the_run_id_has_8_hexadecimal_characters(self):
        for bad in ("", "xyz", "ABCDEF01", "0123456", "0123456789"):
            with self.assertRaises(ValueError):
                Docker(bad)
        self.assertEqual(Docker("0123abcd").name("host-a"), "pupitre-acc-0123abcd-host-a")
        self.assertTrue(Docker("0123abcd").prefix.startswith(PREFIX))

    def test_wait_until_returns_the_value_and_times_out(self):
        counter = iter([0, 0, 5])
        self.assertEqual(wait_until(lambda: next(counter), 5, step=0.01), 5)
        with self.assertRaises(TimeoutError):
            wait_until(lambda: False, 0.1, step=0.02)

    def test_wait_until_keeps_waiting_after_an_error(self):
        calls = []

        def check():
            calls.append(1)
            if len(calls) < 3:
                raise RuntimeError("not yet")
            return "ok"

        self.assertEqual(wait_until(check, 5, step=0.01), "ok")


@unittest.skipUnless(daemon_up(), "the Docker daemon does not answer")
class Cleanup(unittest.TestCase):
    """A network of this run is found by `leftovers`, removed by `cleanup`, and then not found."""

    def test_a_network_is_found_then_removed(self):
        docker = Docker("deadbee1")
        try:
            docker.network_create("selftest")
            found = docker.leftovers()
            self.assertTrue(any(item.startswith("network ") for item in found), found)
            self.assertEqual(docker.cleanup(), [])
            self.assertEqual(docker.leftovers(), [])
        finally:
            docker.cleanup()

    def test_the_label_is_on_the_network(self):
        docker = Docker("deadbee2")
        try:
            name = docker.network_create("selftest")
            label = docker.cmd(["network", "inspect", "--format", "{{index .Labels \"%s\"}}" % LABEL, name]).out.strip()
            self.assertEqual(label, "deadbee2")
            self.assertTrue(docker.cmd(["network", "inspect", "--format", "{{.Internal}}", name]).out.strip() == "true")
        finally:
            docker.cleanup()

    def test_cleanup_does_not_touch_a_resource_of_another_run(self):
        one, two = Docker("deadbee3"), Docker("deadbee4")
        try:
            one.network_create("a")
            two.network_create("a")
            one.cleanup()
            self.assertEqual(one.leftovers(), [])
            self.assertTrue(two.leftovers(), "the network of the other run must stay")
        finally:
            one.cleanup()
            two.cleanup()


if __name__ == "__main__":
    unittest.main()
