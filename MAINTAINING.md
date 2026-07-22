# Maintaining duckdb-opendal

This guide is for maintainers. It covers the three independent version axes this
extension tracks and the release process that freezes all three. For day-to-day
development (build, test, architecture, adding a service) see
[CONTRIBUTING.md](CONTRIBUTING.md).

## The three version axes

A build of this extension is the product of three inputs that move independently.
A release pins a specific value of each; the [compatibility
matrix](README.md#compatibility) records them per release.

| Axis | Pinned by | Moves when |
| --- | --- | --- |
| **A. DuckDB** (+ `extension-ci-tools`, together) | `duckdb` submodule, `extension-ci-tools` submodule, and `duckdb_version` / `ci_tools_version` in `.github/workflows/MainDistributionPipeline.yml` | DuckDB ships a new release |
| **B. OpenDAL** | git `rev` in `opendal/Cargo.toml` (**two** entries) + `opendal/Cargo.lock` | you want newer OpenDAL, a fix, or new-service support |
| **C. The extension** | `version` in `opendal/Cargo.toml` and `description.yml`, plus the compiled-in service set (`opendal/Cargo.toml [features]`) | you ship features or fixes |

> DuckDB extensions are version-locked to DuckDB. The community registry hosts a
> separate binary per DuckDB version and does not backfill old versions, so
> feature availability tracks the extension build for the user's DuckDB. Keep the
> extension current with the latest stable DuckDB (see the README Compatibility
> note).

---

## Axis A — Upgrade DuckDB (+ extension-ci-tools)

DuckDB and `extension-ci-tools` must move together. See the
[`duckdb-upgrade`](.agents/skills/duckdb-upgrade/SKILL.md) skill for the
step-by-step checklist. Summary:

1. **Choose the target tag** (e.g. `v1.5.6`).
   - **Patch** within the same minor (e.g. `v1.5.5` → `v1.5.6`): `extension-ci-tools`
     stays on the same release branch (`v1.5-variegata` for the 1.5 line).
   - **Minor/major** bump (e.g. `v1.5.x` → `v1.6.x`): `extension-ci-tools` moves to
     the new release branch (e.g. `v1.6-*`), and OpenDAL/the C++ layer may need
     compatibility fixes.
2. **Bump the `duckdb` submodule** to `tags/$TARGET`
   (`git -C duckdb fetch --tags && git -C duckdb checkout --detach tags/$TARGET`,
   then `git submodule update --init --recursive duckdb`, then `git add duckdb`).
3. **Bump the `extension-ci-tools` submodule** to the matching branch/tag, and
   update the `branch` hint in `.gitmodules` to match the workflow's
   `ci_tools_version`.
   > Known nit: `.gitmodules` currently pins `extension-ci-tools branch = v1.5.4`
   > while the workflow uses `ci_tools_version: v1.5-variegata`. Align these when
   > you next touch this axis.
4. **Update `.github/workflows/MainDistributionPipeline.yml`**: `duckdb_version`
   and `ci_tools_version` in the `duckdb-stable-build` and `code-quality-check`
   jobs (and the `@vX.Y-*` ref on the reusable `_extension_*` workflows if the
   ci-tools branch changed).
5. **Update docs**: the submodule-pin note in `CONTRIBUTING.md` and the
   [compatibility matrix](README.md#compatibility).
6. **Verify**: `make format-all`, then build (`GEN=ninja make`), then
   `make rust-test`, `make test-local`, and the S3 tier
   (`make s3-up && make test-common-s3 && make s3-assert-no-incomplete && make s3-down`).
7. **Review** `git diff --submodule=log` and note any C-API compatibility changes
   in the commit / PR body.
8. Proceed to **[Releasing](#releasing)**.

---

## Axis B — Upgrade OpenDAL

OpenDAL is pinned to an `apache/opendal` **commit** (a `main` revision), not a
published release. It should only move when a target DuckDB upgrade requires a
fix, or when a new service / OpenDAL feature is needed.

1. **Choose the target `apache/opendal` commit** (`rev`).
2. **Update both `rev` entries in `opendal/Cargo.toml`** — keep them identical:
   - `[dependencies.opendal].rev`
   - `opendal-http-transport-reqwest.rev`
3. **Refresh the lockfile**: `cargo update -p opendal`
   (from `opendal/`), which updates `opendal/Cargo.lock`.
4. **Verify**: rebuild, then `make rust-test`, `make test-local`, and the S3 tier.
5. **Adjust** the FFI/core (`opendal/src/*.rs`) for any OpenDAL API changes.
6. **Update docs**: the OpenDAL `rev` column in the
   [compatibility matrix](README.md#compatibility) and the `CHANGELOG.md`.

---

## Axis C — Extension changes

Feature work (e.g. enabling a new service — see the "Adding a service" recipe in
[CONTRIBUTING.md](CONTRIBUTING.md)) and bug fixes drive a semver bump of the
extension itself. Land them normally, then cut a release.

---

## Releasing

See the [`extension-release`](.agents/skills/extension-release/SKILL.md) skill
for the full checklist. Summary:

1. **Pick the version** (semver; `0.x` may break).
2. **Bump `version`** in **both** `opendal/Cargo.toml` and `description.yml`
   (keep them equal).
3. **Update `CHANGELOG.md`**: rename `[Unreleased]` to the new version with a
   date, add a fresh empty `[Unreleased]`, and refresh the compare links at the
   bottom. Seed the notes from the draft release's auto-generated notes.
4. **Update the [compatibility matrix](README.md#compatibility)** with the new row
   (extension version, DuckDB, OpenDAL `rev`, services).
5. **Commit and push `main`.**
6. **Tag `vX.Y.Z`** at that commit and push the tag. This triggers the
   `create-release-draft` job, which builds the per-platform binaries and drafts a
   GitHub release with generated notes. Review and **publish the draft**.
7. **Bump `description.yml:ref`** to the released commit — in this repo and in the
   `community-extensions` registry entry.
8. **Open/refresh the community-extensions PR** (or, if already listed, a
   follow-up PR that only bumps the `ref`).

> **Invariant:** the git tag, `description.yml:ref`, and the deployed community
> binary must all point to the same commit.
>
> Current drift to fix on the next release: the `v0.1.0` tag (`0d4e1fac`) is
> **behind** the deployed community ref (`6e998ef`). Cut the next version from a
> single commit so the tag and `ref` match.

---

## The `create-release-draft` job

Defined in `.github/workflows/MainDistributionPipeline.yml`, triggered on tags.
It downloads the per-platform build artifacts, flattens/renames them, and creates
a **draft** GitHub release with auto-generated notes.

It is **not** required by the community registry (which builds from source at
`description.yml:ref`). It is kept because it is the project's only channel for
**downloadable, per-platform binaries** — useful for manual/unsigned installs,
air-gapped environments, or targeting a DuckDB version the registry no longer
serves (load with `allow_unsigned_extensions`). It also anchors version tags and
publishes changelog notes.
