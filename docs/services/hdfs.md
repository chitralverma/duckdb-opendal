# `hdfs-native://` — HDFS

Reads and writes against an HDFS cluster over the namenode's RPC protocol, using
a pure-Rust client — no JVM, no `libhdfs`, nothing to install alongside DuckDB.

Unlike the object stores, HDFS has real directories: `opendal_stat` on a prefix
returns `mode = 'directory'`, server-side rename is supported, and recursive
delete removes a subtree.

## URL

```text
hdfs-native://namenode.example.com:8020/path/to/file
```

The authority is the **namenode's RPC address**; it becomes the `name_node`
(`hdfs://<authority>`), and the rest of the URL is the path.

## Secret

A cluster that needs no client-side settings needs no secret:

```sql
SELECT * FROM read_parquet('hdfs-native://namenode.example.com:8020/data/events.parquet');
```

Create one to pin a root, or to reach a nameservice (see below):

```sql
CREATE SECRET hdfs (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://namenode.example.com:8020',
    config MAP{'root': '/user/analytics'}
);
```

| Key         | Meaning                                                        |
| ----------- | -------------------------------------------------------------- |
| `name_node` | Namenode URL. Taken from the URL authority; see nameservices.    |
| `root`      | Prefix every path is resolved against.                           |

`hdfs-native` needs quoting in `CREATE SECRET` (`TYPE "hdfs-native"`) because of
the hyphen.

## Nameservices (HA)

Clusters with two namenodes — the norm on Cloudera/HDP — are addressed by
listing both. A comma cannot go in a URL authority, so leave the authority empty
and put the list in the secret; the client then treats them as one nameservice
and fails over between them:

```sql
CREATE SECRET hdfs_ha (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://',
    config MAP{'name_node': 'hdfs://nn1.example.com:8020,nn2.example.com:8020'}
);

SELECT * FROM read_parquet('hdfs-native:///data/events.parquet');
```

Note the three slashes: the authority is empty, so the path starts right after
them.

## Client configuration

The secret carries only `name_node` and `root`. Everything else the client
understands — failover policy, observer reads, ViewFS mount tables, datanode
hostname resolution, wire encryption, Kerberos — comes from Hadoop's own XML,
exactly as it would for any other HDFS client:

```sh
export HADOOP_CONF_DIR=/etc/hadoop/conf   # core-site.xml + hdfs-site.xml
duckdb
```

`$HADOOP_CONF_DIR` (or `$HADOOP_HOME/etc/hadoop`) is read at operator build
time; the settings the secret provides win over the files. So an existing
client config directory usually just works. The list of settings the client acts
on is in the
[hdfs-native README](https://github.com/Kimahriman/hdfs-native#supported-hdfs-settings).

The effective user comes from `HADOOP_USER_NAME`, falling back to the OS user:

```sh
HADOOP_USER_NAME=analytics duckdb   # files are written as `analytics`
```

## Security

- **Kerberos** works through the client's GSSAPI support, driven by
  `hadoop.security.authentication=kerberos` in your `HADOOP_CONF_DIR` and a
  ticket in the cache (`kinit`). It needs `libgssapi_krb5` present at runtime —
  `libgssapi-krb5-2` on Debian, `krb5-libs` on RHEL, `brew install krb5` on
  macOS. This extension's test tier is insecure, so Kerberos is not covered by
  its tests; treat it as inherited from
  [hdfs-native](https://github.com/Kimahriman/hdfs-native#kerberos-support)
  rather than something we verify.
- **Encryption at rest (KMS) is not available.** Reading or writing inside an
  HDFS encryption zone needs the client's `kms` feature, which OpenDAL does not
  enable, so those paths fail. Enabling it needs a change upstream in OpenDAL;
  the HTTPS variant additionally pulls in a second TLS provider (`aws-lc-rs`),
  which this extension avoids deliberately.
- Wire encryption (`dfs.data.transfer.protection`) and SASL negotiation are
  configured through `HADOOP_CONF_DIR` like everything else.

## Notes

- **Datanode reachability matters.** The namenode hands back block locations
  pointing at datanode hostnames; DuckDB connects to those directly, so they must
  resolve and be reachable from the client — the usual failure when a cluster
  runs behind NAT or inside Docker.
- Appends are supported (`write_can_append`), which most object stores cannot do.
- Erasure-coded reads and writes, observer reads, ViewFS and router-based
  federation come from the client; see its
  [feature list](https://github.com/Kimahriman/hdfs-native#supported-hdfs-features).
- `webhdfs://` is not built in: against the pinned OpenDAL revision a failed
  WebHDFS rename is reported as a successful write. Use `hdfs-native://`, which
  reaches the same cluster.
