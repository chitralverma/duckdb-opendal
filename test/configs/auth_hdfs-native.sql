-- Init script for the hdfs-native:// service (loaded by
-- test/configs/hdfs-native.json). The namenode RPC address comes from the URL
-- authority, and the test cluster runs with permissions disabled, so the secret
-- exists only to register the scheme's configuration surface.
CREATE SECRET hdfs_native_common (
    TYPE "hdfs-native", SCOPE 'hdfs-native://${OPENDAL_HDFS_NATIVE_AUTHORITY}'
);
