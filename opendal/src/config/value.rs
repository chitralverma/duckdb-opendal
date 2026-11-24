use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use human_units::iec::{iec_unit, Byte, Prefix};
use human_units::si::si_unit;

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

    pub(crate) const fn from_bytes(bytes: usize) -> Self {
        Self(bytes)
    }

    pub(crate) fn get(self) -> usize {
        self.0
    }
}

/// `value` in IEC units, e.g. `iec(8, Prefix::Mebi)` is 8 MiB.
pub(crate) const fn iec(value: u64, prefix: Prefix) -> usize {
    Byte::with_iec_prefix(value, prefix).0 as usize
}

/// Bytes in powers of 1000 (`kB`, `MB`, ...).
#[si_unit(symbol = "B", min_prefix = "")]
struct SiBytes(u64);

/// Bits in powers of 1000 (`kb`, `Mb`, ...).
#[si_unit(symbol = "b", min_prefix = "")]
struct SiBits(u64);

/// Bits in powers of 1024 (`Kib`, `Mib`, ...).
#[iec_unit(symbol = "b")]
struct IecBits(u64);

fn parse_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    // `KB` is the common spelling of SI `kB`.
    let value = match value.strip_suffix("KB") {
        Some(number) => format!("{number}kB"),
        None => value.to_string(),
    };
    if let Some(bytes) = value
        .parse::<u64>()
        .ok()
        .or_else(|| value.parse::<Byte>().ok().map(|v| v.0))
        .or_else(|| value.parse::<SiBytes>().ok().map(|v| v.0))
    {
        return Some(bytes);
    }
    let bits = value
        .parse::<SiBits>()
        .map(|v| v.0)
        .or_else(|_| value.parse::<IecBits>().map(|v| v.0))
        .ok()?;
    (bits % 8 == 0).then_some(bits / 8)
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

    pub(crate) fn is_zero(self) -> bool {
        self.0.is_zero()
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
        for (value, bytes) in [
            ("2 KB", 2_000),
            ("2 kB", 2_000),
            ("16 kb", 2_000),
            ("3 GB", 3_000_000_000),
        ] {
            assert_eq!(
                ByteSize::parse("io", "chunk_size", value).unwrap().get(),
                bytes,
                "{value}"
            );
        }
        for value in ["1 mB", "1 Kb", "1 MIB", "99999999999 EB"] {
            assert!(
                ByteSize::parse("io", "chunk_size", value).is_err(),
                "{value}"
            );
        }

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
