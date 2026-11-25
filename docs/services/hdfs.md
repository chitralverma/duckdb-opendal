# `hdfs-native://`: HDFS

Reads and writes HDFS over the namenode RPC protocol. The client is pure Rust:
no JVM, no `libhdfs`.

HDFS has real directories. `opendal_stat` on a prefix returns
`mode = 'directory'`, rename is server-side, and recursive delete removes a
subtree.

## URL

```text
hdfs-native://namenode.example.com:8020/path/to/file
```

The authority is a namenode RPC address, or a nameservice from the client
configuration. The rest of the URL is the path.

## Secret

A cluster that needs no client settings needs no secret:

```sql
SELECT * FROM read_parquet('hdfs-native://namenode.example.com:8020/data/events.parquet');
```

Create a secret to set a root, or to reach a nameservice:

```sql
CREATE SECRET hdfs (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://namenode.example.com:8020',
    config MAP{'root': '/user/analytics'}
);
```

For all `config` keys, see the OpenDAL
[`hdfs-native` configuration reference](https://opendal.apache.org/services/hdfs-native/#configuration-reference).

`name_node` comes from the URL authority unless the secret sets it.
`TYPE "hdfs-native"` needs quotes because of the hyphen.

### Nameservices (HA)

A nameservice in the client configuration (`dfs.nameservices`) works as the
URL authority. The client fails over between its namenodes:

```sql
SELECT * FROM read_parquet('hdfs-native://mycluster/data/events.parquet');
```

Without a nameservice in the configuration, list the namenodes in the secret.
A URL authority cannot hold a comma, so leave it empty:

```sql
CREATE SECRET hdfs_ha (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://',
    config MAP{'name_node': 'hdfs://nn1.example.com:8020,nn2.example.com:8020'}
);

SELECT * FROM read_parquet('hdfs-native:///data/events.parquet');
```

The URL has three slashes because the authority is empty.

For router-based federation, list the router addresses.

### Default file system and ViewFS

`name_node = 'default'` uses `fs.defaultFS` from the client configuration.
When it is a `viewfs://` mount table, paths go through its links:

```sql
CREATE SECRET hdfs_default (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://',
    config MAP{'name_node': 'default'}
);

SELECT * FROM read_parquet('hdfs-native:///data/events.parquet');
```

## Client configuration

Everything except `name_node` and `root` comes from the Hadoop XML files:
datanode hostnames, wire encryption, Kerberos and KMS.

```sh
export HADOOP_CONF_DIR=/etc/hadoop/conf   # core-site.xml + hdfs-site.xml
duckdb
```

The client reads `$HADOOP_CONF_DIR`, or `$HADOOP_HOME/etc/hadoop`, when DuckDB
first opens the URL. The secret wins over the files. The
[hdfs-native README](https://github.com/Kimahriman/hdfs-native#supported-hdfs-settings)
lists the settings the client uses.

The user comes from `HADOOP_USER_NAME`, then from the OS user:

```sh
HADOOP_USER_NAME=analytics duckdb   # files are written as `analytics`
```

## Security

- **Kerberos**: set `hadoop.security.authentication=kerberos` in
  `HADOOP_CONF_DIR` and get a ticket with `kinit`. The client loads
  `libgssapi_krb5` at runtime: `libgssapi-krb5-2` on Debian, `krb5-libs` on
  RHEL, `brew install krb5` on macOS.
- **Encryption zones (KMS)**: set `hadoop.security.key.provider.path` in
  `HADOOP_CONF_DIR`, for example `kms://https@kms.example.com:9600/kms`. With
  Kerberos, the client authenticates to the KMS with SPNEGO.
- **Wire encryption**: set `hadoop.rpc.protection` and
  `dfs.data.transfer.protection` in `HADOOP_CONF_DIR`.

## Emulator

The test tier runs MiniDFS, the test cluster of the
[hdfs-native](https://github.com/Kimahriman/hdfs-native) client, with host
networking: a namenode on `127.0.0.1:9000`, four datanodes, a KMS and the
encryption zone `/ezone`. MiniDFS writes the client config, with the KMS
address, to `/minidfs/target/test/core-site.xml`. Copy it out of the container
and set `HADOOP_CONF_DIR` to its directory. The tier adds an `hdfs-site.xml`
with the nameservice `odfs-ns` and the ViewFS mount table `odfs-view` as
`fs.defaultFS`. The tier is in
[`test/services/hdfs-native/`](../../test/services/hdfs-native/).

## Notes

- **The client connects to datanodes directly.** The namenode returns datanode
  addresses, so they must resolve and be reachable from DuckDB. A cluster behind
  NAT or in Docker usually fails here. Set `dfs.client.use.datanode.hostname`
  if the datanodes advertise addresses that only work inside the cluster.
- Erasure-coded reads and writes come from the client; see its
  [feature list](https://github.com/Kimahriman/hdfs-native#supported-hdfs-features).
- `webhdfs://` is not built in. Use `hdfs-native://` for the same cluster.
