# Schemes & coexistence with native DuckDB filesystems

`opendal_fs` registers a DuckDB filesystem that serves several URL schemes
(currently `fs://`, `memory://`, `s3://`; more OpenDAL services are added over
time). Some of these — notably `s3://` (and `gcs://`, `azure://`, `r2://`, …) —
are **also** claimed by DuckDB's built-in extensions (`httpfs`, `azure`).

This page explains how the two coexist and how to control which one wins.

## How DuckDB picks a filesystem

For a given URL, DuckDB's virtual filesystem asks each registered subsystem
`CanHandleFile(path)`. Among those that can:

- a subsystem that reports **`IsManuallySet() == true` wins immediately**;
- otherwise the **last-registered** matching subsystem wins.

DuckDB also **auto-loads** `httpfs` (or `azure`) the first time you touch a
reserved scheme like `s3://`. So if both `opendal_fs` and `httpfs` are loaded,
`s3://` may be served by either, depending on load order — which is not something
you want to rely on.

## Making `opendal_fs` win selected schemes

Use the setting `opendal_override_native_filesystems` — a **comma-separated list
of schemes** for which `opendal_fs` should take precedence over native
extensions:

```sql
-- opendal_fs now wins s3:// and gcs:// over httpfs, for this session:
SET opendal_override_native_filesystems = 's3,gcs';

-- Reads under those schemes go through opendal_fs (and its secrets / layers):
SELECT * FROM read_parquet('s3://bucket/data.parquet');

-- Turn it back off (native handlers regain precedence):
SET opendal_override_native_filesystems = '';
```

- Default is empty → **no override**; native handlers keep their normal
  precedence.
- The override is **per scheme**: only the schemes you list are taken over.
  `opendal_fs` still declines schemes it doesn't support, so those always fall
  through to the native handler.

### Coexistence example

Because the override is per scheme, you can mix `opendal_fs` and native handlers
in the **same session**:

```sql
LOAD opendal_fs;
LOAD httpfs;
SET opendal_override_native_filesystems = 's3';

-- s3://  -> served by opendal_fs (uses your CREATE SECRET ... TYPE s3)
SELECT * FROM read_parquet('s3://bucket/a.parquet');

-- s3n:// -> not overridden (and not claimed by opendal_fs) -> served by httpfs
SELECT * FROM read_parquet('s3n://bucket/b.parquet');
```

## Secrets when both extensions are loaded

Both `opendal_fs` and `httpfs` register a `TYPE s3` secret. `opendal_fs`
tolerates the secret type already existing (it will not fail to load if `httpfs`
registered it first). When the override routes `s3://` to `opendal_fs`, your
`CREATE SECRET (TYPE s3, ...)` is consumed by `opendal_fs`'s SCOPE-matched
resolution (see the main README / secrets docs).

## Notes & limitations

- The override is a session/global setting; set it before running queries that
  should be affected.
- Full, automatic multi-service coexistence (e.g. exposing every OpenDAL service
  under a dedicated, non-conflicting umbrella scheme like `opendal://<service>/…`
  so no toggle is needed) is a future direction. For now the override list is the
  supported mechanism.
- Alternatively, DuckDB's `SET disabled_filesystems = '<Name>'` can disable a
  native filesystem entirely by name — a blunter instrument than the per-scheme
  override.
