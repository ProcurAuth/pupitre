# SPDX-License-Identifier: MIT OR Apache-2.0
"""Shared helpers of the unit tests of the acceptance harness: the paths and the loader of a module by path."""
import importlib.util
import os

HERE = os.path.dirname(os.path.abspath(__file__))
ACCEPTANCE = os.path.dirname(HERE)
ROOT = os.path.dirname(os.path.dirname(ACCEPTANCE))


def load(name: str, path: str):
    """Load the Python file at `path` as the module `name`, with no search of sys.path."""
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def generator():
    """The module `tests/fixtures/generate.py`."""
    return load("generate", os.path.join(ROOT, "tests", "fixtures", "generate.py"))
