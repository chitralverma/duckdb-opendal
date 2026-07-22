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
| **B. OpenDAL** | release version **or** git `rev` on the two OpenDAL deps in `opendal/Cargo.toml` + `opendal/Cargo.lock` | you want newer OpenDAL, a fix, or new-service support |
| **C. The extension** | `version` in `opendal/Cargo.toml` (source of truth), mirrored to the `community-extensions` registry `description.yml`, plus the compiled-in service set (`opendal/Cargo.toml [features]`) | you ship features or fixes |

> DuckDB extensions are version-locked to DuckDB; the registry hosts one binary
> per DuckDB version and never backfills. Keep the extension current with the
> latest stable DuckDB. See [README Compatibility](README.md#compatibility).

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
3. **Bump the `extension-ci-tools` submodule** to the head of the DuckDB
   release-line branch (`v1.5-variegata` for the 1.5 line), matching
   `ci_tools_version` in the workflow. This only changes on a minor/major bump.
   Keep the `.gitmodules` `branch` hint for both `duckdb` and `extension-ci-tools`
   set to that release-line branch.
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

OpenDAL can be pinned to a **published release** (a crates.io version) or an
`apache/opendal` **git commit** (a `main` revision). It is currently pinned to a
`rev`. Move it when a target DuckDB upgrade requires a fix, or when a new service
/ OpenDAL feature is needed. Keep the two OpenDAL deps in `opendal/Cargo.toml`
(`[dependencies.opendal]` and `opendal-http-transport-reqwest`) on the **same**
release/rev.

1. **Choose the target** — a crates.io version (e.g. `0.56`) or an
   `apache/opendal` commit (`rev`).
2. **Update both deps in `opendal/Cargo.toml`**, kept identical:
   - Release → set `version = "X"` on both (drop the `git`/`rev` keys).
   - Rev → set `rev = "<sha>"` on both (`[dependencies.opendal].rev` and
     `opendal-http-transport-reqwest.rev`) with `git = "…/opendal.git"`.
3. **Refresh the lockfile**: `cargo update -p opendal` (from `opendal/`), which
   updates `opendal/Cargo.lock`.
4. **Verify**: rebuild, then `make rust-test`, `make test-local`, and the S3 tier.
5. **Adjust** the FFI/core (`opendal/src/*.rs`) for any OpenDAL API changes.
6. **Update docs**: the OpenDAL column in the
   [compatibility matrix](README.md#compatibility) (version **or** short rev) and
   the `CHANGELOG.md`.

---

## Axis C — Extension changes

Feature work (e.g. enabling a new service — see the "Adding a service" recipe in
[CONTRIBUTING.md](CONTRIBUTING.md)) and bug fixes drive a semver bump of the
extension itself. Land them normally, then cut a release.

> The extension version is `version` in `opendal/Cargo.toml` (source of truth);
> the release step mirrors it into the registry `description.yml`
> (`extensions/opendal/description.yml`), which lives in the
> [`community-extensions`](https://github.com/duckdb/community-extensions) repo,
> **not** here. `pyproject.toml` (`duckdb-opendal-tooling`, `package = false`) is
> the tooling venv — not the extension, not bumped on release.

---

## Releasing

See the [`extension-release`](.agents/skills/extension-release/SKILL.md) skill
for the full checklist. Summary:

1. **Pick the version** (semver; `0.x` may break).
2. **Bump `version`** in `opendal/Cargo.toml` (the source of truth). Build so
   `opendal/Cargo.lock` picks it up.
3. **Update `CHANGELOG.md`**: rename `[Unreleased]` to the new version with a
   date, add a fresh empty `[Unreleased]`, and refresh the compare links at the
   bottom. Seed the notes from the draft release's auto-generated notes.
4. **Update the [compatibility matrix](README.md#compatibility)** with the new row
   (extension version, DuckDB, OpenDAL release/rev, services).
5. **Commit and push `main`.** Record the commit SHA — this is the release commit.
6. **Tag `vX.Y.Z`** at that commit and push the tag. This triggers the
   `create-release-draft` job, which builds the per-platform binaries and drafts a
   GitHub release with generated notes. Review and **publish the draft**.
7. **Update the community-extensions `description.yml`**
   (`extensions/opendal/description.yml`): set `version` to `X.Y.Z` and `ref` to
   the release commit, so the registry builds exactly the tagged commit.
8. **Open/refresh the community-extensions PR** (or, if already listed, a
   follow-up PR bumping `version` + `ref`).

> **Invariant:** the git tag, the community-extensions `description.yml:ref`, and
> the deployed community binary must all point to the same commit. Cut each
> release from a single commit so they cannot drift.

---

## The `create-release-draft` job

Defined in `.github/workflows/MainDistributionPipeline.yml`, triggered on tags.
It downloads the per-platform build artifacts, flattens/renames them, and creates
a **draft** GitHub release with auto-generated notes.

It is **not** required by the community registry (which builds from source at the
community-extensions `description.yml:ref`). Kept as the project's only channel for
downloadable per-platform binaries — manual/unsigned installs, air-gapped
environments, or targeting a DuckDB the registry no longer serves (load with
`allow_unsigned_extensions`). Also anchors tags and publishes changelog notes.
