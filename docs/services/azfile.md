# `azfile://` — Azure Files

SMB-style file shares in an Azure storage account, over the Files REST API.

## URL

```text
azfile://myaccount.file.core.windows.net/path/to/file
```

The authority is the **account host**. The endpoint (`https://` + authority) and
the account name (everything before the first dot) are taken from it, so the
secret only carries the share name and the credential.

## Secret

```sql
CREATE SECRET az_file (
    TYPE azfile,
    SCOPE 'azfile://myaccount.file.core.windows.net',
    config MAP{
        'share_name': 'warehouse',
        'account_key': '...'
    }
);

SELECT * FROM read_parquet(
    'azfile://myaccount.file.core.windows.net/events/2026.parquet'
);
```

| Key           | Meaning                                            |
| ------------- | -------------------------------------------------- |
| `share_name`  | File share name. Required.                         |
| `account_key` | Shared key.                                        |
| `sas_token`   | Shared access signature, instead of `account_key`. |

## Notes

- An `endpoint` in the secret is ignored — the URL authority always wins.
- One share per secret.
- Azure Files is optimised for file-share semantics rather than analytics
  throughput; for large scans prefer [`azblob://`](azblob.md) or
  [`azdls://`](azdls.md).
- The Azurite emulator does not implement the Files endpoint; this service needs
  a real Azure account.
