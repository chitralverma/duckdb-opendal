# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this
project adheres to [Semantic Versioning](https://semver.org/). While the
extension is in `0.x` (alpha), minor versions may include breaking changes.

Each release is built against a specific DuckDB version and pins a specific
Apache OpenDAL revision; see the compatibility matrix in the
[README](README.md#compatibility).

<!-- Draft release notes are auto-generated on tag by the create-release-draft
     job (GitHub "Generate release notes"); curate them into the sections below. -->

## [Unreleased]

### Added

- Azure services: `azblob://` (Azure Blob Storage), `azdls://` (Data Lake
  Storage Gen2) and `azfile://` (Azure Files), all in the default build
  (`services-azblob`, `services-azdls`, `services-azfile`).
- `azblob` test tier backed by the Azurite emulator (`make azblob-up` /
  `azblob-down`); `azdls`/`azfile` are credential-gated tiers that skip unless
  their `OPENDAL_AZ{DLS,FILE}_*` env vars are set.
- `opendal_schemes()` table function listing the storage schemes compiled into
  the build.
- `opendal_info(url)` table function describing a URL's scheme, support, matched
  secret, usability, operator root, and capabilities; operator-build failures
  are enriched with a `CREATE SECRET` hint.

### Changed

- Docs: per-service URL shapes and secret examples moved out of
  `docs/configuration.md` into a new `docs/services.md`; `docs/schemes.md` is
  renamed `docs/coexistence.md` and is now purely about coexisting with native
  DuckDB filesystems; the README links the doc set.
- CI/build tracks the DuckDB `v1.5-variegata` release line (any `v1.5.x` patch)
  instead of a pinned tag; a daily scheduled run rebuilds against the latest
  patch. DuckDB patch bumps now need no repo change.
- `opendal_ls` handles glob patterns directly (wildcards in the path); the
  separate `opendal_glob` function is removed.
- `opendal_copy` accepts file, directory, and glob sources, emitting one row per
  copied object.
- `cargo update`: serde_json 1.0.151, tokio 1.53.1, rustls 0.23.42,
  futures 0.3.33.

### Removed

- `opendal_glob` table function (folded into `opendal_ls`).
- The `limit` option on `opendal_ls` / `opendal_du` (it bounded entries scanned,
  not rows returned).

## [0.1.0] - 2026-07-22

Initial alpha release, built for DuckDB v1.5.5.

### Added

- Virtual filesystem for `fs://` / `file://`, `s3://`, and `memory://`.
- Read & write Parquet, CSV, and JSON through any supported service, including
  glob patterns (e.g. `read_parquet('s3://bucket/**/*.parquet')`).
- Table functions: `opendal_version`, `opendal_stat`, `opendal_ls`,
  `opendal_glob`, `opendal_du`, and `opendal_copy`.
- Scoped secrets with per-service `config`, `io_config`, `retry_config`, and
  `cache_config` via DuckDB's secret manager.
- Configurable layers: retry, timeout, concurrent-limit, and Foyer-backed
  in-memory / on-disk data caching.
- `opendal_override_native_filesystems` setting for coexistence with the native
  `httpfs` extension.
- DuckDB FileSystem logging integration for structured I/O tracing.

[Unreleased]: https://github.com/chitralverma/duckdb-opendal/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/chitralverma/duckdb-opendal/releases/tag/v0.1.0
