# `azblob://` — Azure Blob Storage

Block-blob storage in an Azure storage account.

## URL

```text
azblob://container/path/to/blob
```

The authority is the **container**. The storage account is not part of the URL —
it comes from the endpoint in the secret.

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

SELECT * FROM read_parquet('azblob://warehouse/events/2026.parquet');
```

| Key              | Meaning                                                        |
| ---------------- | -------------------------------------------------------------- |
| `endpoint`       | Blob endpoint, e.g. `https://<account>.blob.core.windows.net`. |
| `account_name`   | Storage account name.                                          |
| `account_key`    | Shared key. Alternative: `sas_token`.                          |
| `sas_token`      | Shared access signature, instead of `account_key`.             |
| `skip_signature` | `true` for public containers that need no credentials.         |

## Azurite (local emulator)

Azurite serves the account from a path segment, so include it in the endpoint:

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

(That account name and key are Azurite's public defaults.)

## Notes

- One secret per container; give each its own `SCOPE`.
- This is a different scheme from DuckDB's native `azure` extension
  (`azure://` / `abfss://`), so the two never collide.
- For hierarchical-namespace accounts you may prefer
  [`azdls://`](azdls.md), which exposes real directories.
