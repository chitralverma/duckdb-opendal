use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use opendal::layers::ConcurrentLimitLayer;
use opendal::Operator;

use super::value::{positive_usize, unknown, ByteSize};

/// Services served over a network, where per-request latency dominates.
fn is_remote(scheme: &str) -> bool {
    !matches!(scheme, "fs" | "memory")
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DirectionConfig {
    /// Per-operation request fan-out.
    pub(crate) concurrent: Option<NonZeroUsize>,
    /// Per-request chunk size in bytes.
    pub(crate) chunk_size: Option<ByteSize>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct IoConfig {
    pub(crate) read: DirectionConfig,
    pub(crate) write: DirectionConfig,
    /// Shared operator-wide ceiling across all operations.
    concurrent_limit: Option<NonZeroUsize>,
}

impl IoConfig {
    pub(crate) fn parse(values: BTreeMap<String, String>) -> Result<Self, String> {
        let mut config = Self::default();
        for (key, value) in values {
            match key.as_str() {
                "read.concurrent" => {
                    config.read.concurrent = Some(positive_usize("io", &key, &value)?)
                }
                "read.chunk_size" => {
                    let size = ByteSize::parse("io", &key, &value)?;
                    if size.get() == 0 {
                        return Err(
                            "invalid io.read.chunk_size='0': expected a positive size".to_string()
                        );
                    }
                    config.read.chunk_size = Some(size);
                }
                "write.concurrent" => {
                    config.write.concurrent = Some(positive_usize("io", &key, &value)?)
                }
                "write.chunk_size" => {
                    let size = ByteSize::parse("io", &key, &value)?;
                    if size.get() == 0 {
                        return Err(
                            "invalid io.write.chunk_size='0': expected a positive size".to_string()
                        );
                    }
                    config.write.chunk_size = Some(size);
                }
                "concurrent_limit" => {
                    config.concurrent_limit = Some(positive_usize("io", &key, &value)?)
                }
                _ => return Err(unknown("io", &key)),
            }
        }
        Ok(config)
    }

    /// Fill unset fields with defaults suited to the service. Remote object
    /// stores pay a round trip per request, so reads prefetch in larger
    /// concurrent chunks and writes upload parts concurrently; local and
    /// in-memory services keep OpenDAL's own defaults. Explicit user settings
    /// always win.
    pub(crate) fn with_service_defaults(mut self, scheme: &str) -> Self {
        const REMOTE_READ_CHUNK: ByteSize = ByteSize::from_bytes(8 * 1024 * 1024);
        const REMOTE_CONCURRENT: usize = 4;
        if !is_remote(scheme) {
            return self;
        }
        let concurrent = NonZeroUsize::new(REMOTE_CONCURRENT);
        if self.read.concurrent.is_none() {
            self.read.concurrent = concurrent;
        }
        if self.read.chunk_size.is_none() {
            self.read.chunk_size = Some(REMOTE_READ_CHUNK);
        }
        if self.write.concurrent.is_none() {
            self.write.concurrent = concurrent;
        }
        self
    }

    pub(crate) fn apply_layer(&self, op: Operator) -> Operator {
        match self.concurrent_limit {
            Some(limit) => op.layer(ConcurrentLimitLayer::new(limit.get())),
            None => op,
        }
    }

    pub(crate) fn read_chunk(&self) -> Option<usize> {
        self.read.chunk_size.map(ByteSize::get)
    }

    pub(crate) fn write_chunk(&self) -> Option<usize> {
        self.write.chunk_size.map(ByteSize::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(values: &[(&str, &str)]) -> IoConfig {
        IoConfig::parse(
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn remote_services_get_prefetch_defaults() {
        let io = parsed(&[]).with_service_defaults("s3");
        assert_eq!(io.read.concurrent.map(|v| v.get()), Some(4));
        assert_eq!(io.read_chunk(), Some(8 * 1024 * 1024));
        assert_eq!(io.write.concurrent.map(|v| v.get()), Some(4));
    }

    #[test]
    fn local_services_keep_opendal_defaults() {
        for scheme in ["fs", "memory"] {
            let io = parsed(&[]).with_service_defaults(scheme);
            assert!(io.read.concurrent.is_none(), "{scheme}");
            assert!(io.read_chunk().is_none(), "{scheme}");
            assert!(io.write.concurrent.is_none(), "{scheme}");
        }
    }

    #[test]
    fn explicit_settings_win_over_defaults() {
        let io = parsed(&[("read.concurrent", "1"), ("read.chunk_size", "1 MiB")])
            .with_service_defaults("s3");
        assert_eq!(io.read.concurrent.map(|v| v.get()), Some(1));
        assert_eq!(io.read_chunk(), Some(1024 * 1024));
        assert_eq!(io.write.concurrent.map(|v| v.get()), Some(4));
    }
}
