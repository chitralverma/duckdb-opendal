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
        let bytes = parse_bytes(value).ok_or_else(|| {
            invalid(
                section,
                key,
                value,
                "expected bytes or a size such as '8 MB', '8 MiB', or '64 Mib'",
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

fn parse_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(bytes) = value.parse::<u64>() {
        return Some(bytes);
    }
    if let Ok(bytes) = value.parse::<Byte>() {
        return Some(bytes.0);
    }

    let split = value.rfind(char::is_numeric)?;
    let number = value[..=split].trim().parse::<u64>().ok()?;
    let unit = value[split + 1..].trim();
    let (factor, bits) = match unit {
        "B" => (1, false),
        "kB" | "KB" => (1_000, false),
        "MB" => (1_000_000, false),
        "GB" => (1_000_000_000, false),
        "TB" => (1_000_000_000_000, false),
        "PB" => (1_000_000_000_000_000, false),
        "EB" => (1_000_000_000_000_000_000, false),
        "b" => (1, true),
        "kb" => (1_000, true),
        "Mb" => (1_000_000, true),
        "Gb" => (1_000_000_000, true),
        "Tb" => (1_000_000_000_000, true),
        "Kib" => (1_u64 << 10, true),
        "Mib" => (1_u64 << 20, true),
        "Gib" => (1_u64 << 30, true),
        "Tib" => (1_u64 << 40, true),
        "Pib" => (1_u64 << 50, true),
        "Eib" => (1_u64 << 60, true),
        _ => return None,
    };
    let value = number.checked_mul(factor)?;
    if bits {
        (value % 8 == 0).then_some(value / 8)
    } else {
        Some(value)
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
            ByteSize::parse("io", "chunk_size", "1 MB").unwrap().get(),
            1_000_000
        );
        assert_eq!(
            ByteSize::parse("io", "chunk_size", "8 Mb").unwrap().get(),
            1_000_000
        );
        assert_eq!(
            ByteSize::parse("io", "chunk_size", "8 Mib").unwrap().get(),
            1_048_576
        );
        assert!(ByteSize::parse("io", "chunk_size", "1 b").is_err());

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
