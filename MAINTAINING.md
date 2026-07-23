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
| **A. DuckDB** (+ `extension-ci-tools`, together) | `duckdb_version` / `ci_tools_version` in `.github/workflows/MainDistributionPipeline.yml` + the `duckdb` / `extension-ci-tools` submodules — all pointed at the DuckDB **release-line branch** (`v1.5-variegata`) | DuckDB ships a new **minor** (patches auto-tracked) |
| **B. OpenDAL** | release version **or** git `rev` on the two OpenDAL deps in `opendal/Cargo.toml` + `opendal/Cargo.lock` | you want newer OpenDAL, a fix, or new-service support |
| **C. The extension** | `version` in `opendal/Cargo.toml` (source of truth), mirrored to the `community-extensions` registry `description.yml`, plus the compiled-in service set (`opendal/Cargo.toml [features]`) | you ship features or fixes |

> DuckDB extensions are version-locked to DuckDB; the registry hosts one binary
> per DuckDB version and never backfills. Keep the extension current with the
> latest stable DuckDB. See [README Compatibility](README.md#compatibility).

---

## Axis A — Upgrade DuckDB (+ extension-ci-tools)

The pipeline tracks the DuckDB **release-line branch** (`v1.5-variegata`), not a
pinned patch tag. So:

| Event | Action |
| --- | --- |
| **Patch** (`v1.5.5` → `v1.5.6`) | **Nothing** — the daily run covers it. |
| **Minor** (`v1.5.x` → `v1.6.x`) | Flip to the new line branch (see below). |

**Daily health check:** the `schedule` cron in `MainDistributionPipeline.yml`
rebuilds + tests against the current `v1.5-variegata` head. It runs on `main`
(not a tag) → `create-release-draft` skipped, nothing published. A red run = a new
DuckDB patch broke us → fix, then cut a release.

### Minor bump (only) — see the [`duckdb-upgrade`](.agents/skills/duckdb-upgrade/SKILL.md) skill

1. **New line branch** = `vX.Y-<codename>` (e.g. `v1.6-*`). Find it:
   `git ls-remote --heads https://github.com/duckdb/duckdb "vX.Y*"`.
2. **`.gitmodules`**: set the `branch` hint for both `duckdb` and
   `extension-ci-tools` to the new line branch.
3. **Submodules**: point both at the new line-branch head
   (`git -C <sub> fetch --depth 1 origin <line-branch>` →
   `git -C <sub> checkout --detach FETCH_HEAD` → `git add <sub>`).
4. **`MainDistributionPipeline.yml`**: `duckdb_version`, `ci_tools_version`, and the
   `@vX.Y-*` ref on the reusable `_extension_*` workflows → new line branch.
5. **Docs**: submodule-pin note in `CONTRIBUTING.md`, the
   [compatibility matrix](README.md#compatibility), and `CHANGELOG.md`.
6. **Verify**: `make format-all`, build (`GEN=ninja make`), then `make rust-test`,
   `make test-local`, and the S3 tier
   (`make s3-up && make test-common-s3 && make s3-assert-no-incomplete && make s3-down`).
7. **Review** `git diff --submodule=log`; note any C-API compatibility changes.
8. Proceed to **[Releasing](#releasing)**.

> **Reproducibility trade-off:** a branch ref is a moving target — a re-run can
> build against a newer DuckDB commit than before. Release binaries are pinned by
> the git tag's submodule commits at tag time; the registry builds its own DuckDB.

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
