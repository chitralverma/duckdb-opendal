# Coexistence with DuckDB's own filesystems

Some URL schemes are claimed by more than one extension: `s3://` by both
`opendal` and DuckDB's built-in `httpfs`, for example. This page shows how to
decide which one serves a query.

Only overlapping schemes are affected. `s3://` and `gcs://` are contested;
`fs://`, `memory://`, `azblob://`, `azdls://` and `azfile://` are not claimed by
any built-in extension, so they always go through `opendal`. DuckDB's `azure` extension uses `azure://` /
`abfss://`, which `opendal` does not serve.

## The symptom

If both extensions are loaded, `s3://` may be handled by either — DuckDB
generally gives it to whichever registered last, and it auto-loads `httpfs` the
moment you touch a reserved scheme. So the same query can behave differently
depending on load order: your `CREATE SECRET (TYPE s3, …)` options, retry
settings and caching are only applied if `opendal` is the one serving the URL.

Check who would serve it:

```sql
SELECT scheme, supported, secret_name, usable FROM opendal_info('s3://warehouse/a.parquet');
```

## Choosing `opendal` for a scheme

`opendal_override_native_filesystems` is a comma-separated list of schemes where
`opendal` takes precedence:

```sql
SET opendal_override_native_filesystems = 's3';

-- Now served by opendal, using its secrets, retries and caching:
SELECT * FROM read_parquet('s3://warehouse/data.parquet');

-- Hand it back to httpfs:
SET opendal_override_native_filesystems = '';
```

- Empty by default — nothing is overridden.
- Per scheme: list several with `'s3,gcs'`. Schemes `opendal` does not support
  are never taken over, whatever you list.
- It is a session/global setting; set it before the queries it should affect.

You can mix handlers in one session:

```sql
LOAD opendal;
LOAD httpfs;
SET opendal_override_native_filesystems = 's3';

SELECT * FROM read_parquet('s3://warehouse/a.parquet');    -- opendal
SELECT * FROM read_parquet('s3n://warehouse/b.parquet');   -- httpfs
```

The blunter alternative is DuckDB's `SET disabled_filesystems = '<name>';`, which
switches a filesystem off entirely by name (`SELECT * FROM duckdb_settings()` and
DuckDB's docs list the names).

## Load order

**Load `httpfs` before `opendal`.** Both register a `TYPE s3` secret; `opendal`
tolerates the type already existing, but `httpfs` loaded *after* `opendal` can
fail on it — and because extension loading is not transactional, retrying that
load may then complain that `HTTPFileSystem` is already registered.

## Secrets

There is one `TYPE s3` secret type, shared. Whichever extension serves the URL
reads it, so keep the options valid for the one you intend to win. When
`opendal` serves it, scope matching works as described in
[configuration](configuration.md).

A secret type your build does *not* include can quietly belong to another
extension: `TYPE gcs` maps to `httpfs`, and with
`autoload_known_extensions = true` DuckDB will try to load `httpfs` rather than
report an unknown type. To see only this extension's types:

```sql
SET autoload_known_extensions = false;
CREATE SECRET gcs_not_compiled (TYPE gcs, config MAP{});   -- fails if not compiled in
```
