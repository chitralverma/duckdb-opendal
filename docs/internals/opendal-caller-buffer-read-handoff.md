# OpenDAL caller-owned buffer reads: issue/RFC handoff

## Purpose

This document is input for an OpenDAL community discussion, issue, or RFC. It
describes an API mismatch found while building the `duckdb-opendal` DuckDB
filesystem extension and asks whether OpenDAL can support reads directly into a
caller-owned buffer.

No API shape below is a final proposal. The goal is to validate feasibility and
identify the correct OpenDAL abstraction level.

## Problem statement

DuckDB's C++ `FileSystem::Read` contract supplies a destination pointer and
requires the filesystem to fill that memory:

```cpp
void Read(FileHandle &handle, void *buffer, int64_t nr_bytes, idx_t location);
```

The extension bridges this call to OpenDAL Rust through C FFI. OpenDAL's reader
pipeline yields owned `Buffer`/`Bytes` chunks. Even `Reader::read_into`, which is
the best currently available API for this integration, streams those chunks and
copies them into the caller's `BufMut`:

```rust
pub async fn read_into(
    &self,
    buf: &mut impl BufMut,
    range: impl Into<BytesRange>,
) -> Result<usize> {
    let mut stream = self.clone().into_stream(range).await?;
    let mut read = 0;
    loop {
        let Some(bs) = stream.try_next().await? else {
            return Ok(read);
        };
        read += bs.len();
        buf.put(bs);
    }
}
```

With `&mut [u8]` as `BufMut`, `bytes::BufMut::put_slice` performs
`copy_from_slice` into DuckDB's allocation.

The effective path is therefore:

```text
backend/transport -> OpenDAL Buffer/Bytes -> DuckDB-owned buffer
                                         one memcpy
```

The extension cannot return or substitute OpenDAL's buffer because DuckDB owns
the destination allocation and its `Read` API has no buffer-adoption mechanism.

## Current implementation

`duckdb-opendal` exposes the DuckDB pointer as a fixed-capacity mutable slice and
passes it to `Reader::read_into`:

```rust
let mut dst: &mut [u8] = std::slice::from_raw_parts_mut(buf, len as usize);
let n = reader.read_into(&mut dst, offset..end).await?;
```

This is the minimum-copy implementation available through OpenDAL's current
public APIs. It avoids the additional allocation/flatten/copy that an
implementation based on `Reader::read()` plus `Buffer::to_bytes()` could incur.

It should be described as **one-copy** or **zero-extra-copy**, not zero-copy.

## Why `transmute` or pointer casting cannot solve this

An unsafe cast only changes how a memory address is interpreted. It cannot make
two separate allocations share storage or transfer ownership between runtimes.

```text
OpenDAL-owned allocation: A
DuckDB-owned allocation:  B
```

DuckDB requires bytes to be present at B. Reinterpreting A does not populate B.
The DuckDB method also receives B by value, so the extension cannot replace the
caller's pointer.

Trying to present OpenDAL memory as DuckDB memory would violate one or more of:

- DuckDB's requirement that its supplied destination is filled;
- OpenDAL `Buffer` lifetime and ownership;
- pooled-buffer return semantics;
- `Bytes` sharing/reference counts;
- allocator compatibility;
- aliasing and mutability rules.

Likely outcomes are use-after-free, double free, stale data, or a still-unfilled
DuckDB allocation. This is an API/ownership mismatch, not a type-layout problem.

## Backend-specific impact

### Local filesystem

OpenDAL's fs reader currently obtains a pooled buffer, resizes it, performs the
positioned file read into that buffer, freezes it into `Bytes`, and returns an
OpenDAL `Buffer`:

```text
kernel -> OpenDAL pooled buffer -> DuckDB buffer
                              one memcpy
```

A native DuckDB filesystem can perform:

```text
kernel -> DuckDB buffer
```

The extra copy is most likely to matter for local storage, fast attached
storage, and memory/cache-backed services where network latency does not hide
memory bandwidth costs.

### HTTP/object storage

HTTP transports generally produce owned response-body chunks. Those chunks are
copied into DuckDB's buffer. Network latency often dominates, but the copy may
still matter for high-throughput networks, large scans, many concurrent reads,
or cache hits.

### Foyer cache

Foyer returns cached values as OpenDAL/foyer-owned bytes. Cache hits still copy
into DuckDB's destination. This can make the copy more visible because no remote
round trip masks it.

## Related OpenDAL APIs

### `Reader::read`

Returns OpenDAL `Buffer` and is zero-copy within OpenDAL because it retains
underlying `Bytes`. It cannot satisfy a caller-owned destination contract
without a copy.

### `Reader::read_into`

Accepts `BufMut`, but currently consumes buffers produced by the regular reader
stream and copies them into the destination. It does not pass caller memory down
to the backend or transport.

### `Reader::fetch`

Merges and concurrently fetches multiple ranges, returning `Vec<Buffer>`. It is
useful when the caller owns the output buffers, but does not address a foreign
API that requires preallocated destination memory. DuckDB's generic filesystem
interface also exposes only one contiguous range per `Read` call.

## Open question for the OpenDAL community

Can OpenDAL add an optional caller-buffer read path that lets capable services
or transports write directly into caller-provided memory while preserving the
existing `Buffer`-returning APIs?

The desired property is:

```text
backend/transport -> caller-owned destination
```

without an intermediate OpenDAL-owned data buffer.

## Possible API directions

These are discussion sketches only.

### 1. Reader-level `read_exact_into`

```rust
pub async fn read_exact_into(
    &self,
    dst: &mut [u8],
    range: Range<u64>,
) -> Result<()>;
```

Pros:

- simple high-level integration;
- clear fixed-capacity contract;
- naturally matches `pread` and DuckDB positioned reads.

Question: can this avoid copying if the lower-level service reader still
returns `Buffer`?

### 2. Optional raw reader capability

Add a lower-level operation to `oio` reader interfaces:

```rust
async fn read_at_into(
    handle: &Self::Handle,
    offset: u64,
    dst: &mut [u8],
) -> Result<usize>;
```

Services that support direct destination reads implement it; the raw layer
falls back to existing `read_at` + copy for others.

Pros:

- enables actual zero-extra-copy for fs and compatible services;
- preserves compatibility through fallback;
- capability can be service-specific.

Questions:

- How should the capability be advertised?
- How does it compose with layers?
- Can async trait/lifetime requirements support a borrowed destination safely?
- How should cancellation behave while a backend holds the borrow?

### 3. Sink/callback-based streaming

```rust
pub async fn read_with_sink(
    &self,
    range: BytesRange,
    sink: impl ReadSink,
) -> Result<usize>;
```

The sink exposes writable regions or accepts chunks. This may compose better
with streaming transports, but accepting chunks alone still implies a copy for
caller-owned destinations.

### 4. Mutable-buffer abstraction

OpenDAL could define a safe owned/borrowed destination abstraction capable of
crossing executor tasks. This may be necessary for transports that require an
owned `'static` buffer, but it is more complex and could constrain foreign-memory
integrations.

## Layer and transport concerns

Any proposal should consider:

- retry: a failed attempt may partially modify caller memory;
- timeout/cancellation: borrowed memory must not outlive the future;
- concurrent/chunked reads: disjoint destination slices could be filled in
  parallel;
- cache layers: a hit may still require a copy unless the caller can adopt the
  cached allocation;
- integrity/decompression/encryption layers: transformations may require
  intermediate buffers;
- HTTP transports: common clients expose owned `Bytes` chunks rather than a
  caller-provided body destination;
- short reads and EOF semantics;
- initialization safety for `MaybeUninit<u8>` versus initialized `&mut [u8]`;
- service capability reporting and fallback behavior.

## Suggested incremental scope

A pragmatic first RFC could target positioned reads for the fs service:

1. Add an optional lower-level caller-buffer read method.
2. Implement it for fs using direct positioned reads into the destination.
3. Provide a generic fallback through existing `Buffer` reads and one copy.
4. Expose a high-level `Reader` method.
5. Benchmark and validate semantics before extending transports/services.

This would prove the abstraction without requiring every backend to change.

## Benchmark plan

Compare current `read_into` with a caller-buffer prototype for:

1. fs sequential reads at 64 KiB, 1 MiB, 8 MiB, and 64 MiB;
2. fs random positioned reads representative of Parquet column chunks;
3. concurrent scans across multiple reader handles;
4. memory service and foyer memory-cache hits;
5. CPU time, cycles, memory bandwidth, allocations, and wall time;
6. remote S3 as a control where network latency may dominate.

Include native DuckDB local filesystem reads as the lower-bound comparison.

## Acceptance criteria for a useful API

- Caller supplies the destination memory.
- At least one backend writes directly into it without an intermediate data
  allocation/copy.
- Safe lifetime and cancellation semantics.
- Clear short-read behavior.
- Existing `read`/`read_into` behavior remains compatible.
- Generic fallback for unsupported services/layers.
- No requirement that foreign callers adopt OpenDAL's allocator or ownership.

## Source references used in this investigation

- OpenDAL 0.58 `types/read/reader.rs`:
  - `Reader::read` retains `Bytes` in `Buffer`;
  - `Reader::read_into` iterates the reader stream and calls `BufMut::put`;
  - `Reader::fetch` returns `Vec<Buffer>`.
- OpenDAL fs 0.58 `reader.rs`:
  - `PositionRead::read_at` reads into an OpenDAL pooled buffer and returns
    `Buffer`.
- `bytes` `BufMut for &mut [u8]`:
  - `put_slice` uses `copy_from_slice`.
- DuckDB 1.5.4 `FileSystem::Read`:
  - caller supplies `void *buffer` and expects it to be filled.
- `duckdb-opendal/opendal/src/reader.rs`:
  - wraps the DuckDB destination as `&mut [u8]` and calls OpenDAL
    `Reader::read_into`.
