# Coexistence with other DuckDB filesystems

DuckDB core, its core extensions and community extensions can serve URL schemes
that `opendal` also serves. This page lists the schemes from DuckDB core and its
core extensions, explains which filesystem serves a URL, and shows how to choose
`opendal`. The same rules apply to community extensions.

## Terms

| Term           | Meaning                                                                                                                                                        |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claimed scheme | A URL scheme that DuckDB core, a core extension or a community extension serves.                                                                               |
| Shared scheme  | A claimed scheme that `opendal` also serves.                                                                                                                   |
| Counterpart    | A filesystem from DuckDB core, a core extension or a community extension that serves the same storage service as an `opendal` service. Its schemes can differ. |

The **Notes** section of each [service guide](services/README.md) names the
counterpart from DuckDB core or a core extension, and the differences from it.

## Claimed schemes

From DuckDB v2.0 core and its core extensions:

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

- Shared schemes are in bold. With DuckDB core and its core extensions, they are
  `file://`, `http://`, `https://`, `s3://`, `gcs://` and `r2://`.
- `memory` and `azfile` have no counterpart in DuckDB core or a core extension.
- **Filesystem** is the name that `disabled_filesystems` and the DuckDB
  `FileSystem` log use. `opendal` is `OpenDalFileSystem`.
- No other core extension registers a filesystem.
- `httpfs` also serves each scheme in its `s3_url_scheme_aliases` setting.

Community extensions are not listed, because the set changes often. A community
extension can claim any scheme, including one that only `opendal` serves among
DuckDB core and its core extensions, such as `azblob://`. It can also register
secret types and report that it is _manually set_. Before you use one with
`opendal`, read its documentation for the schemes and secret types it
registers. To see which filesystem served a URL, use the query in
[Which filesystem serves a URL](#which-filesystem-serves-a-url).

## Which filesystem serves a URL

For each URL, DuckDB asks every loaded filesystem if it can serve the URL:

1. If one or more of them report that they are _manually set_, DuckDB uses the
   one that was loaded first.
2. If not, DuckDB uses the one that was loaded last.
3. If none can serve the URL and its scheme is in the
   [claimed schemes](#claimed-schemes) table, except `file://` and `abfs://`,
   DuckDB autoloads that core extension and asks again. DuckDB does not
   autoload community extensions.
4. If none can serve the URL, DuckDB uses `LocalFileSystem`.

`opendal` can serve every URL whose scheme `FROM opendal_schemes();` lists,
except a URL with a query string or a fragment (`?`, `#`). It reports that it is
manually set only for the schemes in
[`opendal_override_schemes`](#choose-opendal-for-a-scheme).

The results:

- **A shared scheme goes to `opendal`** when `opendal` is loaded after the
  other filesystems that serve it, and none of them reports that it is manually
  set. You do not need the override for this.
- **A URL with a query string or a fragment goes to another filesystem**, for
  example a pre-signed `s3://` URL goes to `httpfs`. If no loaded filesystem
  can serve it, DuckDB tries to autoload `httpfs`, which fails (see
  [Load order](#load-order)).
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
  it was loaded first, even for a scheme in the override list. Some community
  extensions do this for all of their schemes, or for the filesystems they wrap.

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

**Load other storage extensions before `opendal`.** `opendal` registers one
secret type per scheme it serves, and accepts a type that is already
registered. An extension that registers the same type after `opendal` fails to
load if it does not accept this.

`httpfs` is such an extension. It registers the `s3`, `gcs` and `r2` secret
types, so after `opendal` is loaded, `LOAD httpfs` fails with
`Attempted to register an already registered secret type`. Every autoload of
`httpfs` fails in the same way, for example on a `gs://`, `s3a://` or `hf://` URL
or on `SET s3_region`. Extensions that load `httpfs` fail too.

A load before `opendal` has one cost: if that extension reports that it is
manually set, it wins over `opendal_override_schemes` (see
[Which filesystem serves a URL](#which-filesystem-serves-a-url)). For such an
extension, choose the order by the filesystem you want to serve its shared
schemes.

The `azure` extension shares no secret type with `opendal` and loads in either
order.

## Secrets

`opendal` replaces the `CREATE SECRET` syntax of each secret type that another
extension registered first with the same name. From DuckDB core and its core
extensions, these are `s3`, `gcs` and `r2` from `httpfs`, and `http` from DuckDB
core. After `LOAD opendal`, the default provider of these types takes the
parameters in [configuration](configuration.md), and parameters such as
`KEY_ID`, `SECRET` or `BEARER_TOKEN` fail with `Unknown parameter`. Other
providers, such as `PROVIDER credential_chain`, keep their syntax.

A secret works only for the extension whose syntax created it. Each extension
finds a secret by its `SCOPE`, but ignores or rejects the parameters of the
other. For example, `httpfs` ignores the `opendal` parameters, and `opendal`
rejects a secret that `httpfs` created. To use both extensions, create the
secrets of the other extension before `LOAD opendal`, and give each secret a
`SCOPE` that only one extension serves.
