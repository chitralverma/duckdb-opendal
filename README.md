# duckdb-opendal (`opendal`)

A DuckDB extension that integrates [Apache OpenDAL](https://opendal.apache.org/) as a virtual filesystem, enabling transparent read and write access to multiple storage services through a unified SQL interface.

Query, glob, and write files (Parquet, CSV, JSON, etc.) directly on remote and local storage using standard SQL.

---

## Installation

Install the signed extension from the DuckDB community repository:

```sql
INSTALL opendal FROM community;
LOAD opendal;
```

> **Requires DuckDB ≥ 1.5.5.** Community binaries are published per DuckDB
> version; the `opendal` extension is built for the current stable release
> (currently v1.5.5). On older versions, `INSTALL opendal FROM community` will
> return an HTTP 404 — upgrade DuckDB to the latest patch release. To target an
> older DuckDB, build from source (see below) against the matching version and
> load the binary with `allow_unsigned_extensions`.

---

## Key Features

- **Unified Virtual Filesystem**: Serve files from multiple services (currently `fs://` / `file://`, `s3://`, and `memory://` — with more services coming soon) directly within DuckDB queries.
- **Table Functions**:
  - `opendal_version()` — Returns extension and core OpenDAL library versions.
  - `opendal_stat()` — Returns metadata for a single path (such as mode, size, and user_metadata).
  - `opendal_ls()` — Lists files and subdirectories with optional recursion.
  - `opendal_glob()` — Performs pattern-based file listing.
  - `opendal_du()` — Returns aggregated size rollups grouped by parent directory.
  - `opendal_copy()` — Efficiently copies objects within or across storage services (drains OpenDAL's native server-side `Copier` where supported, or falls back to chunked streaming).
- **Scoped Secrets**: Configure credentials, custom endpoints, and regions with bucket/path specificity using DuckDB's native secret manager.
- **Configurable Layers**: Tuning of retry behavior, timeout ceilings, concurrent request limits, and data caching per secret or globally.
- **Built-in Data Caching**: Support for in-memory and persistent on-disk data caching (leveraging Foyer) without any manual query adjustments.
- **Coexistence Layer**: Dynamically delegate specific schemes (like `s3://`) to `opendal` instead of native extensions (like `httpfs`) using `opendal_override_native_filesystems`.

---

## SQL Usage Examples

### 1. Simple Local Read & Write

Ensure the local filesystem root is configured via secrets:

```sql
-- Register a secret for local filesystem access
CREATE SECRET local_root (
    TYPE fs,
    SCOPE 'fs://',
    config MAP{'root': '.'}
);

-- Write a Parquet file to a relative path
COPY (SELECT range AS id, range * 2 AS doubled FROM range(1000))
TO 'fs:///output_data.parquet' (FORMAT parquet);

-- Read the Parquet file back
SELECT count(*), sum(doubled)
FROM read_parquet('fs:///output_data.parquet');
```

### 2. S3 / Object Storage Configuration

Configure S3 options under a scoped secret:

```sql
CREATE SECRET warehouse (
    TYPE s3,
    SCOPE 's3://warehouse-bucket',
    config MAP{
        'access_key_id': 'your-access-key',
        'secret_access_key': 'your-secret-key',
        'region': 'us-east-1',
        'endpoint': 'http://127.0.0.1:9000'
    },
    io_config MAP{
        'read.concurrent': '4',
        'read.chunk_size': '8 MiB',
        'write.concurrent': '4',
        'write.chunk_size': '8 MiB'
    },
    retry_config MAP{
        'max_times': '5',
        'jitter': 'true'
    }
);

-- Read from the S3 bucket
SELECT * FROM read_parquet('s3://warehouse-bucket/data/**/*.parquet');
```

### 3. Storage Utilities & Table Functions

```sql
-- Stat a file
SELECT * FROM opendal_stat('s3://warehouse-bucket/data/file.parquet');

-- List directory contents recursively
SELECT name, metadata.content_length
FROM opendal_ls('s3://warehouse-bucket/data/', recursive := true);

-- Calculate disk usage rollup
SELECT parent, file_count, total_size
FROM opendal_du('s3://warehouse-bucket/data/');

-- Copy files across services
SELECT * FROM opendal_copy(
    's3://warehouse-bucket/data/input.csv',
    'fs:///local_backups/backup.csv'
);
```

### 4. Native Coexistence

By default, DuckDB routes `s3://` and `gcs://` to its native `httpfs` extension. You can instruct `opendal` to take precedence for specific schemes using:

```sql
-- Let opendal handle s3:// and gcs:// queries
SET opendal_override_native_filesystems = 's3,gcs';

-- Queries under these schemes now run through opendal's filesystem registry
SELECT * FROM read_parquet('s3://my-bucket/dataset.parquet');

-- Revert back to httpfs handling
SET opendal_override_native_filesystems = '';
```

---

## Compatibility

**Use the latest DuckDB release to get the latest services.** Community binaries
are published per DuckDB version and the extension is built for the current
stable release, so the set of available services and fixes tracks the extension
build for *your* DuckDB (see [Installation](#installation)). Check your installed
version with `SELECT opendal_version();` and cross-reference the matrix below.

Each release pins a DuckDB version and an Apache OpenDAL revision:

| Extension | DuckDB | OpenDAL rev | Services              |
| --------- | ------ | ----------- | --------------------- |
| 0.1.0     | v1.5.5 | `3180510`   | `fs`, `memory`, `s3`  |

See the [CHANGELOG](CHANGELOG.md) for per-version details, and
[MAINTAINING.md](MAINTAINING.md) for how these versions are upgraded.

---

## Contributing

Bug reports, feature requests, and pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the developer guide (build, test, and
architecture).

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
