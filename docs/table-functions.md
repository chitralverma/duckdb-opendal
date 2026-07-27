# Table functions

Inspect and manage storage from SQL. Every function takes URLs in the usual
`scheme://authority/path` form and uses the same secrets as reads and writes.

| Function                              | Use it to                                          |
| ------------------------------------- | -------------------------------------------------- |
| [`opendal_ls`](#opendal_ls)           | List a directory, or match a glob                  |
| [`opendal_stat`](#opendal_stat)       | Get metadata for one object                        |
| [`opendal_du`](#opendal_du)           | Total up sizes per directory                       |
| [`opendal_copy`](#opendal_copy)       | Copy files within or across services               |
| [`opendal_info`](#opendal_info)       | Check whether a URL is usable                      |
| [`opendal_schemes`](#opendal_schemes) | See which services this build supports             |
| `opendal_version()`                   | Extension and OpenDAL versions (a scalar function) |

## `opendal_ls`

```sql
-- A directory
SELECT path, metadata.content_length FROM opendal_ls('s3://warehouse/data/');

-- Recursively
SELECT * FROM opendal_ls('s3://warehouse/data/', recursive := true);

-- A glob
SELECT * FROM opendal_ls('s3://warehouse/data/**/part-[0-9].parquet');
```

A path containing `*`, `?` or `[` switches to glob matching. `*` and `?` stop at
`/`; `**` crosses it. Globs always search recursively from the fixed prefix
before the first wildcard.

Options: `recursive`, `start_after`, `versions`, `deleted`.

Entries stream in whatever order the service returns them — add `ORDER BY` if
you need a stable order.

## `opendal_stat`

```sql
SELECT * FROM opendal_stat('s3://warehouse/data/file.parquet');
```

Optional arguments, all passed to the service: `version`, `if_match`,
`if_none_match`, `if_modified_since`, `if_unmodified_since`.

```sql
SELECT * FROM opendal_stat(
    's3://warehouse/data/file.parquet',
    if_modified_since := TIMESTAMP '2026-01-01'
);
```

## `opendal_du`

```sql
SELECT directory, file_count, total_size FROM opendal_du('s3://warehouse/data/');
```

Rolls sizes up per parent directory. Accepts a file, a directory or a glob;
files and glob matches are grouped by their parent. Also takes `start_after`,
`versions` and `deleted`.

## `opendal_copy`

```sql
-- One file, across services
SELECT * FROM opendal_copy(
    's3://warehouse/input.parquet',
    'azblob://backup/output.parquet'
);

-- A whole tree (destination is a prefix)
SELECT * FROM opendal_copy('s3://warehouse/data/', 'azblob://backup/data/');

-- Everything matching a glob
SELECT * FROM opendal_copy('s3://warehouse/data/**/*.parquet', 'azblob://backup/');
```

Returns one row per copied object — `source`, `destination`, `bytes_copied`,
`metadata` — as each copy commits. For a directory or glob source, each file
keeps its path relative to the fixed part of the source.

Options: `if_not_exists`, `concurrent`, `chunk_size`, and (single file only)
`source_version` and `source_content_length_hint`.

Good to know:

- Both URLs use their own matching secret, so cross-service and cross-account
  copies work.
- Within one service the server-side copy API is used when available; otherwise
  data is streamed through in bounded chunks.
- A copy is a side effect, not part of the transaction: rolling back the query
  does not undo it. Nothing is copied until the function is actually scanned, so
  `LIMIT 0` copies nothing.

## `opendal_info`

```sql
SELECT * FROM opendal_info('s3://warehouse/data/');
```

The first thing to run when a URL misbehaves:

| Column         | Meaning                                                          |
| -------------- | ---------------------------------------------------------------- |
| `scheme`       | The URL's scheme                                                 |
| `supported`    | Whether this build includes that service                         |
| `secret_name`  | Which secret matched, or `NULL`                                  |
| `usable`       | Whether the URL can actually be served                           |
| `root`         | The resolved root, when usable                                   |
| `capabilities` | What the service supports (read, write, copy, versioning, …)     |
| `message`      | Why it is not usable — including a hint about the missing secret |

## `opendal_schemes`

```sql
FROM opendal_schemes();
```

One row per service compiled into your build, including aliases such as `file`
for `fs`. Needs no secret.

## Entry metadata

`opendal_ls`, `opendal_du`, `opendal_stat` and `opendal_copy` all return a
nested `metadata` struct:

- `mode` — `file`, `directory` or `unknown`
- `content_length` — size in bytes
- `last_modified` — timestamp, when the service reports one
- `etag`, `version`, `content_type`, `content_md5`, `content_encoding`,
  `content_disposition`, `cache_control` — nullable strings
- `is_current`, `is_deleted` — for version-aware listings
- `user_metadata` — `MAP(VARCHAR, VARCHAR)`

Listings return whatever metadata the service includes in its response; fields
it omits are `NULL`. Use `opendal_stat` when you need a complete picture of one
object.
