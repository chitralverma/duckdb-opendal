//! Shared Tokio runtime for bridging OpenDAL's async API to DuckDB's sync
//! FileSystem API.
//!
//! OpenDAL's `Operator` is async-only. Every FFI entry that performs I/O runs
//! its future on a single, process-wide **multi-thread** Tokio runtime via
//! `block_on`. A multi-thread runtime is required so that multiple DuckDB
//! worker threads calling in concurrently do not serialize on a single-threaded
//! executor (see plan risk R-async). We must never call `block_on` from within
//! a future already running on this runtime (no nested block_on).

use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Access the shared multi-thread Tokio runtime, initializing it on first use.
pub(crate) fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .thread_name("duckdb-opendal")
            .build()
            .expect("failed to build duckdb-opendal Tokio runtime")
    })
}

/// Run an async block to completion on the shared runtime.
pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}
