# Contributing to duckdb-opendal

Developer guide: how the extension is put together, how to build it, and how to
test it. For user-facing features and SQL usage, see the [README](README.md). For
maintainer tasks — upgrading DuckDB / OpenDAL and cutting releases — see
[MAINTAINING.md](MAINTAINING.md).

## Architecture

A C++ DuckDB `FileSystem` shell over the Apache OpenDAL **Rust** core, bridged by
a thin `extern "C"` FFI:

- **C++ shell** (`src/`) — `OpenDalFileSystem : duckdb::FileSystem`, registered
  via `RegisterSubSystem`, plus the secrets and table functions. Delegates every
  operation to the Rust core.
- **Rust core** (`opendal/`) — a `staticlib` crate (`duckdb-opendal`) wrapping the
  OpenDAL Rust crate and exposing `od_*` `extern "C"` functions. A single
  multi-thread Tokio runtime bridges OpenDAL's async API to DuckDB's sync FS API
  (`block_on`); every FFI entry is wrapped in `catch_unwind`.
- **Bridge** — [cbindgen](https://github.com/mozilla/cbindgen) generates
  `src/include/rust.h` at build time; [Corrosion](https://github.com/corrosion-rs/corrosion)
  drives `cargo` from CMake and links the staticlib into the extension.

```
src/                       C++ shell (.cpp) + include/ (.hpp, generated rust.h)
opendal/                   Rust crate: src/*.rs, Cargo.toml ([features] select services)
test/sql/common/           functionality tests, run against every service
test/sql/services/         service-specific quirks only
test/configs/              per-service --test-config JSON + auth_<svc>.sql
test/services/             docker-compose provisioning + plan.py + shared fixtures/
duckdb/, extension-ci-tools/   submodules on the v1.5 line (v1.5-variegata)
```

> Generated files (`src/include/rust.h`) are produced by cbindgen — never hand-edit.

## Prerequisites

- **Rust** (stable, MSRV 1.91+)
- **CMake** 3.22+
- **Compiler**: GCC/Clang (Linux/macOS) or MSVC (Windows)
- **Ninja** or Make
- **Docker** (only for the emulator test tiers)
- **[uv](https://github.com/astral-sh/uv)** (only for formatting tools)

## Building

```sh
git clone --recurse-submodules https://github.com/chitralverma/duckdb-opendal.git
cd duckdb-opendal

make                # or: GEN=ninja make   (recommended)
```

Produces:
- `./build/release/duckdb` — DuckDB shell with the extension preloaded
- `./build/release/extension/opendal/opendal.duckdb_extension` — the loadable binary

The default build compiles the crate's `default` feature set; build a subset
with `cargo build --no-default-features --features services-s3`. The service
list lives in `opendal/Cargo.toml [features]`, and `FROM opendal_schemes();`
reports what a binary ended up with.

## Testing

### Rust unit tests

```sh
make rust-test
```

### SQL tests (SQLLogicTest)

The test suite is **functionality-first**: one common suite in `test/sql/common/`
runs against **every service**. Each service has a `test/configs/<name>.json`
that binds `${OPENDAL_BASE}` (and any service params) and creates its secret via
`init_script` (`auth_<name>.sql`). Service-specific quirks live in
`test/sql/services/<name>.test`.

`make test-common-<name>` runs the common suite (plus that service's quirks test,
if any) against one service by setting `DUCKDB_TEST_CONFIG=test/configs/<name>.json`.

```sh
make test-local            # fs + memory (no infrastructure)
make test-common-fs        # a single service
make test-common-memory
```

> Note: plain `make test` (from extension-ci-tools) runs `test/*` with no config,
> so the common tests **skip** (they require `${OPENDAL_BASE}`). Use
> `make test-local` / `make test-common-<name>` for the real suite.

### Service tiers

A tier is either **provisioned** (an emulator in Docker) or **credential-gated**
(a real cloud account). Everything a tier needs lives in `test/services/<svc>/`;
no Makefile or CI target names a service.

```sh
make s3-up            # start + provision (any provisioned service)
make test-common-s3   # the common suite (+ services/s3.test, if present)
make s3-verify        # the service's post-test checks, if it has any
make s3-down          # stop and remove
```

**Provisioned services** — `test/services/<svc>/`:

| File                 | Role                                                          |
| -------------------- | ------------------------------------------------------------- |
| `docker-compose.yml` | Required. `<svc>-up` runs `up -d --wait`, so long-running services need healthchecks. |
| service named `init` | Optional one-shot provisioning (buckets, fixtures). Put it behind `profiles: ["init"]`; `<svc>-up` runs it after the rest is healthy. |
| `ready.sh`           | Optional readiness gate for images compose cannot healthcheck (no shell). Runs after `up`, before `init`. |
| `verify.sh`          | Optional post-test assertion; `<svc>-verify` runs it, CI runs it after the suite. |

Both hooks are run as `sh <script>` (POSIX sh, no exec bit required), so a
forgotten `chmod +x` cannot silently drop a readiness gate or an assertion.

**Read-only services** (`http`, for example) cannot run the common suite: it
seeds itself by writing, and `external_read.test` finishes with a glob. Such a
config sets neither `OPENDAL_BASE` nor `OPENDAL_EXTERNAL_BASE` — the
service-agnostic common tests still run, and the read/stat/capability coverage
goes in `test/sql/services/<svc>.test`.

**Credential-gated services** — no compose; an empty `requires-secrets` marker
makes the planner drop them when secrets are unavailable (fork PRs). They skip
unless their env vars are set:

```sh
export OPENDAL_AZDLS_HOST=myaccount.dfs.core.windows.net
export OPENDAL_AZDLS_FILESYSTEM=odfs-test
export OPENDAL_AZDLS_ACCOUNT_KEY=...
make test-common-azdls
```

In CI those come from a single repository secret, `OPENDAL_TEST_ENV`, holding
`KEY=value` lines for every such service; the workflow masks each value and
exports it. Adding a service means editing that secret, not the workflow.

The runner only substitutes `${...}` in an `init_script` from the config's own
`test_env` (materialized *before* `on_init` runs) — never from the process
environment. So a credential-gated service ships two files and `test-common-<svc>`
picks between them via `test/services/render_config.py`:

- `test/configs/<svc>.json` — tracked stub: no `OPENDAL_BASE`, no `init_script`,
  so every common test skips cleanly;
- `test/services/<svc>/config.template.json` — the real config with `${VAR}`
  placeholders, rendered `0600` to `build/test-configs/<svc>.json` (gitignored)
  when every referenced var is set.

A skipped tier looks exactly like a passing one, so an expired secret would
quietly drop coverage to zero. `OPENDAL_REQUIRE_CLOUD_TIERS=1` turns a missing
credential into a hard failure. CI applies it on `main` pushes and the scheduled
run — but only when the repo variable `OPENDAL_REQUIRE_CLOUD_TIERS` is set to
`1`, so it is opt-in: turn it on once `OPENDAL_TEST_ENV` actually carries every
credential-gated service, otherwise the first push to `main` fails on the
missing secret.

### External-object fixtures

`make fixtures` generates `test/services/fixtures/` once, shared by every
service; `<svc>-up` depends on it. A tier exposes them under its own
`external/` prefix however its backend allows (MinIO copies them in from the
`init` service, fake-gcs-server mounts them, Azurite uploads them over REST) and
sets `OPENDAL_EXTERNAL_BASE` in its config, which makes
`common/external_read.test` run. Never regenerate fixtures per service.

### How env substitution works

The SQLLogicTest runner substitutes `${VAR}`/`{VAR}` anywhere in a test body from
its env map. A variable is only substituted if the file registers it with
`require-env VAR` (skips the test when unset — the availability gate) or
`test-env VAR <default>`. Config-JSON `test_env` values feed these. This is why
common tests `require-env OPENDAL_BASE` and skip cleanly when run without a config.

Each I/O test also declares `set ignore_error_messages` (clears the SQLLogicTest
default), so service HTTP/connection errors **fail loudly** instead of being
silently skipped — important for remote/object-store services.

## Adding a service

Enable OpenDAL service `<svc>` **without touching `test/sql/common/*`** (each is
its own PR — see [issue #6](https://github.com/chitralverma/duckdb-opendal/issues/6)):

1. **Cargo feature** — `services-<svc> = ["opendal/services-<svc>"]` in
   `opendal/Cargo.toml [features]`, and in `default` if it should build by default.
2. **Config** — `test/configs/<svc>.json`: `OPENDAL_BASE` (+ service params) in
   `test_env`, `init_script: test/configs/auth_<svc>.sql`,
   `statically_loaded_extensions: ["core_functions", "parquet", "opendal"]`.
3. **Secret** — `test/configs/auth_<svc>.sql`: one `CREATE SECRET (TYPE <svc>, …)`
   referencing `${…}` from `test_env` (pure SQL).
4. **Tier** — a `test/services/<svc>/` directory following the conventions in
   [Service tiers](#service-tiers): a compose file for an emulator, or a
   `requires-secrets` marker plus `config.template.json` for a real-cloud service.
5. **External reads (optional)** — expose the shared fixtures under `external/`
   and set `OPENDAL_EXTERNAL_BASE`; see [External-object fixtures](#external-object-fixtures).
6. **Quirks test (optional)** — `test/sql/services/<svc>.test`, gated on an env
   var only that config sets. `make test-common-<svc>` runs it automatically.
7. **Docs** — `docs/services/<svc>.md`, following the template in
   [docs/services/README.md](docs/services/README.md), and linked from its table.

`test/services/plan.py` discovers the service from `test/configs/<svc>.json`;
the CI `sql-tests` job then runs it with no workflow change (emulators on every
PR including forks, credential-gated services only where secrets exist).

## Formatting & linting

```sh
uv run make format-all   # C++ (clang-format 11.0.1) + Rust (rustfmt)
make rust-lint           # clippy -D warnings
uv run ruff format test/services && uv run ruff check test/services   # settings in pyproject.toml
```

CI runs the C++ format/tidy checks, the Rust format/clippy checks, and the SQL
test tiers on every PR. Keep changes formatter-clean.

## License

By contributing you agree your contributions are licensed under the project's
[MIT License](LICENSE).
