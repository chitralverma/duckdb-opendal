#!/usr/bin/env python3
"""Plan the SQL test matrix from the test/ layout (see CONTRIBUTING.md, Testing).

A "service" is any test/configs/<name>.json. For each one we derive:

  name           the scheme/service name (config file stem)
  config         path to the --test-config JSON
  service_test   test/sql/services/<name>.test, if present
  compose        test/services/<name>/docker-compose.yml, if present
  needs_secrets  True if provisioning needs real cloud credentials
                 (marker file test/services/<name>/requires-secrets)
  kind           how the tier gets its backend, from the two above:
                   local     nothing to start (fs, memory)
                   emulator  started from the compose file
                   cloud     a real account, credentials from the environment
  backend        what it runs against: the "(...)" that ends the config's
                 description, up to any "; note"

Emulator tiers run on every PR, incl. forks. Cloud tiers only run when secrets
are available; `--have-secrets false` drops them so fork PRs still run the
emulator tiers.

Adding a service is data-only: drop in test/configs/<name>.json (+ optional
test/services/<name>/docker-compose.yml and test/sql/services/<name>.test) and
the planner picks it up. run_tiers.py runs the plan.

Usage:
  plan.py [--have-secrets true|false]        # JSON array (GitHub matrix include)
  plan.py --table                            # the plan, grouped by kind
  plan.py --names                            # all service names, space-separated
  plan.py --provisioned                      # names that have a compose file
"""

import argparse
import json
import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CONFIGS_DIR = os.path.join(REPO_ROOT, "test", "configs")
SERVICES_DIR = os.path.join(REPO_ROOT, "test", "services")
SERVICE_TESTS_DIR = os.path.join(REPO_ROOT, "test", "sql", "services")

# Plan order, and what each kind needs.
KINDS = {
    "local": "nothing to start",
    "emulator": "docker compose",
    "cloud": "real account",
}
BACKEND = re.compile(r"\(([^()]*)\)\s*$")


def _rel(path):
    return os.path.relpath(path, REPO_ROOT)


def _backend(config):
    with open(config, encoding="utf-8") as handle:
        match = BACKEND.search(json.load(handle).get("description", ""))
    return match.group(1).split(";")[0].strip() if match else ""


def discover():
    services = []
    for entry in sorted(os.listdir(CONFIGS_DIR)):
        if not entry.endswith(".json"):
            continue
        name = entry[: -len(".json")]
        config = os.path.join(CONFIGS_DIR, entry)
        compose = os.path.join(SERVICES_DIR, name, "docker-compose.yml")
        service_test = os.path.join(SERVICE_TESTS_DIR, name + ".test")
        marker = os.path.join(SERVICES_DIR, name, "requires-secrets")
        if os.path.exists(compose):
            kind = "emulator"
        elif os.path.exists(marker):
            kind = "cloud"
        else:
            kind = "local"
        services.append(
            {
                "name": name,
                "config": _rel(config),
                "service_test": _rel(service_test) if os.path.exists(service_test) else "",
                "compose": _rel(compose) if os.path.exists(compose) else "",
                "needs_secrets": os.path.exists(marker),
                "kind": kind,
                "backend": _backend(config),
            }
        )
    order = list(KINDS)
    return sorted(services, key=lambda b: (order.index(b["kind"]), b["name"]))


def plan(have_secrets=True):
    """(services to run, services dropped for lack of secrets), in plan order."""
    services = discover()
    if have_secrets:
        return services, []
    return [b for b in services if not b["needs_secrets"]], [b for b in services if b["needs_secrets"]]


def table(services, dropped):
    width = max((len(b["name"]) for b in services + dropped), default=0)
    lines = []
    for kind, needs in KINDS.items():
        group = [b for b in services if b["kind"] == kind]
        if group:
            lines.append(f"{kind} ({needs}): {len(group)}")
            lines.extend(f"  {b['name']:<{width}}  {b['backend']}".rstrip() for b in group)
    if dropped:
        lines.append(f"not run (no secrets in this run): {len(dropped)}")
        lines.extend(f"  {b['name']:<{width}}  {b['backend']}".rstrip() for b in dropped)
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--have-secrets", choices=["true", "false"], default="true")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--table", action="store_true", help="print the plan grouped by kind")
    group.add_argument("--names", action="store_true", help="print all service names")
    group.add_argument("--provisioned", action="store_true", help="print names with a compose file")
    args = parser.parse_args()

    services, dropped = plan(args.have_secrets == "true")

    if args.table:
        print(table(services, dropped))
    elif args.names:
        print(" ".join(b["name"] for b in services))
    elif args.provisioned:
        print(" ".join(b["name"] for b in services if b["compose"]))
    else:
        json.dump(services, sys.stdout)
        sys.stdout.write("\n")


if __name__ == "__main__":
    main()
