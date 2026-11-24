use std::collections::BTreeMap;
use std::fs::{DirBuilder, File, TryLockError};
use std::num::NonZeroUsize;
use std::ops::RangeInclusive;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use foyer::{
    BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder,
    HybridCachePolicy, PsyncIoEngineConfig,
};
use human_units::iec::{Byte, Prefix};
use opendal::layers::CacheLayer;
use opendal::Operator;
use opendal_service_foyer::{Foyer, FoyerKey, FoyerValue};

use crate::runtime::block_on;

use super::value::{non_empty_path, positive_usize, unknown, ByteSize, HumanDuration};

/// The smallest chunk for which a cache fill still reads 4 MiB per request.
const DEFAULT_CHUNK: usize = iec(256, Prefix::Kibi);
const CHUNK_RANGE: RangeInclusive<usize> = iec(64, Prefix::Kibi)..=iec(4, Prefix::Mebi);
/// Foyer's default disk block, which fits any allowed chunk.
const DISK_BLOCK: usize = iec(16, Prefix::Mebi);
const MIN_DISK_SIZE: usize = 8 * DISK_BLOCK;

const fn iec(value: u64, prefix: Prefix) -> usize {
    Byte::with_iec_prefix(value, prefix).0 as usize
}

type Hybrid = HybridCache<FoyerKey, FoyerValue>;

/// Open disk caches by directory, so operators with one configuration share one.
static DISK_CACHES: Mutex<BTreeMap<PathBuf, Weak<DiskCache>>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Debug)]
pub(crate) struct CacheConfig {
    /// In-memory tier size.
    memory_size: ByteSize,
    /// Base directory of the disk tier; each configuration gets a subdirectory.
    disk_path: Option<PathBuf>,
    /// Disk tier size.
    disk_size: ByteSize,
    /// Size of the cached object ranges.
    chunk_size: ByteSize,
    /// How long object metadata is trusted before it is revalidated.
    ttl: HumanDuration,
    /// In-memory shard count.
    shards: NonZeroUsize,
}

/// Keeps an operator's disk cache, and the lock on its directory, alive.
pub(crate) struct CacheGuard {
    _disk: Option<Arc<DiskCache>>,
}

struct DiskCache {
    layer: CacheLayer,
    hybrid: Hybrid,
    _lock: File,
}

impl Drop for DiskCache {
    fn drop(&mut self) {
        // Finish disk writes before the directory lock, dropped last, is released.
        let _ = block_on(self.hybrid.close());
    }
}

impl CacheConfig {
    pub(crate) fn parse(values: BTreeMap<String, String>) -> Result<Option<Self>, String> {
        if values.is_empty() {
            return Ok(None);
        }
        let mut config = Self {
            memory_size: ByteSize::from_bytes(iec(256, Prefix::Mebi)),
            disk_path: None,
            disk_size: ByteSize::from_bytes(iec(1, Prefix::Gibi)),
            chunk_size: ByteSize::from_bytes(DEFAULT_CHUNK),
            ttl: HumanDuration::parse("cache", "ttl", "1m")?,
            shards: NonZeroUsize::new(4).unwrap(),
        };
        for (key, value) in values {
            match key.as_str() {
                "memory_size" => config.memory_size = ByteSize::parse("cache", &key, &value)?,
                "disk_path" => config.disk_path = Some(disk_path(&value)?),
                "disk_size" => config.disk_size = ByteSize::parse("cache", &key, &value)?,
                "chunk_size" => config.chunk_size = ByteSize::parse("cache", &key, &value)?,
                "ttl" => config.ttl = HumanDuration::parse("cache", &key, &value)?,
                "shards" => config.shards = positive_usize("cache", &key, &value)?,
                "block_size" | "min_file_size" | "max_file_size" => {
                    return Err(format!(
                        "cache.{key} is no longer supported: the cache stores object \
                         ranges of cache.chunk_size"
                    ))
                }
                _ => return Err(unknown("cache", &key)),
            }
        }
        let chunk = config.chunk_size.get();
        if !CHUNK_RANGE.contains(&chunk) {
            return Err(format!(
                "cache.chunk_size must be between {} and {}",
                Byte(*CHUNK_RANGE.start() as u64),
                Byte(*CHUNK_RANGE.end() as u64)
            ));
        }
        if config.memory_size.get() / config.shards < chunk {
            return Err("cache.memory_size must hold one cache.chunk_size per shard".to_string());
        }
        if config.disk_size.get() < MIN_DISK_SIZE {
            return Err(format!(
                "cache.disk_size must be at least {}",
                Byte(MIN_DISK_SIZE as u64)
            ));
        }
        Ok(Some(config))
    }

    pub(crate) fn apply(
        &self,
        op: Operator,
        namespace: Option<&str>,
    ) -> Result<(Operator, CacheGuard), String> {
        let (layer, disk) = match &self.disk_path {
            Some(path) => {
                let disk = self.disk_cache(path.join(namespace.unwrap_or("default")))?;
                (disk.layer.clone(), Some(disk))
            }
            None => (self.layer(self.build(None)?)?, None),
        };
        Ok((op.layer(layer), CacheGuard { _disk: disk }))
    }

    fn disk_cache(&self, dir: PathBuf) -> Result<Arc<DiskCache>, String> {
        let mut caches = DISK_CACHES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cache) = caches.get(&dir).and_then(Weak::upgrade) {
            return Ok(cache);
        }
        caches.retain(|_, cache| cache.strong_count() > 0);

        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&dir)
            .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        // Foyer does not lock its files; one cache per directory keeps it intact.
        let lock =
            File::create(dir.join("LOCK")).map_err(|e| format!("lock {}: {e}", dir.display()))?;
        lock.try_lock().map_err(|e| match e {
            TryLockError::WouldBlock => format!("{} is in use by another process", dir.display()),
            TryLockError::Error(e) => format!("lock {}: {e}", dir.display()),
        })?;

        let hybrid = self.build(Some(&dir))?;
        let cache = Arc::new(DiskCache {
            layer: self.layer(hybrid.clone())?,
            hybrid,
            _lock: lock,
        });
        caches.insert(dir, Arc::downgrade(&cache));
        Ok(cache)
    }

    fn layer(&self, hybrid: Hybrid) -> Result<CacheLayer, String> {
        let cache =
            Operator::new(Foyer::new().cache(hybrid)).map_err(|e| format!("cache build: {e}"))?;
        Ok(CacheLayer::new(cache)
            .map_err(|e| format!("cache build: {e}"))?
            .with_chunk_size(self.chunk_size.get())
            .with_ttl(self.ttl.get()))
    }

    fn build(&self, dir: Option<&Path>) -> Result<Hybrid, String> {
        let memory_size = self.memory_size.get();
        let disk_size = self.disk_size.get();
        let shards = self.shards.get();
        let dir = dir.map(Path::to_path_buf);

        block_on(async move {
            // Write the disk tier on insert so evicted chunks need no extra write.
            let policy = match dir {
                Some(_) => HybridCachePolicy::WriteOnInsertion,
                None => HybridCachePolicy::default(),
            };
            let storage = HybridCacheBuilder::new()
                .with_policy(policy)
                .memory(memory_size)
                .with_shards(shards)
                .with_weighter(|key: &FoyerKey, value: &FoyerValue| key.path.len() + value.len())
                .storage();
            let storage = match dir {
                Some(dir) => {
                    let device = FsDeviceBuilder::new(&dir)
                        .with_capacity(disk_size)
                        .build()
                        .map_err(|e| format!("cache device ({}): {e}", dir.display()))?;
                    storage
                        .with_io_engine_config(PsyncIoEngineConfig::new())
                        .with_engine_config(
                            BlockEngineConfig::new(device)
                                .with_block_size(DISK_BLOCK)
                                // Keep removed entries removed across restarts.
                                .with_tombstone_log(true),
                        )
                }
                None => storage,
            };
            storage
                .build()
                .await
                .map_err(|e| format!("cache build: {e}"))
        })
    }
}

fn disk_path(value: &str) -> Result<PathBuf, String> {
    let path = non_empty_path("cache", "disk_path", value)?;
    if path.is_absolute() && !path.components().any(|c| c == Component::ParentDir) {
        Ok(path)
    } else {
        Err(format!(
            "invalid cache.disk_path='{value}': expected an absolute path without '..'"
        ))
    }
}
