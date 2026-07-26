#!/usr/bin/env python3
"""Resolve the --test-config JSON to use for one service, and print its path.

Emulator-backed services need nothing here: their `test/configs/<name>.json` is
self-contained (the emulator credentials are public literals) and is printed
unchanged.

Real-cloud services cannot check their endpoint/credentials into git, and the
SQLLogicTest runner does **not** read the process environment when substituting
`${...}` in a config's `init_script` (only the config's own `test_env` feeds
that map, and it is materialized before on_init runs). So such a service ships
two files:

  test/configs/<name>.json                       tracked discovery stub — no
                                                 OPENDAL_BASE, no init_script,
                                                 so every common test skips
  test/services/<name>/config.template.json      same config with ${VAR}
                                                 placeholders + the init_script

When every `${VAR}` the template references is present in the environment, the
template is rendered to `build/test-configs/<name>.json` (gitignored) and that
path is printed; otherwise the stub is printed and the suite skips cleanly.
This keeps secrets out of the repo and out of the working tree.

Usage:
  render_config.py <service>
"""

import json
import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PLACEHOLDER = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")


def render(service):
    stub = os.path.join(REPO_ROOT, "test", "configs", service + ".json")
    template = os.path.join(REPO_ROOT, "test", "services", service, "config.template.json")
    if not os.path.exists(template):
        return stub

    with open(template, encoding="utf-8") as handle:
        body = handle.read()

    missing = sorted({name for name in PLACEHOLDER.findall(body) if not os.environ.get(name)})
    if missing:
        print(
            f"{service}: credentials not configured ({', '.join(missing)}) — " "tests will skip",
            file=sys.stderr,
        )
        return stub

    # json.dumps(...)[1:-1] escapes the value for a JSON string literal.
    rendered = PLACEHOLDER.sub(lambda m: json.dumps(os.environ[m.group(1)])[1:-1], body)
    out_dir = os.path.join(REPO_ROOT, "build", "test-configs")
    os.makedirs(out_dir, exist_ok=True)
    out_path = os.path.join(out_dir, service + ".json")
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(rendered)
    return out_path


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: render_config.py <service>")
    print(os.path.relpath(render(sys.argv[1]), REPO_ROOT))


if __name__ == "__main__":
    main()
