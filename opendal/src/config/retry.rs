use std::collections::BTreeMap;

use opendal::layers::RetryLayer;
use opendal::Operator;

use super::value::{boolean, float, unknown, usize_value, HumanDuration};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RetryConfig {
    /// Maximum retries after the initial attempt.
    max_times: Option<usize>,
    /// Exponential backoff multiplier (finite and >= 1).
    factor: Option<f32>,
    /// Add random delay before each retry.
    jitter: bool,
    /// Lower bound for retry delay.
    min_delay: Option<HumanDuration>,
    /// Upper bound for retry delay.
    max_delay: Option<HumanDuration>,
}

impl RetryConfig {
    pub(crate) fn parse(values: BTreeMap<String, String>) -> Result<Option<Self>, String> {
        if values.is_empty() {
            return Ok(None);
        }
        let mut config = Self::default();
        for (key, value) in values {
            match key.as_str() {
                "max_times" => config.max_times = Some(usize_value("retry", &key, &value)?),
                "factor" => {
                    let factor = float("retry", &key, &value)?;
                    if !factor.is_finite() || factor < 1.0 {
                        return Err(format!(
                            "invalid retry.factor='{value}': expected a finite number >= 1"
                        ));
                    }
                    config.factor = Some(factor);
                }
                "jitter" => config.jitter = boolean("retry", &key, &value)?,
                "min_delay" => {
                    config.min_delay = Some(HumanDuration::parse("retry", &key, &value)?)
                }
                "max_delay" => {
                    config.max_delay = Some(HumanDuration::parse("retry", &key, &value)?)
                }
                _ => return Err(unknown("retry", &key)),
            }
        }
        if matches!((config.min_delay, config.max_delay), (Some(min), Some(max)) if min > max) {
            return Err("retry.min_delay must not exceed retry.max_delay".to_string());
        }
        Ok(Some(config))
    }

    pub(crate) fn apply(&self, op: Operator) -> Operator {
        let mut layer = RetryLayer::new();
        if let Some(value) = self.max_times {
            layer = layer.with_max_times(value);
        }
        if let Some(value) = self.factor {
            layer = layer.with_factor(value);
        }
        if self.jitter {
            layer = layer.with_jitter();
        }
        if let Some(value) = self.min_delay {
            layer = layer.with_min_delay(value.get());
        }
        if let Some(value) = self.max_delay {
            layer = layer.with_max_delay(value.get());
        }
        op.layer(layer)
    }
}
