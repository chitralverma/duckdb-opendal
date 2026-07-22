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
duckdb/, extension-ci-tools/   pinned submodules (v1.5.5)
```

> Generated files (`src/include/rust.h`) are produced by cbindgen — never hand-edit.

## Prerequisites

- **Rust** (stable, MSRV 1.91+)
- **CMake** 3.22+
- **Compiler**: GCC/Clang (Linux/macOS) or MSVC (Windows)
- **Ninja** or Make
- **Docker** (only for the S3 test tier)
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

The default build compiles all services in the crate's `default` feature set
(`memory`, `fs`, `s3`). Build a subset with
`cargo build --no-default-features --features services-s3` (see
`opendal/Cargo.toml [features]`).

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

### S3 tier (MinIO via Docker)

```sh
make s3-up                    # start MinIO + fault proxy, create buckets, seed fixtures
make test-common-s3           # common suite + services/s3.test against s3://
make s3-assert-no-incomplete  # verify the abort test left no orphaned uploads
make s3-down
```

Provisioning lives entirely in `test/services/s3/docker-compose.yml` (MinIO + a
one-shot bucket-init + a fault proxy). `make s3-up` also runs `make fixtures`,
which generates the shared external-object fixtures uploaded to `s3://warehouse/external/`.

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

1. **Cargo feature** — add `services-<svc> = ["opendal/services-<svc>"]` to
   `opendal/Cargo.toml [features]` (and to `default` if it should build by default).
2. **Config** — `test/configs/<svc>.json`: set `OPENDAL_BASE` (+ service params) in
   `test_env`; `init_script: test/configs/auth_<svc>.sql`;
   `statically_loaded_extensions: ["core_functions", "parquet", "opendal"]`.
3. **Secret** — `test/configs/auth_<svc>.sql`: one `CREATE SECRET (TYPE <svc>, …)`
   referencing `${…}` from `test_env` (pure SQL).
4. **Provisioning** — for an emulator, add `test/services/<svc>/docker-compose.yml`
   (service + a way to create the bucket/container) and Makefile targets
   `<svc>-up` / `<svc>-down` (+ `<svc>-assert-no-incomplete` if relevant). `<svc>-up`
   must **not return until the service is ready** — readiness is service-specific:
   a container with a shell can use a compose `healthcheck` + `up --wait` (like
   `s3-up`); an image without one (e.g. `fsouza/fake-gcs-server`) needs a
   host-side poll (`curl` the endpoint in a loop) in `<svc>-up`. For a
   real-cloud-only service, skip the compose and add an empty
   `test/services/<svc>/requires-secrets` marker so the planner gates it.
5. **External-object reads (optional)** — expose the shared `test/services/fixtures/`
   as objects under `<service>/external/` and set `OPENDAL_EXTERNAL_BASE` in the
   config; `common/external_read.test` then runs automatically. The mechanism is
   emulator-specific: MinIO uses an `mc cp` init step; `fake-gcs-server` auto-loads
   a mounted `/data/<bucket>/external/` folder — either way, mount the shared
   `test/services/fixtures/` (don't regenerate per service).
6. **Quirks test (optional)** — `test/sql/services/<svc>.test`, gated on an env var
   only that config sets. `make test-common-<svc>` runs it automatically.

`test/services/plan.py` discovers the new service and the CI `sql-tests` job runs
it (emulators on every PR incl. forks; secret-gated services only when secrets
are present).

## Formatting & linting

```sh
uv run make format-all   # C++ (clang-format 11.0.1) + Rust (rustfmt)
make rust-lint           # clippy -D warnings
ruff format test/services/**/*.py && ruff check test/services/**/*.py
```

CI runs the C++ format/tidy checks, the Rust format/clippy checks, and the SQL
test tiers on every PR. Keep changes formatter-clean.

## License

By contributing you agree your contributions are licensed under the project's
[MIT License](LICENSE).
