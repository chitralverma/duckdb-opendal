# `azblob://`, `azdls://`, `azfile://` — Azure Storage

Three Azure services, one per storage API:

| Scheme      | Service                      | Use it for                                        |
| ----------- | ---------------------------- | ------------------------------------------------- |
| `azblob://` | Blob Storage                 | General object storage; the usual choice.          |
| `azdls://`  | Data Lake Storage Gen2       | Accounts with a hierarchical namespace.            |
| `azfile://` | Azure Files                  | Existing SMB file shares. Slower for large scans.  |

## URL

```text
azblob://container/path/to/blob
azdls://myaccount.dfs.core.windows.net/path/to/object
azfile://myaccount.file.core.windows.net/path/to/file
```

`azblob` takes the **container** as the authority; the account comes from the
endpoint in the secret.

`azdls` and `azfile` take the **account host**. The endpoint (`https://` +
authority) and the account name (up to the first dot) are read from it, so their
secrets carry only the filesystem/share and the credential — and an `endpoint`
in the secret is ignored. For a sovereign cloud, put its host in the URL
(`azdls://myaccount.dfs.core.chinacloudapi.cn/…`).

## Secret

```sql
CREATE SECRET az_blob (
    TYPE azblob,
    SCOPE 'azblob://warehouse',
    config MAP{
        'endpoint': 'https://myaccount.blob.core.windows.net',
        'account_name': 'myaccount',
        'account_key': '...'
    }
);

CREATE SECRET az_dls (
    TYPE azdls,
    SCOPE 'azdls://myaccount.dfs.core.windows.net',
    config MAP{'filesystem': 'warehouse', 'account_key': '...'}
);

CREATE SECRET az_file (
    TYPE azfile,
    SCOPE 'azfile://myaccount.file.core.windows.net',
    config MAP{'share_name': 'warehouse', 'account_key': '...'}
);

SELECT * FROM read_parquet('azblob://warehouse/events/2026.parquet');
```

For all `config` keys, see the OpenDAL configuration reference for
[`azblob`](https://opendal.apache.org/services/azblob/#configuration-reference),
[`azdls`](https://opendal.apache.org/services/azdls/#configuration-reference)
and [`azfile`](https://opendal.apache.org/services/azfile/#configuration-reference).
For `azdls` and `azfile`, this extension sets `endpoint` and `account_name` from
the URL, as [URL](#url) describes.

One container, filesystem or share per secret — give each its own `SCOPE`.

## Emulator

Azurite covers `azblob` only, and serves the account from a path segment:

```sql
CREATE SECRET az_local (
    TYPE azblob,
    SCOPE 'azblob://warehouse',
    config MAP{
        'endpoint': 'http://127.0.0.1:10000/devstoreaccount1',
        'account_name': 'devstoreaccount1',
        'account_key': 'Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=='
    }
);
```

(That account name and key are Azurite's public defaults.) There is no emulator
for the Data Lake or Files endpoints — `azdls` and `azfile` need a real account.

## Notes

- **Counterpart:** [`azure`](../coexistence.md#claimed-schemes) for `azblob`
  (`az://`, `azure://`) and `azdls` (`abfss://`, `abfs://`). `azfile` has none
  in DuckDB core or a core extension. The schemes differ, so a URL goes to one
  of these two extensions only. A community extension can still claim
  `azblob://`, `azdls://` or `azfile://`.
- On a hierarchical-namespace account, prefer `azdls://` over `azblob://`: it
  exposes real directories, directory rename and recursive delete.
