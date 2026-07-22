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
