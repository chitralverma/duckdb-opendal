//! Reader/writer I/O tuning options (concurrent + chunk), resolved into the
//! operator's effective `io.*` option map before crossing the FFI.
//!
//! These map onto OpenDAL's `reader_with(...).concurrent(n).chunk(sz)` and
//! `writer_with(...).concurrent(n).chunk(sz)` builders (see the OpenDAL
//! performance docs). `concurrent` bounds parallel range reads / multipart
//! writes; `chunk` sets the per-request size. Zero / unset means "leave OpenDAL
//! to its per-service default".
//!
//! Recognized effective keys:
//!   io.read.concurrent    usize — parallel range reads
//!   io.read.chunk         usize — per-read chunk size (bytes)
//!   io.write.concurrent   usize — parallel multipart writes
//!   io.write.chunk        usize — per-write chunk size (bytes)

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
}
