---
name: extension-release
description: Cut a duckdb-opendal release — bump the extension version, update the CHANGELOG and compatibility matrix, tag it (triggers the create-release-draft binaries), then bump description.yml:ref and sync the DuckDB community-extensions registry entry. Use when the user asks to release, cut a version, tag a release, publish the extension, or update the community registry ref.
---

# Release duckdb-opendal

A release freezes all three version axes (DuckDB, OpenDAL, extension) into one
commit and publishes it to the DuckDB community registry. See `MAINTAINING.md`
for the axis model.

**Invariant to uphold:** the git tag, the community-extensions
`description.yml:ref`, and the deployed community binary must all point to the
**same commit**. Cut each release from a single commit so they cannot drift.

The extension's registry `description.yml` lives only in the `community-extensions`
repo (`extensions/opendal/description.yml`) — there is **no copy in this repo**.
`opendal/Cargo.toml` is the source of truth for the version.

## Inputs

Confirm the new semver version (`0.x` may include breaking changes) and that any
DuckDB / OpenDAL upgrades for this release are already merged.

## Workflow (do not skip ahead)

1. **Pre-flight**: `main` is green; working tree clean; decide the version.
2. **Bump the version** in `opendal/Cargo.toml` (`version`) — the source of truth.
   Run a build so `opendal/Cargo.lock` picks up the new version. Do **not** bump
   `pyproject.toml` (that is the `duckdb-opendal-tooling` venv, not the extension).
3. **Update `CHANGELOG.md`**:
   - Rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`.
   - Add a fresh empty `## [Unreleased]`.
   - Update the compare/tag links at the bottom.
4. **Update the compatibility matrix** in `README.md` with the new row
   (extension, DuckDB, OpenDAL `rev`, services).
5. **Commit** (e.g. `release: vX.Y.Z`) and **push `main`**. Record the commit SHA
   — this is the release commit that everything must point to.
6. **Tag and push**:
   ```sh
   git tag -a vX.Y.Z <release-sha> -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```
   This triggers `create-release-draft`, which builds the per-platform binaries
   and drafts a GitHub release with generated notes. Fold those notes into the
   CHANGELOG section if they add detail, then **publish the draft release**.
7. **Update the community-extensions `description.yml`**
   (`extensions/opendal/description.yml` in the `community-extensions` checkout):
   set `version` to `X.Y.Z` and `ref` to `<release-sha>`, so the registry builds
   exactly the tagged commit.
8. **Open/refresh the community-extensions PR**:
   - If the extension is not yet listed, open the add-extension PR; otherwise open
     a follow-up PR bumping `version` + `ref`.
   - Push to update the PR. Community CI needs maintainer approval for fork PRs.
9. **Verify deployment** once merged (CDN is per DuckDB version):
   ```sh
   curl -s -o /dev/null -w "%{http_code}\n" \
     http://community-extensions.duckdb.org/<duckdb_version>/linux_amd64/opendal.duckdb_extension.gz
   ```

## Verification

The release is complete only when:

- `vX.Y.Z` tag and the community-extensions `description.yml:ref` are the **same
  commit**.
- `opendal/Cargo.toml` version and the community-extensions `description.yml`
  version match `X.Y.Z`.
- `CHANGELOG.md` has the dated `X.Y.Z` section and a fresh `[Unreleased]`.
- The GitHub release is published with per-platform assets.
- The community-extensions PR is updated (and, once merged + built, the CDN
  serves the new binary for the target DuckDB version).

## Notes

- The community registry builds from source at the community-extensions
  `description.yml:ref`; it ignores the GitHub release assets. The GitHub release
  exists for downloadable binaries (older-DuckDB / unsigned / air-gapped installs)
  and version anchoring.
- A `description.yml`-only change (`version` / `ref`) in `community-extensions` is
  what re-triggers the community build + deploy.
