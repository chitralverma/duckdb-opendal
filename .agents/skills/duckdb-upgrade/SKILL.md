---
name: duckdb-upgrade
description: Upgrade duckdb-opendal to a new DuckDB release — bump the duckdb and extension-ci-tools submodules together, update the CI workflow (duckdb_version / ci_tools_version), refresh docs and the compatibility matrix, then build and test. Use when the user asks to upgrade DuckDB, bump the duckdb submodule, sync to a new DuckDB tag, or target a new DuckDB version.
---

# Upgrade duckdb-opendal to a new DuckDB release

DuckDB and `extension-ci-tools` move **together**; OpenDAL and the extension
version are separate axes (see `MAINTAINING.md`). This skill only advances the
DuckDB axis. Do not bump the OpenDAL `rev` or the extension `version` here unless
a compilation failure forces an OpenDAL compatibility fix.

## Inputs

Confirm the target DuckDB tag (e.g. `v1.5.6`) before making changes. Determine
whether it is a **patch** within the current minor line or a **minor/major** bump
— this decides the `extension-ci-tools` branch.

## Workflow (do not skip ahead)

1. **Pin the `duckdb` submodule** to the target tag:
   ```sh
   git -C duckdb fetch --tags --depth 1 origin refs/tags/$TARGET:refs/tags/$TARGET
   git -C duckdb checkout --detach tags/$TARGET
   git -C duckdb submodule update --init --recursive
   git add duckdb
   git submodule status duckdb   # confirm the new commit + (tag)
   ```
2. **Pin `extension-ci-tools`** to the branch/tag matching the target DuckDB:
   - Patch within the same minor → keep the current release branch
     (e.g. `v1.5-variegata` for the 1.5 line); usually no change.
   - Minor/major bump → move to the new release branch (e.g. `v1.6-*`) and update
     the `branch` hint for `extension-ci-tools` in `.gitmodules` to match.
3. **Update `.github/workflows/MainDistributionPipeline.yml`**:
   - `duckdb_version` in `duckdb-stable-build` and `code-quality-check`.
   - `ci_tools_version` in both jobs (only changes on a minor/major bump).
   - The `@vX.Y-*` ref on the reusable `_extension_*` workflows if the ci-tools
     branch changed.
4. **Update docs**:
   - The submodule-pin note in `CONTRIBUTING.md`.
   - The DuckDB column of the compatibility matrix in `README.md`.
   - Add a `### Changed` entry under `[Unreleased]` in `CHANGELOG.md`.
5. **Format**: `uv run make format-all`.
6. **Build**: `GEN=ninja make` (static + loadable targets must link).
7. **Test**:
   ```sh
   make rust-test
   make test-local
   make s3-up && make test-common-s3 && make s3-assert-no-incomplete && make s3-down
   ```
8. **Review** `git diff --submodule=log`; record any DuckDB C-API compatibility
   changes in the commit / PR body.

## Verification

The upgrade is complete only when:

- `git submodule status duckdb` shows the target tag, and `extension-ci-tools`
  matches the DuckDB release line.
- `MainDistributionPipeline.yml` names the same DuckDB release in every job.
- The static and loadable `opendal` targets link.
- `make rust-test`, `make test-local`, and the S3 tier pass.
- `README.md` matrix, `CONTRIBUTING.md`, and `CHANGELOG.md` reflect the new
  DuckDB version.

## Then

Cut a release with the `extension-release` skill so the tag, `description.yml:ref`,
and the deployed community binary all advance together.

## References

- DuckDB [Release Notes](https://github.com/duckdb/duckdb/releases)
- DuckDB [core extension patches](https://github.com/duckdb/duckdb/commits/main/.github/patches/extensions)
  (useful when the C-API changed and the build breaks)
