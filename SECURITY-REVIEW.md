# Milestone 1.7.1 — Security, FFI and Execution Review

Milestone 1.7.1 inherits every WAL/page/B+Tree/checkpoint/recovery hardening item from 1.6.1.

## Fixed 1.7-specific issues

### Oversized FFI input declarations
`adb_open` rejects paths larger than 64 KiB and `adb_execute_plan_json` rejects plans larger
than 8 MiB before constructing Rust slices. Fallible functions clear output handles first.

### Batch wire narrowing and malformed batches
ADB Batch v1 encoding now checks row count, column count, bitmap length and payload lengths
before conversion to u32, verifies every column has exactly the batch row count, and caps the
encoded batch at 64 MiB.

### Per-batch execution memory boundary
`QueryCursor` estimates batch-owned heap memory and rejects a batch exceeding the configured
query memory limit instead of blindly returning it.

### Panic boundary
FFI operations remain wrapped by `catch_unwind`; an internal Rust panic is converted to
`ADB_INTERNAL` instead of unwinding through C.

## Important residual C ABI risk
Raw pointers are inherently unsafe. The library can check nullness and declared length, but it
cannot prove that a non-null pointer actually references that many readable/writable bytes.
Likewise, `Box::from_raw` requires each opaque handle to be released exactly once. Therefore:
- forged/dangling pointers can cause undefined behavior,
- double close/release can cause undefined behavior,
- use-after-release can cause undefined behavior.

This ABI is appropriate only for a trusted native adapter obeying the header contract. A future
hardening path is a Rust-owned registry with generational integer handles.

## Other residual risks
- Full Scan still captures the complete stable logical row set before producing batches. The
  new per-batch limit does not prevent the initial snapshot collection from becoming large.
- JSON plan parsing is transitional; serde_json has recursion protection and the plan has a
  depth validator, but protobuf/binary protocol is a better long-term boundary.
- FFI exposes arbitrary database filesystem paths to the trusted host process; it is not a
  sandbox boundary.
- No authentication/authorization exists in 1.7; that belongs to later security milestones.
- `adb_last_error_ptr` is borrowed thread-local memory and becomes invalid after the next error
  on the same thread.
- CRC32 is accidental-corruption detection, not adversarial integrity protection.
