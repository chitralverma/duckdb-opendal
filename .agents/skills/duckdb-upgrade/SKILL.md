---
name: duckdb-upgrade
description: Move duckdb-opendal to a new DuckDB MINOR release line (e.g. v1.5 -> v1.6) — flip the duckdb + extension-ci-tools submodules and the CI workflow from one release-line branch to the next, refresh docs, then build and test. Patch releases (v1.5.x) need NO action (auto-tracked). Use when the user asks to upgrade DuckDB to a new minor, move to a new release line, or a new DuckDB minor broke the build.
---

# Move duckdb-opendal to a new DuckDB minor line

The pipeline tracks the DuckDB **release-line branch** (e.g. `v1.5-variegata`) via
`duckdb_version`, not a pinned patch tag. Consequences:

- **Patch** (`v1.5.5` → `v1.5.6`): **do nothing.** The moving branch ref picks it
  up; the daily `schedule` run in `MainDistributionPipeline.yml` rebuilds + tests
  against it. This skill is **not** needed.
- **Minor** (`v1.5.x` → `v1.6.x`): flip everything to the new line branch — the
  steps below.

DuckDB and `extension-ci-tools` move **together**. Do not touch the OpenDAL `rev`
or the extension `version` here unless a compile failure forces an OpenDAL
compatibility fix (that is axis B / the release, see `MAINTAINING.md`).

## Inputs

Confirm the target minor and its DuckDB **release-line branch** name
(`vX.Y-<codename>`):

```sh
git ls-remote --heads https://github.com/duckdb/duckdb "vX.Y*"
git ls-remote --heads https://github.com/duckdb/extension-ci-tools "vX.Y*"
```

## Workflow (do not skip ahead)

1. **`.gitmodules`**: set the `branch` hint for **both** `duckdb` and
   `extension-ci-tools` to the new line branch (`vX.Y-<codename>`).
2. **Point both submodules at the new line-branch head**:
   ```sh
   for sub in duckdb extension-ci-tools; do
     git -C "$sub" fetch --depth 1 origin <line-branch>
     git -C "$sub" checkout --detach FETCH_HEAD
   done
   git -C duckdb submodule update --init --recursive
   git add duckdb extension-ci-tools
   git submodule status duckdb extension-ci-tools
   ```
3. **Update `.github/workflows/MainDistributionPipeline.yml`**:
   - `duckdb_version` and `ci_tools_version` in `duckdb-stable-build` and
     `code-quality-check` → the new line branch.
   - The `@vX.Y-*` ref on the reusable `_extension_distribution.yml` /
     `_extension_code_quality.yml` workflows → the new line.
4. **Update docs**:
   - The submodule-pin note in `CONTRIBUTING.md`.
   - The DuckDB column of the compatibility matrix in `README.md`.
   - A `### Changed` entry under `[Unreleased]` in `CHANGELOG.md`.
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

Complete only when:

- `.gitmodules` + both submodules + `MainDistributionPipeline.yml`
  (`duckdb_version`, `ci_tools_version`, reusable-workflow refs) all name the **new**
  line branch.
- The static and loadable `opendal` targets link.
- `make rust-test`, `make test-local`, and the S3 tier pass.
- `README.md` matrix, `CONTRIBUTING.md`, and `CHANGELOG.md` reflect the new line.

## Then

Cut a release with the `extension-release` skill so the tag, the
community-extensions `description.yml:ref`, and the deployed community binary all
advance together.

## References

- DuckDB [Release Notes](https://github.com/duckdb/duckdb/releases)
- DuckDB [core extension patches](https://github.com/duckdb/duckdb/commits/main/.github/patches/extensions)
  (useful when the C-API changed and the build breaks)
