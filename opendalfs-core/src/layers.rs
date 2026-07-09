//! Optional OpenDAL layers applied to an Operator, configured via string
//! key/value options passed across the FFI (see `odop_operator_new`).
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
//!   foyer.enable              bool    — enable the foyer in-memory read cache
//!   foyer.memory_mb           usize   — in-memory cache capacity (default 256)
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
        opts.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
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
    // Currently an in-memory tier (the common case). Disk-tier config is
    // accepted but not yet wired (foyer 0.22 device-builder API); disk keys are
    // reserved so enabling them later needs no UX change.
    if matches!(get("foyer.enable"), Some("true" | "1")) {
        let memory_mb = get("foyer.memory_mb").and_then(|s| s.parse::<usize>().ok()).unwrap_or(256);
        match build_foyer(memory_mb) {
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

/// Build a `FoyerLayer` backed by an in-memory hybrid cache of `memory_mb`
/// megabytes. Runs the async foyer builder on the shared runtime.
fn build_foyer(memory_mb: usize) -> Result<opendal::layers::FoyerLayer, String> {
    use foyer::HybridCacheBuilder;

    const MIB: usize = 1024 * 1024;
    let mem_bytes = memory_mb.max(1) * MIB;

    let cache = block_on(async move {
        HybridCacheBuilder::new()
            .memory(mem_bytes)
            .with_shards(4)
            .storage()
            .build()
            .await
            .map_err(|e| format!("foyer memory build: {e}"))
    })?;

    Ok(opendal::layers::FoyerLayer::new(cache))
}
