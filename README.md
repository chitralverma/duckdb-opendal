# duckdb-opendal (`opendal`)

A DuckDB extension that integrates [Apache OpenDAL](https://opendal.apache.org/) as a virtual filesystem, enabling transparent read and write access to multiple storage backends through a unified SQL interface.

Query, glob, and write files (Parquet, CSV, JSON, etc.) directly on remote and local storage using standard SQL.

---

## Key Features

- **Unified Virtual Filesystem**: Serve files from multiple services (currently `fs://` / `file://`, `s3://`, and `memory://` — with more backends coming soon) directly within DuckDB queries.
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

## Building from Source

```sh
git clone --recurse-submodules https://github.com/chitralverma/duckdb-opendal.git
cd duckdb-opendal
GEN=ninja make
```

This produces `./build/release/duckdb` (a shell with the extension preloaded) and
`./build/release/extension/opendal/opendal.duckdb_extension` (the loadable binary).

For architecture, testing, and how to add a service, see
[CONTRIBUTING.md](CONTRIBUTING.md).

---

## Contributing

Bug reports, feature requests, and pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the developer guide (build, test, and
architecture).

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
