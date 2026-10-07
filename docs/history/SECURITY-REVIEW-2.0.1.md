# Milestone 2.0.1 — Security, Correctness and Durability Review

## Summary

Milestone 2.0 introduced a substantially larger trust boundary than 1.7: SQL text enters a Scala
parser/binder/planner, metadata is persisted by the JVM catalog, plans and mutations cross Java FFM,
and Rust decodes and executes those requests. The original implementation was suitable as a
functional milestone but contained several denial-of-service, corruption-recovery and native-boundary
risks that required hardening.

## Fixed / materially reduced risks

### HIGH — segmented WAL crash tail and oversized frame lengths
Inherited 1.7.1 hardening truncates only an incomplete suffix of the final segment before append and
rejects oversized payload lengths before allocation. Fully framed corruption remains a hard error.

### HIGH — corrupt page/B+Tree data could panic
Current and temporal B+Tree codecs now validate counts, shape, ordering and all byte ranges. Hardened
pages carry CRC32 and common header/heap slot invariants are validated before higher-level decoding.

### HIGH — unbounded mutation JSON at the C ABI
Original 2.0 bounded neither INSERT row JSON nor UPDATE assignment JSON. A native caller could claim
a huge length, causing excessive parsing/allocation after `from_raw_parts`. 2.0.1 rejects zero/oversized
payloads before constructing the Rust slice.

### HIGH — malformed ADB Batch metadata in the JVM adapter
The original Java decoder trusted rows, columns, bitmap lengths, payload lengths and variable offsets.
Malformed native bytes could cause very large Java allocations, BufferUnderflowException, or invalid
range copies. The hardened decoder bounds and validates every structural quantity first.

### HIGH — FileCatalog durability and corruption handling
The original FileCatalog wrote a temporary properties file then renamed it, but did not force file data
or the parent directory and trusted persisted entity/field counts and conversions. 2.0.1 adds bounded
restore, semantic catalog validation, deterministic SHA-256 integrity, file force, atomic rename and
parent-directory force. A failed publication reloads the last durable snapshot into memory.

### MEDIUM — SQL parser resource exhaustion / narrowing
The original parser accepted unbounded input and recursive EXPLAIN nesting and narrowed LIMIT using
`Long.toInt`. 2.0.1 applies lexical/nesting budgets, iterative EXPLAIN wrapping and explicit LIMIT range
validation.

### MEDIUM — JVM gateway defeats native batching by materializing every result row
The Milestone 2 API returns a complete `QueryResult`, so an unlimited SELECT can accumulate large JVM
memory even though Rust returns batches. 2.0.1 caps gateway materialization at one million rows and
requires callers to use LIMIT for larger results.

### RESOLVED — Gradle/JDK/Scala incompatibility
The first 2.0.1 hardening build used Scala 3.3.3 and therefore had to emit Java 21 bytecode while
running on JDK 22. The corrected demo build upgrades every Scala module to Scala 3.3.8, which supports
targeting JDK 22 directly. Java and Scala now share target level 22; the explicit JUnit Platform
launcher remains for Gradle 9.x test workers.

## Residual risks / intentional Milestone 2.0 scope limits

### Raw C ABI remains caller-unsafe
The Rust FFI can validate null pointers and lengths, but it cannot prove that an arbitrary non-null C
pointer refers to readable memory for the claimed length. Likewise, double-closing a raw opaque C
handle from an invalid native caller can still cause undefined behavior. Eliminating this class fully
requires an ABI redesign (for example generation-checked numeric handles/registry) rather than local
null/length checks.

### Rust scan path is still not fully streaming from storage
The early execution architecture can materialize visible rows before producing batches. The JVM result
cap prevents unbounded gateway accumulation, but does not turn the underlying table scan into a fully
bounded page iterator. A later execution milestone should provide true streaming storage scans.

### Catalog has no multi-process writer coordination
`FileCatalog` is synchronized within one JVM instance, but Milestone 2.0 does not define a database-wide
multi-process catalog lock. Two independent JVM processes writing the same catalog/data directory are
outside the supported model and could overwrite metadata. Production deployment should enforce one
writer process until an explicit lock/coordination layer exists.

### Catalog checksum is integrity detection, not authenticity
SHA-256 detects accidental or unsophisticated modification but is not keyed. A process/user with write
access to the database directory can alter the catalog and recompute the digest. Filesystem ownership,
permissions and later authentication/authorization remain required.

### Single active metadata file
Catalog/root/checkpoint metadata in this historical line does not yet use every later dual-slot or
generation fallback mechanism. Some fully corrupted latest metadata files therefore fail open rather
than automatically falling back to a previous valid generation.

### No authentication / authorization / network TLS
Milestone 2.0 is still an embedded/local control-plane milestone. User/role authentication and RBAC are
introduced much later (3.0), and node-to-node mTLS later still. Absence of those features in 2.0.1 is a
scope limit, not a production security claim.

### SQL subset intentionally has coarse error handling
Malformed SQL produces `IllegalArgumentException`; this is acceptable at this layer but not a complete
client protocol error taxonomy.

## Overall assessment

2.0.1 is materially safer than the original 2.0 for local development, testing and architectural
experimentation. It is still not a production multi-user database security boundary. The most important
remaining architectural risks are raw-C handle ownership, non-streaming scan materialization and the
single-process metadata-writer assumption.
