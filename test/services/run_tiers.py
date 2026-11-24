#!/usr/bin/env python3
"""Run every SQL test tier in the plan (plan.py), the way CI does.

Prints the plan grouped by kind, runs each tier in its own log group, then
prints one result line per tier. Under GitHub Actions the results also go to
the job summary as a table.

Per kind:
  local     make test-common-<svc>
  emulator  make <svc>-up, test-common-<svc>, <svc>-verify, then always
            <svc>-down so the next tier can reuse ports
  cloud     make test-common-<svc> (its config renders only with credentials;
            see render_config.py)

Every tier runs even after a failure; the exit code is non-zero if any failed.
A cloud tier without credentials passes by skipping every test, so it is
reported as "skipped" rather than "passed".

Usage:
  run_tiers.py [--have-secrets true|false] [service ...]
"""

import argparse
import os
import subprocess
import sys
import time

# Siblings in test/services/, importable because Python puts the script's own
# directory on sys.path.
import plan
import render_config

IN_ACTIONS = os.environ.get("GITHUB_ACTIONS") == "true"


def make(target):
    print(f"$ make {target}", flush=True)
    return subprocess.run(["make", target], check=False, cwd=plan.REPO_ROOT).returncode == 0


def run_tier(service):
    """One of: passed, skipped (a cloud tier without credentials), failed (<step>)."""
    name = service["name"]
    if service["kind"] != "emulator":
        if not make(f"test-common-{name}"):
            return "failed (tests)"
        return "skipped" if render_config.missing_credentials(name) else "passed"
    result = "passed"
    for step, target in (
        ("up", f"{name}-up"),
        ("tests", f"test-common-{name}"),
        ("verify", f"{name}-verify"),
    ):
        if not make(target):
            result = f"failed ({step})"
            break
    make(f"{name}-down")
    return result


def group(title):
    if IN_ACTIONS:
        print(f"::group::{title}", flush=True)
    else:
        print(f"\n== {title}", flush=True)


def endgroup():
    if IN_ACTIONS:
        print("::endgroup::", flush=True)


ICONS = {"passed": "✅", "skipped": "⏭️"}


def summary(results, dropped):
    lines = [
        "### SQL test tiers",
        "",
        "| Service | Kind | Backend | Result | Time |",
        "| --- | --- | --- | --- | --- |",
    ]
    for service, result, seconds in results:
        cells = [f"`{service['name']}`", service["kind"], service["backend"]]
        cells += [f"{ICONS.get(result, '❌')} {result}", f"{seconds:.0f}s"]
        lines.append("| " + " | ".join(cells) + " |")
    for service in dropped:
        cells = [f"`{service['name']}`", service["kind"], service["backend"], "⏭️ not run (no secrets)", ""]
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--have-secrets", choices=["true", "false"], default="true")
    parser.add_argument("services", nargs="*", help="run only these (default: the whole plan)")
    args = parser.parse_args()

    services, dropped = plan.plan(args.have_secrets == "true")
    if args.services:
        unknown = sorted(set(args.services) - {b["name"] for b in services + dropped})
        if unknown:
            sys.exit(f"unknown service(s): {' '.join(unknown)}")
        services = [b for b in services if b["name"] in args.services]
        dropped = [b for b in dropped if b["name"] in args.services]

    print("Test tiers")
    print(plan.table(services, dropped), flush=True)

    results = []
    for service in services:
        group(f"{service['name']} ({service['kind']}: {service['backend']})")
        start = time.monotonic()
        result = run_tier(service)
        results.append((service, result, time.monotonic() - start))
        endgroup()
        if result.startswith("failed") and IN_ACTIONS:
            print(f"::error title=SQL tier {service['name']}::{service['name']} {result}", flush=True)

    width = max((len(b["name"]) for b in services), default=0)
    print("\nResults")
    for service, result, seconds in results:
        print(f"  {service['name']:<{width}}  {service['kind']:<8}  {result:<15}  {seconds:4.0f}s")
    failures = [service["name"] for service, result, _ in results if result.startswith("failed")]
    counts = {r: sum(1 for _, result, _ in results if result == r) for r in ("passed", "skipped")}
    line = f"{counts['passed']} passed, {counts['skipped']} skipped, {len(failures)} failed"
    print(line + (f": {' '.join(failures)}" if failures else ""))

    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as handle:
            handle.write(summary(results, dropped))

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
