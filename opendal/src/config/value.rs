use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use human_units::iec::Byte;

pub(crate) fn positive_usize(
    section: &str,
    key: &str,
    value: &str,
) -> Result<NonZeroUsize, String> {
    value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| invalid(section, key, value, "expected a positive integer"))
}

pub(crate) fn usize_value(section: &str, key: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| invalid(section, key, value, "expected a non-negative integer"))
}

pub(crate) fn float(section: &str, key: &str, value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .map_err(|_| invalid(section, key, value, "expected a number"))
}

pub(crate) fn boolean(section: &str, key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(invalid(
            section,
            key,
            value,
            "expected true, false, 1, or 0",
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ByteSize(usize);

impl ByteSize {
    pub(crate) fn parse(section: &str, key: &str, value: &str) -> Result<Self, String> {
        let bytes = value
            .parse::<u64>()
            .or_else(|_| value.parse::<Byte>().map(|value| value.0))
            .map_err(|_| {
                invalid(
                    section,
                    key,
                    value,
                    "expected bytes or an IEC size such as '8 MiB'",
                )
            })?;
        usize::try_from(bytes)
            .map(Self)
            .map_err(|_| format!("{section}.{key} exceeds the platform size limit"))
    }

    pub(crate) fn get(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HumanDuration(Duration);

impl HumanDuration {
    pub(crate) fn parse(section: &str, key: &str, value: &str) -> Result<Self, String> {
        value
            .parse::<human_units::Duration>()
            .map(|value| Self(value.0))
            .map_err(|_| {
                invalid(
                    section,
                    key,
                    value,
                    "expected seconds or a duration such as '500ms'",
                )
            })
    }

    pub(crate) fn get(self) -> Duration {
        self.0
    }
}

pub(crate) fn non_empty_path(section: &str, key: &str, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        Err(invalid(section, key, value, "path must not be empty"))
    } else {
        Ok(PathBuf::from(value))
    }
}

pub(crate) fn unknown(section: &str, key: &str) -> String {
    format!("unknown OpenDAL option '{section}.{key}'")
}

fn invalid(section: &str, key: &str, value: &str, expected: &str) -> String {
    format!("invalid {section}.{key}='{value}': {expected}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_and_human_units() {
        assert_eq!(
            ByteSize::parse("io", "chunk_size", "1048576")
                .unwrap()
                .get(),
            1_048_576
        );
        assert_eq!(
            ByteSize::parse("io", "chunk_size", "1 MiB").unwrap().get(),
            1_048_576
        );
        assert_eq!(
            ByteSize::parse("io", "chunk_size", "256 KiB")
                .unwrap()
                .get(),
            262_144
        );

        assert_eq!(
            HumanDuration::parse("timeout", "io_timeout", "15")
                .unwrap()
                .get(),
            Duration::from_secs(15)
        );
        assert_eq!(
            HumanDuration::parse("timeout", "io_timeout", "500ms")
                .unwrap()
                .get(),
            Duration::from_millis(500)
        );
        assert_eq!(
            HumanDuration::parse("timeout", "operation_timeout", "2m")
                .unwrap()
                .get(),
            Duration::from_secs(120)
        );
    }
}
