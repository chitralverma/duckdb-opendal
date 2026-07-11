//! Reader/writer I/O tuning options (concurrent + chunk), configurable per
//! operator (via the secret `layers` map, reserved `io.*` keys) and globally
//! (across all operators, via `odop_set_global_io_options`).
//!
//! These map onto OpenDAL's `reader_with(...).concurrent(n).chunk(sz)` and
//! `writer_with(...).concurrent(n).chunk(sz)` builders (see the OpenDAL
//! performance docs). `concurrent` bounds parallel range reads / multipart
//! writes; `chunk` sets the per-request size. Zero / unset means "leave OpenDAL
//! to its per-service default".
//!
//! Recognized keys (all optional, under the `layers` MAP):
//!   io.read.concurrent    usize — parallel range reads
//!   io.read.chunk         usize — per-read chunk size (bytes)
//!   io.write.concurrent   usize — parallel multipart writes
//!   io.write.chunk        usize — per-write chunk size (bytes)

use std::sync::RwLock;

/// Per-direction tuning. `0` (the default) means "unset — use OpenDAL's
/// per-service default".
#[derive(Clone, Copy, Default)]
pub(crate) struct DirOptions {
    pub concurrent: usize,
    pub chunk: usize,
}

/// Reader + writer I/O options for an operator.
#[derive(Clone, Copy, Default)]
pub(crate) struct IoOptions {
    pub read: DirOptions,
    pub write: DirOptions,
}

impl IoOptions {
    /// Parse `io.*` keys out of the operator's layer option list. Unrecognized
    /// keys are ignored; malformed values leave that field at its default.
    pub(crate) fn from_opts(opts: &[(String, String)]) -> Self {
        let mut io = IoOptions::default();
        for (k, v) in opts {
            let parsed = v.parse::<usize>().ok();
            match k.as_str() {
                "io.read.concurrent" => io.read.concurrent = parsed.unwrap_or(0),
                "io.read.chunk" => io.read.chunk = parsed.unwrap_or(0),
                "io.write.concurrent" => io.write.concurrent = parsed.unwrap_or(0),
                "io.write.chunk" => io.write.chunk = parsed.unwrap_or(0),
                _ => {}
            }
        }
        io
    }

    /// Fill any unset (zero) field from `base` (the global defaults). Per-
    /// operator values take precedence over globals.
    pub(crate) fn with_defaults(mut self, base: &IoOptions) -> Self {
        if self.read.concurrent == 0 {
            self.read.concurrent = base.read.concurrent;
        }
        if self.read.chunk == 0 {
            self.read.chunk = base.read.chunk;
        }
        if self.write.concurrent == 0 {
            self.write.concurrent = base.write.concurrent;
        }
        if self.write.chunk == 0 {
            self.write.chunk = base.write.chunk;
        }
        self
    }
}

/// Process-global I/O option defaults, applied to every operator unless it sets
/// its own values. Written via `odop_set_global_io_options`.
static GLOBAL_IO: RwLock<IoOptions> = RwLock::new(IoOptions {
    read: DirOptions {
        concurrent: 0,
        chunk: 0,
    },
    write: DirOptions {
        concurrent: 0,
        chunk: 0,
    },
});

/// Snapshot of the global I/O defaults.
pub(crate) fn global() -> IoOptions {
    *GLOBAL_IO.read().unwrap()
}

/// Set the process-global I/O defaults. A field of `0` means "unset".
///
/// # Safety
/// Plain scalar FFI; always safe. Panic-guarded by the caller.
#[no_mangle]
pub unsafe extern "C" fn odop_set_global_io_options(
    read_concurrent: usize,
    read_chunk: usize,
    write_concurrent: usize,
    write_chunk: usize,
) {
    let _ = std::panic::catch_unwind(|| {
        let mut g = GLOBAL_IO.write().unwrap();
        *g = IoOptions {
            read: DirOptions {
                concurrent: read_concurrent,
                chunk: read_chunk,
            },
            write: DirOptions {
                concurrent: write_concurrent,
                chunk: write_chunk,
            },
        };
    });
}
