use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use opendal::layers::ConcurrentLimitLayer;
use opendal::Operator;

use super::value::{positive_usize, unknown, ByteSize};

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
