# duckdb-opendal documentation

User guides for the `opendal` DuckDB extension. For installation and a quick
tour, start with the [README](../README.md).

## Guides

| Guide                                 | Read it when you want to                                                                    |
| ------------------------------------- | ------------------------------------------------------------------------------------------- |
| [Services](services/README.md)        | Connect to a storage service — one page per scheme, each with a working `CREATE SECRET`.    |
| [Configuration](configuration.md)     | Understand secrets and scopes, set global defaults, tune concurrency, retries and timeouts. |
| [Caching](caching.md)                 | Speed up repeated reads, or add a persistent on-disk cache.                                 |
| [Table functions](table-functions.md) | List, stat, measure or copy objects from SQL.                                               |
| [Coexistence](coexistence.md)         | Share a scheme such as `s3://` with DuckDB's `httpfs` extension.                            |

## Quick orientation

Every URL has the same shape, whatever the service:

```text
scheme://authority/path/to/object
```

Credentials and service options come from a DuckDB secret whose `SCOPE` matches
the URL, so queries stay plain SQL:

```sql
CREATE SECRET warehouse (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{'access_key_id': '...', 'secret_access_key': '...', 'region': 'us-east-1'}
);

SELECT * FROM read_parquet('s3://warehouse/events/2026.parquet');
```

Two functions answer most "why doesn't this work?" questions:

```sql
FROM opendal_schemes();                  -- which services this build supports
FROM opendal_info('s3://warehouse/x');   -- is this URL usable, and with which secret?
```
