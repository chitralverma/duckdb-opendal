---
name: extension-release
description: Cut a duckdb-opendal release — bump the extension version, update the CHANGELOG and compatibility matrix, tag it (triggers the create-release-draft binaries), then bump description.yml:ref and sync the DuckDB community-extensions registry entry. Use when the user asks to release, cut a version, tag a release, publish the extension, or update the community registry ref.
---

# Release duckdb-opendal

A release freezes all three version axes (DuckDB, OpenDAL, extension) into one
commit and publishes it to the DuckDB community registry. See `MAINTAINING.md`
for the axis model.

**Invariant to uphold:** the git tag, `description.yml:ref`, and the deployed
community binary must all point to the **same commit**. (Historical drift:
`v0.1.0` tag was behind the deployed ref — do not repeat this.)

## Inputs

Confirm the new semver version (`0.x` may include breaking changes) and that any
DuckDB / OpenDAL upgrades for this release are already merged.

## Workflow (do not skip ahead)

1. **Pre-flight**: `main` is green; working tree clean; decide the version.
2. **Bump the version** in both, kept equal:
   - `opendal/Cargo.toml` → `version`
   - `description.yml` → `extension.version`
   Run a build so `opendal/Cargo.lock` picks up the new version.
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
7. **Bump `description.yml:ref`** to `<release-sha>` in this repo (commit + push),
   so the registry builds exactly the tagged commit.
8. **Sync the community-extensions registry**:
   - Update `extensions/opendal/description.yml` in the `community-extensions`
     checkout so it is **byte-identical** to this repo's `description.yml`
     (same `version` and `ref`).
   - If the extension is not yet listed, open the add-extension PR; otherwise open
     a follow-up PR that bumps `version` + `ref`.
   - Push to update the PR. Community CI needs maintainer approval for fork PRs.
9. **Verify deployment** once merged (CDN is per DuckDB version):
   ```sh
   curl -s -o /dev/null -w "%{http_code}\n" \
     http://community-extensions.duckdb.org/<duckdb_version>/linux_amd64/opendal.duckdb_extension.gz
   ```

## Verification

The release is complete only when:

- `vX.Y.Z` tag, this repo's `description.yml:ref`, and the community entry's `ref`
  are the **same commit**.
- `opendal/Cargo.toml` and `description.yml` versions match `X.Y.Z`.
- `CHANGELOG.md` has the dated `X.Y.Z` section and a fresh `[Unreleased]`.
- The GitHub release is published with per-platform assets.
- The community-extensions PR is updated (and, once merged + built, the CDN
  serves the new binary for the target DuckDB version).

## Notes

- The community registry builds from source at `ref`; it ignores the GitHub
  release assets. The GitHub release exists for downloadable binaries
  (older-DuckDB / unsigned / air-gapped installs) and version anchoring.
- A description-only change (`version`/`ref`) is what re-triggers the community
  build+deploy; keep the two `description.yml` files identical.
