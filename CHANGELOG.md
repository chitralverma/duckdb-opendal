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

- `sftp://` service, compiled in by default. Tested against an atmoz/sftp
  emulator (`make sftp-up` / `sftp-down`); OpenDAL authenticates with a key only,
  so the target generates a throwaway keypair.
- `gcs://` (Google Cloud Storage) service, compiled in by default. Tested against
  a fake-gcs-server emulator (`make gcs-up` / `gcs-down`).
- `opendal_schemes()` table function listing the storage schemes compiled into
  the build.
- `opendal_info(url)` table function describing a URL's scheme, support, matched
  secret, usability, operator root, and capabilities; operator-build failures
  are enriched with a `CREATE SECRET` hint.

### Changed

- CI/build tracks the DuckDB `v1.5-variegata` release line (any `v1.5.x` patch)
  instead of a pinned tag; a daily scheduled run rebuilds against the latest
  patch. DuckDB patch bumps now need no repo change.
- `opendal_ls` handles glob patterns directly (wildcards in the path); the
  separate `opendal_glob` function is removed.
- `opendal_copy` accepts file, directory, and glob sources, emitting one row per
  copied object.
- `cargo update`: serde_json 1.0.151, tokio 1.53.1, rustls 0.23.42,
  futures 0.3.33.
- Pinned OpenDAL revision bumped `3180510` → `7ce945d` (0.58.1), which fixes the
  gcs writer double-encoding object keys (keys containing `/` were stored under a
  literal `%2F` name and could not be read back).

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
