# `fs://` — local filesystem

Reads and writes files on the machine running DuckDB. Also registered as
`file://`.

## URL

```text
fs:///path/relative/to/root
```

The authority is empty, so paths use **three** slashes. The path is always
relative to the `root` you configure — it is never an absolute host path.

## Secret

```sql
CREATE SECRET local_root (
    TYPE fs,
    SCOPE 'fs://',
    config MAP{'root': '/var/data'}
);

SELECT * FROM read_parquet('fs:///events/2026.parquet');   -- /var/data/events/2026.parquet
```

| Key                | Meaning                                                  |
| ------------------ | -------------------------------------------------------- |
| `root`             | Base directory every path is resolved against. Required. |
| `atomic_write_dir` | Staging directory for atomic writes (same filesystem).   |

Use `'root': '.'` to work relative to DuckDB's current working directory.

## Notes

- The root confines access: paths cannot escape it.
- Writes create parent directories as needed.
- DuckDB's own paths (no scheme) are unaffected — only `fs://` and `file://`
  URLs go through this extension.
