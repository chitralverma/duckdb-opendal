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
    "limit" := 1000,
    start_after := 'prefix/key',
    versions := false,
    deleted := false
);
```

`limit` is a DuckDB SQL keyword, so quote it when used as a named argument.
OpenDAL treats it as a backend page-size hint, not a total row limit.

## `opendal_glob`

```sql
SELECT *
FROM opendal_glob('s3://bucket/data/**/part-[0-9].parquet');
```

Globs match full operator-relative paths. `*` and `?` do not cross `/`; `**`
does. Character classes and ranges are supported. List options accepted by
`opendal_ls` are also accepted by `opendal_glob`; globbing always lists
recursively from the static prefix before the first wildcard.

## `opendal_du`

`opendal_du` retains per-parent-directory rollups and accepts a file, directory,
or glob target. Files and glob matches are grouped by parent. It accepts
`"limit"`, `start_after`, `versions`, and `deleted` list options.

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

When both URLs resolve to the same effective operator and native copy is
supported, the function drains OpenDAL's server-side `Copier`. Otherwise it
streams bounded chunks from an OpenDAL `Reader` into an OpenDAL `Writer`, using
each URL's independently scoped secret. Transfer failures abort the destination
writer. One row is returned only after destination commit:

`source`, `destination`, `bytes_copied`, `metadata`.

`source_content_length_hint` is validated against a version-aware source stat.
A mismatch fails instead of risking a truncated segmented copy.

Copy is a side effect, not a DuckDB transaction. Query rollback cannot undo a
completed remote copy. Mutation starts only when the table function is scanned;
an optimized-away function or `LIMIT 0` does not copy.
