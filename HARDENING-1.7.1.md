# Milestone 1.7.1 — Execution / FFI Hardening

This patch contains the complete Milestone 1.6.1 storage/WAL hardening and preserves the
Milestone 1.7 execution boundary: PhysicalPlan, batch execution and C ABI.

Additional 1.7.1 hardening:
- 64 KiB maximum FFI path input,
- 8 MiB maximum PhysicalPlan JSON input,
- output handles are cleared before fallible FFI operations,
- panic containment remains active at every FFI operation,
- engine conflict/I/O/corruption status mapping is improved,
- batch row/column cardinality validation,
- checked `usize -> u32` wire conversions,
- 64 MiB maximum encoded ADB Batch v1 size,
- per-batch estimated query memory-limit enforcement,
- removal of an internal Scan `expect()` panic path,
- explicit C-header pointer/handle lifetime contract.

Raw C pointers cannot be made memory-safe by length checks alone. A caller that supplies an
invalid non-null pointer, double-releases a handle, or uses a handle after release can still
cause undefined behavior. Eliminating that class requires a different ABI (for example,
generational integer handles backed by a Rust registry).
