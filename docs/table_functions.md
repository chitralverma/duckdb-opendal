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
