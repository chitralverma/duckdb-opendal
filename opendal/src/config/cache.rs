use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use opendal::Operator;

use crate::runtime::block_on;

use super::value::{non_empty_path, positive_usize, unknown, ByteSize};

#[derive(Clone, Debug)]
pub(crate) struct CacheConfig {
    /// In-memory data-cache size.
    memory_size: ByteSize,
    /// Optional base directory for isolated persistent cache namespaces.
    disk_path: Option<PathBuf>,
    /// Persistent cache size.
    disk_size: ByteSize,
    /// Persistent cache block size.
    block_size: ByteSize,
    /// Smallest object admitted to the cache.
    min_file_size: ByteSize,
    /// Largest object admitted to the cache.
    max_file_size: Option<ByteSize>,
    /// In-memory shard count.
    shards: NonZeroUsize,
}

impl CacheConfig {
    pub(crate) fn parse(values: BTreeMap<String, String>) -> Result<Option<Self>, String> {
        if values.is_empty() {
            return Ok(None);
        }
        let mut config = Self {
            memory_size: ByteSize::parse("cache", "memory_size", "256 MiB")?,
            disk_path: None,
            disk_size: ByteSize::parse("cache", "disk_size", "1 GiB")?,
            block_size: ByteSize::parse("cache", "block_size", "4 MiB")?,
            min_file_size: ByteSize::parse("cache", "min_file_size", "0")?,
            max_file_size: None,
            shards: NonZeroUsize::new(4).unwrap(),
        };
        for (key, value) in values {
            match key.as_str() {
                "memory_size" => config.memory_size = ByteSize::parse("cache", &key, &value)?,
                "disk_path" => config.disk_path = Some(non_empty_path("cache", &key, &value)?),
                "disk_size" => config.disk_size = ByteSize::parse("cache", &key, &value)?,
                "block_size" => config.block_size = ByteSize::parse("cache", &key, &value)?,
                "min_file_size" => config.min_file_size = ByteSize::parse("cache", &key, &value)?,
                "max_file_size" => {
                    config.max_file_size = Some(ByteSize::parse("cache", &key, &value)?)
                }
                "shards" => config.shards = positive_usize("cache", &key, &value)?,
                _ => return Err(unknown("cache", &key)),
            }
        }
        for (key, size) in [
            ("memory_size", config.memory_size),
            ("disk_size", config.disk_size),
            ("block_size", config.block_size),
        ] {
            if size.get() == 0 {
                return Err(format!("invalid cache.{key}='0': expected a positive size"));
            }
        }
        if matches!(config.max_file_size, Some(max) if config.min_file_size > max) {
            return Err("cache.min_file_size must not exceed cache.max_file_size".to_string());
        }
        Ok(Some(config))
    }

    pub(crate) fn apply(
        &self,
        op: Operator,
        namespace: Option<&str>,
    ) -> (Operator, Option<String>) {
        let layer = match self.build_layer(namespace) {
            Ok(layer) => layer,
            Err(error) => {
                return (op, Some(format!("OpenDAL cache disabled: {error}")));
            }
        };
        let min = self.min_file_size.get();
        let layer = match self.max_file_size {
            Some(max) => {
                let max = max.get();
                if max < usize::MAX {
                    layer.with_size_limit(min..max + 1)
                } else {
                    layer.with_size_limit(min..)
                }
            }
            None => layer.with_size_limit(min..),
        };
        (op.layer(layer), None)
    }

    fn build_layer(&self, namespace: Option<&str>) -> Result<opendal::layers::FoyerLayer, String> {
        use foyer::{
            BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder,
            PsyncIoEngineConfig,
        };

        let memory_size = self.memory_size.get();
        let disk_size = self.disk_size.get();
        let block_size = self.block_size.get();
        let disk_path = self
            .disk_path
            .as_ref()
            .map(|path| path.join(namespace.unwrap_or("default")));
        let shards = self.shards.get();

        let cache = block_on(async move {
            let memory = HybridCacheBuilder::new()
                .memory(memory_size)
                .with_shards(shards);
            match disk_path {
                Some(path) => {
                    std::fs::create_dir_all(&path)
                        .map_err(|e| format!("mkdir {}: {e}", path.display()))?;
                    let device = FsDeviceBuilder::new(&path)
                        .with_capacity(disk_size)
                        .build()
                        .map_err(|e| format!("cache device ({}): {e}", path.display()))?;
                    memory
                        .storage()
                        .with_io_engine_config(PsyncIoEngineConfig::new())
                        .with_engine_config(
                            BlockEngineConfig::new(device).with_block_size(block_size),
                        )
                        .build()
                        .await
                        .map_err(|e| format!("cache build: {e}"))
                }
                None => memory
                    .storage()
                    .build()
                    .await
                    .map_err(|e| format!("cache build: {e}")),
            }
        })?;
        Ok(opendal::layers::FoyerLayer::new(cache))
    }
}
