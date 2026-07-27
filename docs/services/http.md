# `http://`, `https://` — HTTP servers

Read files straight off a web server, object-store URL, or any static host. One
service serves both schemes.

**Read-only.** It can `read` and `stat`; there is no write, list, glob, copy or
delete. Everything else in this extension still applies — secrets, retries,
timeouts, caching.

## URL

```text
https://example.com/data/events.parquet
http://127.0.0.1:8080/data/events.parquet
```

The authority is the **host** (with port if any); it becomes the endpoint, and
the rest of the URL is the path. Nothing else is needed, so a public file works
with no secret at all:

```sql
SELECT * FROM read_parquet('https://example.com/data/events.parquet');
```

## Secret

Only needed for authentication:

```sql
CREATE SECRET web (
    TYPE https,
    SCOPE 'https://example.com/private',
    config MAP{'username': 'reader', 'password': '...'}
);

SELECT * FROM read_parquet('https://example.com/private/events.parquet');
```

| Key        | Meaning                                      |
| ---------- | -------------------------------------------- |
| `username` | Basic-auth user.                             |
| `password` | Basic-auth password.                         |
| `token`    | Bearer token; sent instead of Basic auth.    |
| `endpoint` | Overrides the endpoint derived from the URL. |

Scope the secret to the prefix that needs it — URLs outside it stay anonymous.
`TYPE http` and `TYPE https` are separate secret types; use the one matching the
scheme in your URLs.

## Partitioned datasets

Globbing a dataset root needs `list`, so it fails here:

```sql
SELECT * FROM read_parquet('https://example.com/data/**/*.parquet');
-- IO Error: ... service 'http' does not support list
```

List the files explicitly instead. Hive partition columns still come from the
paths, and partition filters still prune:

```sql
SELECT year, count(*) FROM read_parquet([
    'https://example.com/data/year=2025/part-0.parquet',
    'https://example.com/data/year=2026/part-0.parquet'
], hive_partitioning = true)
GROUP BY year;
```

With `WHERE year = 2026`, the 2025 file is never fetched — DuckDB opens only the
first file (to bind the schema) and the partitions that match.

If the data lives in an object store, prefer that store's own scheme — globbing
and discovery work normally there. `http`/`https` is for hosts that only speak
plain HTTP.

## Notes

- **Query strings are rejected** (`?token=…`), so pre-signed URLs do not work
  here. Use the service's own scheme (`s3://`, `azblob://`, …) with a secret
  instead — see [configuration](../configuration.md) for the URL rules.
- **`http://` and `https://` are DuckDB's most contested schemes** — `httpfs`
  claims them and DuckDB auto-loads it. To route them through this extension:
  `SET opendal_override_native_filesystems = 'http,https';` — see
  [coexistence](../coexistence.md). Without it you are almost certainly using
  `httpfs`, and none of the settings here apply.
- Range requests drive projection and filter pushdown, so reading a few columns
  from a large remote Parquet file does not download all of it. Servers that do
  not honour `Range` fall back to full reads.
- Caching helps most here; see [caching](../caching.md).
