# SPDX-License-Identifier: MIT OR Apache-2.0
"""The report of a run of the acceptance tests: a list of results with the status ok, FAIL or SKIP."""
import sys
import time

OK = "ok"
FAIL = "FAIL"
SKIP = "SKIP"


class Report:
    """Collect the results and print them as they come."""

    def __init__(self, stream=None) -> None:
        self.stream = stream or sys.stdout
        self.results: list = []
        self.started = time.monotonic()

    def add(self, name: str, status: str, detail: str = "") -> None:
        self.results.append((name, status, detail))
        first, _, rest = detail.partition("\n")
        print("%-5s %s%s" % (status, name, (" - " + first) if first else ""), file=self.stream, flush=True)
        if rest and status == FAIL:
            print("\n".join("      " + line for line in rest.splitlines()[:60]), file=self.stream, flush=True)

    def ok(self, name: str, detail: str = "") -> None:
        self.add(name, OK, detail)

    def fail(self, name: str, detail: str = "") -> None:
        self.add(name, FAIL, detail)

    def skip(self, name: str, detail: str = "") -> None:
        self.add(name, SKIP, detail)

    def callback(self, name: str, passed: bool, detail: str = "") -> None:
        """The form that the smoke test uses."""
        self.add(name, OK if passed else FAIL, detail)

    def count(self, status: str) -> int:
        return sum(1 for _, s, _ in self.results if s == status)

    @property
    def failed(self) -> bool:
        return self.count(FAIL) > 0

    def summary(self) -> str:
        return "%d ok, %d failed, %d skipped in %.0fs" % (self.count(OK), self.count(FAIL), self.count(SKIP), time.monotonic() - self.started)
