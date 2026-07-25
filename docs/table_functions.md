# OpenDAL table functions

Table-function rows mirror OpenDAL's `Entry`: `path`, `name`, and a nested
`metadata` struct. Listing and globbing stream entries in backend order. Add an
explicit `ORDER BY` when deterministic ordering is required.

## Entry metadata

`metadata` contains OpenDAL's public metadata fields:

- `mode`: `file`, `directory`, or `unknown`
- `content_length`: bytes (`UBIGINT`)
- `cache_control`, `content_disposition`, `content_md5`, `content_type`,
  `content_encoding`, `etag`, `version`: nullable strings
- `last_modified`: nullable timestamp
- `is_current`: nullable boolean
- `is_deleted`: boolean
- `user_metadata`: `MAP(VARCHAR, VARCHAR)`

Listing backends may return partial metadata. The extension does not issue an
implicit `stat` for every listed entry.

## `opendal_stat`

```sql
SELECT * FROM opendal_stat(
    's3://bucket/file.parquet',
    version := 'version-id',
    if_match := 'etag',
    if_none_match := 'etag',
    if_modified_since := TIMESTAMP '2026-01-01',
    if_unmodified_since := TIMESTAMP '2026-12-31'
);
```

All named arguments are optional and map to OpenDAL `StatOptions`.

## `opendal_ls`

```sql
SELECT * FROM opendal_ls(
    's3://bucket/prefix/',
    recursive := true,
    start_after := 'prefix/key',
    versions := false,
    deleted := false
);
```

A path containing a glob wildcard (`*`, `?`, or `[`) switches `opendal_ls` to
glob mode instead of a plain directory listing:

```sql
SELECT * FROM opendal_ls('s3://bucket/data/**/part-[0-9].parquet');
```

Globs match full operator-relative paths. `*` and `?` do not cross `/`; `**`
does. Character classes and ranges are supported. Glob mode always lists
recursively from the static prefix before the first wildcard, and accepts the
same list options as a plain listing. (DuckDB's built-in `glob()` also routes
through the filesystem; `opendal_ls` adds the entry-metadata columns.)

## `opendal_du`

`opendal_du` reports per-parent-directory rollups and accepts a file, directory,
or glob target. Files and glob matches are grouped by parent. It accepts
`start_after`, `versions`, and `deleted` list options.

## `opendal_copy`

```sql
SELECT * FROM opendal_copy(
    's3://source-bucket/input.parquet',
    'azblob://destination/output.parquet',
    if_not_exists := true,
    source_version := 'version-id',
    concurrent := 4,
    chunk_size := 8388608
);
```

The source may be a single file, a directory, or a glob pattern. For a directory
or glob the destination is treated as a prefix and each matched file is copied
preserving its path relative to the static base (the source directory, or the
prefix before the first wildcard). One row is returned per copied object after
its destination commit:

`source`, `destination`, `bytes_copied`, `metadata`.

When both URLs resolve to the same effective operator and native copy is
supported, each file drains OpenDAL's server-side `Copier`; otherwise it streams
bounded chunks from an OpenDAL `Reader` into an OpenDAL `Writer`, using each
URL's independently scoped secret. Transfer failures abort the destination
writer.

`source_content_length_hint` and `source_version` apply to a single-file copy
only; both are rejected for directory/glob sources. The hint is validated against
a version-aware source stat — a mismatch fails instead of risking a truncated
segmented copy.

Copy is a side effect, not a DuckDB transaction. Query rollback cannot undo a
completed remote copy. Mutation starts only when the table function is scanned;
an optimized-away function or `LIMIT 0` does not copy.

## `opendal_schemes`

```sql
SELECT scheme FROM opendal_schemes() ORDER BY scheme;
```

One row per OpenDAL service scheme compiled into this build (derived from the
`services-*` Cargo features), including scheme aliases (e.g. `file` for `fs`).
Takes no arguments and needs no secret.

## `opendal_info`

```sql
SELECT * FROM opendal_info('s3://bucket/prefix/');
```

Describes whether a URL can be served by the extension. Columns:

- `scheme`: the URL scheme.
- `supported`: whether the scheme is compiled into this build.
- `secret_name`: the name of the SCOPE-matched secret, or `NULL` if none is
  configured.
- `usable`: whether an operator could be built (scheme supported *and* required
  configuration present).
- `root`: the operator's root, when usable.
- `capabilities`: `MAP(VARCHAR, BOOLEAN)` of the operator's capabilities, when
  usable.
- `message`: when not usable, the reason — including a hint to configure a secret
  when a supported scheme is missing required configuration (e.g. an `fs` root or
  S3 credentials).

Built operators are cached and deduplicated by effective configuration, so
`opendal_info` reuses the same operator a subsequent read/write would use.
