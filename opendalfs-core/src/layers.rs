//! Optional OpenDAL layers applied to an Operator, configured via string
//! key/value options passed across the FFI (see `od_operator_new`).
//!
//! Recognized keys (all optional):
//!   retry.max_times           usize   — max retry attempts
//!   retry.factor              f32     — backoff multiplier
//!   retry.jitter              bool    — add random jitter
//!   retry.min_delay_ms        u64     — min backoff delay
//!   retry.max_delay_ms        u64     — max backoff delay
//!   timeout.seconds           u64     — per-operation timeout
//!   timeout.io_seconds        u64     — per-IO-chunk timeout
//!   concurrent_limit          usize   — max concurrent requests
//!   foyer.enable              bool    — enable the foyer hybrid read cache
//!   foyer.memory_mb           usize   — in-memory cache capacity (default 256)
//!   foyer.disk_path           string  — directory for the on-disk cache (opt)
//!   foyer.disk_mb             usize   — on-disk cache capacity (default 1024)
//!   foyer.block_mb            usize   — on-disk block size (default 4)
//!
//! I/O tuning (`io.*`) keys are also carried on this map but consumed by
//! `io.rs` (not a layer): `io.read.concurrent`, `io.read.chunk`,
//! `io.write.concurrent`, `io.write.chunk`.
//!
//! Unknown keys are ignored (forward-compatible). Applying no options returns
//! the operator unchanged.

use std::time::Duration;

use opendal::layers::{ConcurrentLimitLayer, RetryLayer, TimeoutLayer};
use opendal::Operator;

use crate::runtime::block_on;

/// Apply layers described by `opts` (key → value) to `op`, returning the
/// layered operator. Keys are parsed leniently; a malformed value for a key
/// causes that single option to be skipped.
pub(crate) fn apply_layers(mut op: Operator, opts: &[(String, String)]) -> Operator {
    // Collect into a small lookup for convenience.
    let get = |name: &str| -> Option<&str> {
        opts.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    // ── Retry ────────────────────────────────────────────────────────────────
    let retry_keys = [
        "retry.max_times",
        "retry.factor",
        "retry.jitter",
        "retry.min_delay_ms",
        "retry.max_delay_ms",
    ];
    if retry_keys.iter().any(|k| get(k).is_some()) {
        let mut retry = RetryLayer::new();
        if let Some(v) = get("retry.max_times").and_then(|s| s.parse::<usize>().ok()) {
            retry = retry.with_max_times(v);
        }
        if let Some(v) = get("retry.factor").and_then(|s| s.parse::<f32>().ok()) {
            retry = retry.with_factor(v);
        }
        if matches!(get("retry.jitter"), Some("true" | "1")) {
            retry = retry.with_jitter();
        }
        if let Some(v) = get("retry.min_delay_ms").and_then(|s| s.parse::<u64>().ok()) {
            retry = retry.with_min_delay(Duration::from_millis(v));
        }
        if let Some(v) = get("retry.max_delay_ms").and_then(|s| s.parse::<u64>().ok()) {
            retry = retry.with_max_delay(Duration::from_millis(v));
        }
        op = op.layer(retry);
    }

    // ── Timeout ──────────────────────────────────────────────────────────────
    if get("timeout.seconds").is_some() || get("timeout.io_seconds").is_some() {
        let mut timeout = TimeoutLayer::new();
        if let Some(v) = get("timeout.seconds").and_then(|s| s.parse::<u64>().ok()) {
            timeout = timeout.with_timeout(Duration::from_secs(v));
        }
        if let Some(v) = get("timeout.io_seconds").and_then(|s| s.parse::<u64>().ok()) {
            timeout = timeout.with_io_timeout(Duration::from_secs(v));
        }
        op = op.layer(timeout);
    }

    // ── Concurrent limit ─────────────────────────────────────────────────────
    if let Some(v) = get("concurrent_limit").and_then(|s| s.parse::<usize>().ok()) {
        op = op.layer(ConcurrentLimitLayer::new(v));
    }

    // ── Foyer hybrid read cache ──────────────────────────────────────────────
    // In-memory tier always; on-disk tier when `foyer.disk_path` is set (a
    // persistent cache comparable to external caching extensions).
    if matches!(get("foyer.enable"), Some("true" | "1")) {
        let memory_mb = get("foyer.memory_mb")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(256);
        let disk_path = get("foyer.disk_path").map(|s| s.to_string());
        let disk_mb = get("foyer.disk_mb")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1024);
        let block_mb = get("foyer.block_mb")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(4);
        match build_foyer(memory_mb, disk_path, disk_mb, block_mb) {
            Ok(layer) => op = op.layer(layer),
            Err(e) => {
                // Caching is best-effort: log and continue without it rather than
                // failing the whole operator.
                eprintln!("[opendalfs] foyer cache disabled ({e})");
            }
        }
    }

    op
}

/// Build a `FoyerLayer` with an in-memory tier of `memory_mb` MB and, when
/// `disk_path` is set, an on-disk tier of `disk_mb` MB with `block_mb` blocks.
/// Runs the async foyer builder on the shared runtime.
fn build_foyer(
    memory_mb: usize,
    disk_path: Option<String>,
    disk_mb: usize,
    block_mb: usize,
) -> Result<opendal::layers::FoyerLayer, String> {
    // `DeviceBuilder` brings `FsDeviceBuilder::build()` into scope.
    use foyer::{
        BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder, PsyncIoEngineConfig,
    };

    const MIB: usize = 1024 * 1024;
    let mem_bytes = memory_mb.max(1) * MIB;

    let cache = block_on(async move {
        let mem = HybridCacheBuilder::new().memory(mem_bytes).with_shards(4);
        match disk_path {
            Some(dir) => {
                std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {dir}: {e}"))?;
                let device = FsDeviceBuilder::new(&dir)
                    .with_capacity(disk_mb.max(1) * MIB)
                    .build()
                    .map_err(|e| format!("foyer device ({dir}): {e}"))?;
                mem.storage()
                    .with_io_engine_config(PsyncIoEngineConfig::new())
                    .with_engine_config(
                        BlockEngineConfig::new(device).with_block_size(block_mb.max(1) * MIB),
                    )
                    .build()
                    .await
                    .map_err(|e| format!("foyer hybrid build: {e}"))
            }
            None => {
                // Memory-only cache.
                mem.storage()
                    .build()
                    .await
                    .map_err(|e| format!("foyer memory build: {e}"))
            }
        }
    })?;

    Ok(opendal::layers::FoyerLayer::new(cache))
}
