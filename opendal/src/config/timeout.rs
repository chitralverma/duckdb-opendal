use std::collections::BTreeMap;

use opendal::layers::TimeoutLayer;
use opendal::Operator;

use super::value::{unknown, HumanDuration};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TimeoutConfig {
    /// Timeout for control operations such as stat/delete.
    operation_timeout: Option<HumanDuration>,
    /// Timeout for I/O operations and body methods.
    io_timeout: Option<HumanDuration>,
}

impl TimeoutConfig {
    pub(crate) fn parse(values: BTreeMap<String, String>) -> Result<Option<Self>, String> {
        if values.is_empty() {
            return Ok(None);
        }
        let mut config = Self::default();
        for (key, value) in values {
            let parsed = HumanDuration::parse("timeout", &key, &value)?;
            if parsed.is_zero() {
                return Err(format!(
                    "invalid timeout.{key}='{value}': duration must be positive"
                ));
            }
            match key.as_str() {
                "operation_timeout" => config.operation_timeout = Some(parsed),
                "io_timeout" => config.io_timeout = Some(parsed),
                _ => return Err(unknown("timeout", &key)),
            }
        }
        Ok(Some(config))
    }

    pub(crate) fn apply(&self, op: Operator) -> Operator {
        let mut layer = TimeoutLayer::new();
        if let Some(value) = self.operation_timeout {
            layer = layer.with_timeout(value.get());
        }
        if let Some(value) = self.io_timeout {
            layer = layer.with_io_timeout(value.get());
        }
        op.layer(layer)
    }
}
