use std::collections::BTreeMap;

use opendal::Operator;

mod cache;
mod io;
mod retry;
mod timeout;
mod value;

pub(crate) use io::IoConfig;

pub(crate) struct OperatorConfig {
    pub(crate) io: IoConfig,
    retry: Option<retry::RetryConfig>,
    timeout: Option<timeout::TimeoutConfig>,
    cache: Option<cache::CacheConfig>,
    cache_namespace: Option<String>,
}

impl OperatorConfig {
    pub(crate) fn parse(
        values: Vec<(String, String)>,
        cache_namespace: Option<String>,
    ) -> Result<Self, String> {
        let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (key, value) in values {
            let Some((section, key)) = key.split_once('.') else {
                return Err(format!("unknown OpenDAL option '{key}'"));
            };
            if !matches!(section, "io" | "retry" | "timeout" | "cache") {
                return Err(format!("unknown OpenDAL option '{section}.{key}'"));
            }
            sections
                .entry(section.to_string())
                .or_default()
                .insert(key.to_string(), value);
        }

        Ok(Self {
            io: IoConfig::parse(sections.remove("io").unwrap_or_default())?,
            retry: retry::RetryConfig::parse(sections.remove("retry").unwrap_or_default())?,
            timeout: timeout::TimeoutConfig::parse(sections.remove("timeout").unwrap_or_default())?,
            cache: cache::CacheConfig::parse(sections.remove("cache").unwrap_or_default())?,
            cache_namespace,
        })
    }

    pub(crate) fn apply_layers(&self, mut op: Operator) -> Operator {
        if let Some(config) = &self.retry {
            op = config.apply(op);
        }
        if let Some(config) = &self.timeout {
            op = config.apply(op);
        }
        op = self.io.apply_layer(op);
        if let Some(config) = &self.cache {
            op = config.apply(op, self.cache_namespace.as_deref());
        }
        op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn parses_sections_into_typed_config() {
        let config = OperatorConfig::parse(
            values(&[
                ("io.read.concurrent", "2"),
                ("io.read.chunk_size", "8 MiB"),
                ("retry.min_delay", "100ms"),
                ("retry.max_delay", "10s"),
                ("timeout.operation_timeout", "1m"),
                ("cache.memory_size", "256 MiB"),
            ]),
            Some("namespace".to_string()),
        )
        .unwrap();
        assert_eq!(config.io.read.concurrent.map(|value| value.get()), Some(2));
        assert_eq!(config.io.read_chunk(), Some(8 * 1024 * 1024));
        assert!(config.retry.is_some());
        assert!(config.timeout.is_some());
        assert!(config.cache.is_some());
    }

    #[test]
    fn rejects_unknown_and_old_keys() {
        for key in [
            "cache.shradz",
            "cache.memory_mb",
            "timeout.seconds",
            "retry.min_delay_ms",
        ] {
            assert!(
                OperatorConfig::parse(values(&[(key, "1")]), None).is_err(),
                "accepted {key}"
            );
        }
    }
}
