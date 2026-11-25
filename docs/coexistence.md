# Coexistence with DuckDB filesystems

DuckDB and its core extensions serve some URL schemes that `opendal` also
serves. This page lists those schemes, explains which filesystem serves a URL,
and shows how to choose `opendal`.

## Terms

| Term           | Meaning                                                                                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claimed scheme | A URL scheme that DuckDB core or a core extension serves. [Claimed schemes](#claimed-schemes) lists them.                                                                 |
| Shared scheme  | A claimed scheme that `opendal` also serves: `file://`, `http://`, `https://`, `s3://`, `gcs://` and `r2://`.                                                             |
| Counterpart    | The filesystem in DuckDB core or a core extension that serves the same storage service as an `opendal` service. Its schemes can differ, for example `az://` for `azblob`. |

The **Notes** section of each [service guide](services/README.md) names the
counterpart of that service and the differences from it.

## Claimed schemes

DuckDB v2.0 and its core extensions:

| Component                                           | Filesystem                   | Claimed schemes                   | Storage service              | `opendal` service                                                                     |
| --------------------------------------------------- | ---------------------------- | --------------------------------- | ---------------------------- | ------------------------------------------------------------------------------------- |
| DuckDB core                                         | `LocalFileSystem`            | `file://`, paths without a scheme | Local files                  | [`fs`](services/fs.md): `fs://`, **`file://`**                                        |
| [`httpfs`](https://github.com/duckdb/duckdb-httpfs) | `HTTPFileSystem`             | `http://`, `https://`             | HTTP servers                 | [`http`](services/http.md): **`http://`**, **`https://`**                             |
| `httpfs`                                            | `S3FileSystem`               | `s3://`, `s3a://`, `s3n://`       | S3 and S3-compatible stores  | [`s3`](services/s3-like.md): **`s3://`**, [`minio`](services/minio-r2.md): `minio://` |
| `httpfs`                                            | `S3FileSystem`               | `gcs://`, `gs://`                 | Google Cloud Storage         | [`gcs`](services/gcs.md): **`gcs://`**                                                |
| `httpfs`                                            | `S3FileSystem`               | `r2://`                           | Cloudflare R2                | [`r2`](services/minio-r2.md): **`r2://`**                                             |
| `httpfs`                                            | `HuggingFaceFileSystem`      | `hf://`                           | Hugging Face Hub             | none                                                                                  |
| [`azure`](https://github.com/duckdb/duckdb-azure)   | `AzureBlobStorageFileSystem` | `azure://`, `az://`               | Azure Blob Storage           | [`azblob`](services/azure.md): `azblob://`                                            |
| `azure`                                             | `AzureDfsStorageFileSystem`  | `abfss://`, `abfs://`             | Azure Data Lake Storage Gen2 | [`azdls`](services/azure.md): `azdls://`                                              |

- Shared schemes are in bold.
- `memory` and `azfile` have no counterpart.
- **Filesystem** is the name that `disabled_filesystems` and the DuckDB
  `FileSystem` log use. `opendal` is `OpenDalFileSystem`.
- No other core extension registers a filesystem.
- `httpfs` also serves each scheme in its `s3_url_scheme_aliases` setting.
- A community extension can serve any scheme. To see which filesystem serves a
  URL, use the query in [Which filesystem serves a URL](#which-filesystem-serves-a-url).

## Which filesystem serves a URL

For each URL, DuckDB asks every loaded filesystem if it can serve the URL:

1. If one or more of them report that they are _manually set_, DuckDB uses the
   one that was loaded first.
2. If not, DuckDB uses the one that was loaded last.
3. If none can serve the URL and its scheme is a claimed scheme other than
   `file://` or `abfs://`, DuckDB autoloads the extension and asks again.
4. If none can serve the URL, DuckDB uses `LocalFileSystem`.

`opendal` can serve every URL whose scheme `FROM opendal_schemes();` lists,
except a URL with a query string or a fragment (`?`, `#`). It reports that it is
manually set only for the schemes in
[`opendal_override_schemes`](#choose-opendal-for-a-scheme).

The results:

- **Shared schemes go to `opendal`** when it is loaded after `httpfs`. You do
  not need the override for this.
- **A URL with a query string or a fragment goes to `httpfs`**, for example a
  pre-signed URL. If `httpfs` is not loaded, DuckDB tries to autoload it, which
  fails (see [Load order](#load-order)).
- **`file://` goes to `opendal`**, because `LocalFileSystem` serves only the
  URLs that no other filesystem can serve. A `file://` URL then needs a
  `TYPE file` secret, or it fails with `root is not specified`. Paths without a
  scheme stay with DuckDB core. Set `root` to the narrowest directory that the
  queries need, because a URL cannot leave it:

  ```sql
  CREATE SECRET local_files (TYPE file, SCOPE 'file://', config MAP{'root': '/srv/data'});
  ```

  `'root': '/'` gives `file://` the same access as DuckDB core.

- **A filesystem that reports that it is manually set wins over `opendal`**, if
  it was loaded first, even for a scheme in the override list. The filesystems
  that `cache_httpfs` wraps work like this.

To see which filesystem served a URL, read the `FileSystem` log:

```sql
CALL enable_logging('FileSystem', storage = 'memory');

SELECT * FROM read_parquet('s3://warehouse/a.parquet');

SELECT DISTINCT message->>'path' AS url, message->>'fs' AS filesystem
FROM duckdb_logs
WHERE type = 'FileSystem';
```

## Choose `opendal` for a scheme

`opendal_override_schemes` is a comma-separated list of schemes for which
`opendal` reports that it is manually set:

```sql
SET opendal_override_schemes = 's3,gcs';

SELECT opendal_is_manually_set('s3://warehouse/a.parquet');   -- true

SET opendal_override_schemes = '';   -- remove all overrides
```

- The default is empty.
- A scheme that `opendal` does not serve has no effect.
- The setting applies to every connection and every database in the process.
- It changes rule 1 only. A filesystem that was loaded before `opendal` and
  that reports that it is manually set still wins.

`SET disabled_filesystems = '<Filesystem>'` does not send a URL to a different
filesystem. A URL that the disabled filesystem would serve fails with a
permission error.

## Load order

**Load `httpfs` before `opendal`.** Both register the `s3`, `gcs` and `r2`
secret types. `opendal` accepts a type that is already registered. `httpfs` does
not, so after `opendal` is loaded, `LOAD httpfs` fails with
`Attempted to register an already registered secret type`. Every autoload of
`httpfs` fails in the same way, for example on a `gs://`, `s3a://` or `hf://` URL
or on `SET s3_region`. Extensions that load `httpfs`, such as `cache_httpfs`,
fail too.

The `azure` extension shares no secret type with `opendal` and loads in either
order.

## Secrets

`opendal` replaces the `CREATE SECRET` syntax of the secret types it shares:
`s3`, `gcs` and `r2` from `httpfs`, and `http` from DuckDB core. After
`LOAD opendal`, the default provider of these types takes the parameters in
[configuration](configuration.md), and parameters such as `KEY_ID`, `SECRET` or
`BEARER_TOKEN` fail with `Unknown parameter`. Other providers, such as
`PROVIDER credential_chain`, keep their syntax.

A secret works only for the extension whose syntax created it. Both extensions
find a secret by its `SCOPE`, but `httpfs` ignores the `opendal` parameters, and
`opendal` rejects a secret that `httpfs` created. To use both extensions, create
the `httpfs` secrets before `LOAD opendal`, and give each secret a `SCOPE` that
only one extension serves.
