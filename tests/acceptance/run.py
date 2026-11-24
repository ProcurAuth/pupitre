#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The runner of the acceptance tests (8.12). Python standard library only. Docker runs everything.

Use: python3 -I tests/acceptance/run.py --bin PATH [--stage STAGE]... [--scenario NAME]... [--require-server]

Stages: images, configs, smoke, scenarios. The default is all of them.
The scenarios need `pupitre serve`. While `serve` is a stub, the stage `scenarios` is skipped with a message.
With `--require-server`, a stub is a failure. The integrator uses this option after the merge of the server.

The run makes only throwaway containers and one private network with the prefix `pupitre-acc-<run>-`. It removes them at
the end, also after an error, and it checks that nothing is left. It leaves the local images `pupitre-acc-host` and
`pupitre-acc-runner` as a build cache. `--remove-images` removes them. `--cleanup RUN` removes what the run RUN left
after a kill of the runner.
"""
import argparse
import os
import secrets
import signal
import sys
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from harness import stages  # noqa: E402
from harness.dockerctl import Docker  # noqa: E402
from harness.lab import Lab  # noqa: E402
from harness.report import Report  # noqa: E402
import scenarios  # noqa: E402

ALL_STAGES = ("images", "configs", "smoke", "scenarios")


def log(text: str) -> None:
    print("      " + text, file=sys.stderr, flush=True)


def load_generator(repo: str):
    import importlib.util

    spec = importlib.util.spec_from_file_location("generate", os.path.join(repo, "tests", "fixtures", "generate.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def cleanup_run(run_id: str) -> int:
    """Remove the containers and the networks that carry the label of the run `run_id`. Nothing else."""
    docker = Docker(run_id, log)
    label = "label=pupitre-acc.run=%s" % run_id
    containers = docker.cmd(["ps", "--all", "--quiet", "--filter", label], check=False).out.split()
    networks = docker.cmd(["network", "ls", "--quiet", "--filter", label], check=False).out.split()
    for item in containers:
        docker.cmd(["rm", "--force", "--volumes", item], check=False)
    for item in networks:
        docker.cmd(["network", "rm", item], check=False)
    print("removed %d containers and %d networks of the run %s" % (len(containers), len(networks), run_id))
    return 0


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin", help="The static binary of Pupitre (scripts/static.sh).")
    parser.add_argument("--stage", action="append", choices=ALL_STAGES, help="A stage to run. Repeat the option for more stages.")
    parser.add_argument("--scenario", action="append", help="A scenario to run. Repeat the option for more scenarios.")
    parser.add_argument("--require-server", action="store_true", help="Fail, do not skip, when `serve` is a stub.")
    parser.add_argument("--remove-images", action="store_true", help="Remove the local images of the harness at the end.")
    parser.add_argument("--seed", type=int, default=7, help="The seed of the large fixture.")
    parser.add_argument("--fixture-no-history", action="store_true", help="Make the large fixture with no versions in .history.")
    parser.add_argument("--cleanup", metavar="RUN", help="Remove what the run RUN left, and stop.")
    parser.add_argument("--list", action="store_true", help="List the scenarios and stop.")
    args = parser.parse_args(argv)

    if args.cleanup:
        return cleanup_run(args.cleanup)
    if args.list:
        for name, _, needs in scenarios.catalog():
            print("%-28s %s" % (name, needs))
        return 0
    if not args.bin or not os.path.isfile(args.bin):
        parser.error("--bin must name the binary of Pupitre")
    stages_wanted = args.stage or list(ALL_STAGES)
    repo = os.path.dirname(os.path.dirname(HERE))

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    run_id = secrets.token_hex(4)
    report = Report()
    lab = Lab(repo, args.bin, run_id, log)
    print("run %s" % run_id, flush=True)
    try:
        if _build(lab, report):
            report.ok("images: build the host and the runner image", "%s, %s" % (lab.host_image, lab.runner_image))
            if any(s in stages_wanted for s in ("configs", "smoke", "scenarios")):
                lab.prepare()
                generator = load_generator(repo)
                fixture = _make_fixture(lab, generator, args.seed, not args.fixture_no_history)
                if "configs" in stages_wanted:
                    stages.check_configs(lab, report, fixture)
                if "smoke" in stages_wanted and not report.failed:
                    stages.run_smoke(lab, report)
                if "scenarios" in stages_wanted and not report.failed:
                    scenarios.run_all(lab, report, args, generator)
    except KeyboardInterrupt:
        report.fail("run", "interrupted")
    except Exception:  # a defect of the harness or of Docker: report it with the traceback, then clean up
        report.fail("run", traceback.format_exc())
    finally:
        problems = lab.close()
        if problems:
            report.fail("cleanup: no container, volume or network is left", "\n".join(problems))
        else:
            report.ok("cleanup: no container, volume or network is left", "run %s" % run_id)
        if args.remove_images:
            for tag in (lab.host_image, lab.runner_image):
                if tag:
                    lab.docker.remove_image(tag)
    print(report.summary())
    return 1 if report.failed else 0


def _build(lab: Lab, report: Report) -> bool:
    try:
        lab.build_images()
    except Exception:
        report.fail("images: build the host and the runner image", traceback.format_exc())
        return False
    return True


def _make_fixture(lab: Lab, generator, seed: int, history: bool = True) -> str:
    """Make the large fixture below the lab folder. Return the blocks `schema` for the configuration `large`."""
    profile = generator.PROFILES["large"]
    files = generator.build(seed, profile, marker=False, history=history)
    generator.write(files, lab.path("fixture"))
    blocks = generator.hcl_blocks(profile, "/srv/fixture/schemas")
    lab.fixture_digest = generator.tree_digest(files)
    lab.fixture_counts = generator.counts(files)
    return blocks


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
