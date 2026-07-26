# `memory://` — in-process memory

An ephemeral store that lives inside the DuckDB process. Useful for tests,
scratch space, and trying out the extension without any credentials.

## URL

```text
memory:///path/to/object
```

The authority is empty, so paths use **three** slashes. Writing to
`memory://objects/a.parquet` (two slashes) instead treats `objects` as the
authority — a different, empty store.

## Secret

None needed:

```sql
COPY (SELECT range AS id FROM range(1000))
TO 'memory:///scratch/ids.parquet' (FORMAT parquet);

SELECT count(*) FROM read_parquet('memory:///scratch/ids.parquet');
```

You may still create one to attach tuning sections:

```sql
CREATE SECRET mem (
    TYPE memory,
    SCOPE 'memory://',
    io_config MAP{'read.concurrent': '2'}
);
```

## Notes

- Contents are lost when the DuckDB process exits.
- Data is held in process memory, outside DuckDB's memory limit — keep an eye on
  how much you write.
