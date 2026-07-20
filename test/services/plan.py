#!/usr/bin/env python3
"""Plan the SQL test matrix from the test/ layout (see docs/testing.md).

A "service" is any test/configs/<name>.json. For each one we derive:

  name           the scheme/service name (config file stem)
  config         path to the --test-config JSON
  service_test   test/sql/services/<name>.test, if present
  compose        test/services/<name>/docker-compose.yml, if present
  needs_secrets  True if provisioning needs real cloud credentials
                 (marker file test/services/<name>/requires-secrets)

Services with a compose file are emulator-backed (run on every PR, incl. forks).
Services with a requires-secrets marker only run when secrets are available;
`--have-secrets false` drops them so fork PRs still run the emulator tiers.

Adding a service is data-only: drop in test/configs/<name>.json (+ optional
test/services/<name>/docker-compose.yml and test/sql/services/<name>.test) and
the planner picks it up.

Usage:
  plan.py [--have-secrets true|false]        # JSON array (GitHub matrix include)
  plan.py --names                            # all service names, space-separated
  plan.py --provisioned                      # names that have a compose file
"""

import argparse
import json
import os
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CONFIGS_DIR = os.path.join(REPO_ROOT, "test", "configs")
SERVICES_DIR = os.path.join(REPO_ROOT, "test", "services")
SERVICE_TESTS_DIR = os.path.join(REPO_ROOT, "test", "sql", "services")


def _rel(path):
    return os.path.relpath(path, REPO_ROOT)


def discover():
    services = []
    for entry in sorted(os.listdir(CONFIGS_DIR)):
        if not entry.endswith(".json"):
            continue
        name = entry[: -len(".json")]
        compose = os.path.join(SERVICES_DIR, name, "docker-compose.yml")
        service_test = os.path.join(SERVICE_TESTS_DIR, name + ".test")
        marker = os.path.join(SERVICES_DIR, name, "requires-secrets")
        services.append(
            {
                "name": name,
                "config": _rel(os.path.join(CONFIGS_DIR, entry)),
                "service_test": _rel(service_test) if os.path.exists(service_test) else "",
                "compose": _rel(compose) if os.path.exists(compose) else "",
                "needs_secrets": os.path.exists(marker),
            }
        )
    return services


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--have-secrets", choices=["true", "false"], default="true")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--names", action="store_true", help="print all service names")
    group.add_argument("--provisioned", action="store_true", help="print names with a compose file")
    args = parser.parse_args()

    services = discover()
    if args.have_secrets == "false":
        services = [b for b in services if not b["needs_secrets"]]

    if args.names:
        print(" ".join(b["name"] for b in services))
    elif args.provisioned:
        print(" ".join(b["name"] for b in services if b["compose"]))
    else:
        json.dump(services, sys.stdout)
        sys.stdout.write("\n")


if __name__ == "__main__":
    main()
