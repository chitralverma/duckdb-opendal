# Changelog

Release notes are **generated, not hand-written**: tagging `vX.Y.Z` runs the
`create-release-draft` job, which builds the binaries and drafts a GitHub
release whose notes come from the merged pull requests. Read them on the
[releases page](https://github.com/chitralverma/duckdb-opendal/releases).

- **Unreleased changes:** [`v0.1.0...main`](https://github.com/chitralverma/duckdb-opendal/compare/v0.1.0...main).
- **Which services and versions a build has:** `FROM opendal_schemes();` and
  `SELECT opendal_version();`.
- **Compatibility per release:** the matrix in the [README](README.md#compatibility).

Write pull-request titles as the changelog entry you want to read later — they
are what ends up in the notes. Everything below `0.1.0` is kept as the
hand-written history from before this was automated.

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

[0.1.0]: https://github.com/chitralverma/duckdb-opendal/releases/tag/v0.1.0
