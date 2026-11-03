# Milestone 2.5.2.1 — telemetry capture path

2.5.2 made telemetry cheap to leave on. 2.5.2.1 looks at what a **captured** query still costs and
removes what the measurements show to be waste. It adds no feature: no new SQL, no advisor, no
action. One engine function is added (C ABI 9, additive); everything else is in the JVM gateway.

## Why a separate version

2.5.2 had already been measured on the reference machine (Ryzen 9 5950X). Changing its telemetry
path afterwards would have made those results describe different code. The change also adds a C
function and moves the ABI number, which a version that says "ABI 8" must not do. So the work is
2.5.2.1, with its own before and after.

**Numbering.** As for 2.5.1 and 2.5.2, a fix-level number is used for a follow-up at the author's
request. Cargo cannot express four components, so the crates stay at `2.5.2`; the engine version
string (`adb_engine_version`), the JVM build, the scripts and the reports say `2.5.2.1`.

## Where the proposal came from, and what changed

The proposal ("Zero-Allocation Telemetry & Lock-Free Capture Path") asked for a single native
telemetry read, a cached shape id, a bit-mask sampler, a preallocated MPSC ring buffer with
mutable slots, consumer-local and double-buffered aggregation, an isolated advisor thread,
thread-local counters and padding, and a family of benchmarks. Its starting point was a capture
that cost about 76 µs more than the query itself in its own tree.

This repository is not that tree, and its 2.5.2 had already done several of the items: the shape
is computed once at plan time and stored in the cached plan, the sampler decides from scalars with
a power-of-two counter, and the worker sleeps instead of being woken by the producer. So the work
started with a measurement (task 2) rather than a rewrite, and was cut to what it showed:

| Proposal item | Decision | Reason |
|---|---|---|
| One native telemetry struct read through FFM | **done** | the capture fetched, decoded and scanned a JSON profile: about 2 KB allocated and 1–3 µs |
| Zero allocation when skipped | **done** | the known-shape test boxed a `Long` (24 bytes per query); fixed with an allocation-free id set |
| Allocation benchmarks, checked in CI | **done** | allocation is exact on any machine; time on a shared VM is not |
| Cached shape id in the plan | already done in 2.5.2 | `CachedQuery.shape` |
| Bit-mask sampler | already done in 2.5.2 | `fastSampleEvery` is a power of two, the test is a mask |
| MPSC ring with preallocated slots | **not done** | a capture now allocates 144 bytes; the queue offer is 0.3–0.5 µs of 17–20 µs |
| Thread-local counters, padding | **not done** | no contention is visible with the VM's two CPUs; revisit on the reference machine |
| Double-buffered aggregation, advisor on snapshots only | **moved to 2.5.3** | there is no advisor to isolate yet |
| `commit_seq` in the native struct | **not done** | the 2.5.2 event does not carry it, so reading it would be a cost with no use |

## Tasks

Each is one commit.

| # | Task | Result |
|---|---|---|
| 1 | Version 2.5.2.1 in the engine string, the JVM build, benchmarks, demo banners, workflows, scripts | `build-milestone-2.5.2.1.sh`; crates stay `2.5.2` |
| 2 | Split the capture cost and measure allocation per query | profile fetch 0.5–3.6 µs, scan 1.1–2.7 µs, +2344 B per capture, +24 B per skip |
| 3 | Rust: `Cursor::telemetry` and `adb_query_telemetry`, ABI 9 | 32-byte struct, no profile tree, no JSON |
| 4 | Java: `NativeQueryTelemetry` and `NativeQuery.readTelemetry` | reusable segment; `invokeExact` |
| 5 | Gateway reads the counters instead of the profile | profile only for EXPLAIN ANALYZE and the feedback log |
| 6 | Allocation-free known-shape test (`LongIdSet`) | skipped query: +0 bytes |
| 7 | Benchmark: rotate the mode order, report the fastest round | removes the order bias of 2.5.2's comparison |
| 8 | Workload report: telemetry allocation section | `telemetry` object in the JVM report |
| 9 | `check-benchmarks.py`: allocation budget | 64 B skipped, 512 B captured |
| 10 | Smoke test: allocation, ABI 9 counters, EXPLAIN ANALYZE with telemetry on | 6-round smallest-figure comparison |
| 11 | Documentation | this file, telemetry.md, ffi.md, README, CHANGELOG, roadmap, invariants |

## Result

| Per executed cached point lookup | 2.5.2 | 2.5.2.1 |
|---|---|---|
| allocated, skipped by the sampler | +24 B | **+0 B** |
| allocated, captured | +2 344 B | **+144 B** |
| JSON profile document built, copied, decoded | every capture | only EXPLAIN ANALYZE and the feedback log |
| engine calls added by a capture | profile (JSON) | `adb_query_telemetry` (32 bytes) |

The time saved per capture is the profile fetch plus the scan: between 1.6 µs and 6 µs in this
VM's runs. The runs were too noisy to show it as a stable percentage (see
[telemetry.md](telemetry.md#measured)); the reference-machine run is where it will be confirmed or
not, and it should not be assumed.

## Reference-machine results and the 2.5.2 write-path question

Two runs on the Ryzen 9 5950X, same build. Read and planning paths: no change against 2.5.1 (ffi
median -0.8 % over 31 timings). Allocation as measured on the VM: skipped +0 bytes, captured +144
bytes. These are in `examples/results/2.5.2.1-*.json` (the quiet run), with a description in
`examples/results/README.md`.

The 2.5.2 report, and the first 2.5.2.1 report, showed the fsync-bound workloads at about half of
2.5.1 (commits 659 to 67-83 per second, indexed writes 7.6k to 3.8-4.0k rows/s). The code could not
account for it: the diff of 2.5.1 and 2.5.2 over the engine, the Rust benchmark and the scripts
contains only version strings, and the Rust benchmark never loads the JVM, the gateway or
telemetry. The second run, on a quiet disk, with nothing else changed, gave writes without index
6.4k to 41k rows/s, three indexes 3.8k to 14k, single-row commit 74 to 136 per second. So the
variation between two runs of one build is larger than the difference between the builds, and **no
regression was found in 2.5.2 or 2.5.2.1**.

The disk-bound figures of earlier reports are not a baseline. In particular the 659 commits/s of
the 2.5.1 report is an outlier: the single-row commit test in the same report gave 73/s. This is
the most likely reading, not a measured proof; running `compare-write-path.sh` (2.5.1 and the
current build alternating on one disk, with an `fdatasync` probe) would prove it.

A possible follow-up, not done here: have the Rust benchmark record the filesystem of its data
directory and a raw `fdatasync` baseline in its report, so a disk effect can be told from a code
regression by the file alone.

## Compatibility

- **C ABI 9** is additive: a binding built for ABI 9 refuses an ABI 8 library (and the reverse), as
  before. Existing functions, structs and status codes are unchanged. Plan wire v4 and data format
  4 are unchanged; no data migration.
- JVM: `NativeLibrary.EXPECTED_ABI` is 9. A custom `TelemetrySink` still works.
- Telemetry behaviour is unchanged except that `rows read` now comes from the engine's leaf-operator
  total (`QueryMetrics.source_rows`) instead of the scan-operator names in the profile text; both
  count the rows produced by the leaves, and the smoke test checks a join (20 + 200 rows).

## Definition of done

- [x] A skipped query allocates nothing more than an unobserved one (checked: 0 bytes)
- [x] A captured query does not fetch or decode the JSON profile
- [x] `adb_query_telemetry` documented in `adb.h`, Rustdoc and Javadoc; layout tested on both sides
- [x] Allocation held to a budget by `check-benchmarks.py` and by the smoke test
- [x] No new behaviour visible to SQL users; no format change
- [x] Reference-machine run of `scripts/benchmark-db.sh` and `scripts/benchmark-ffi.sh` with the new report (`examples/results/2.5.2.1-*.json`, quiet disk)
- [ ] Reference-machine run of the JVM `BenchmarkMain` (the off / skipped / captured timing comparison)

## Not verified

- Gradle (Maven Central is not reachable from the validation environment; CI runs it). The JVM
  modules were compiled with `scalac` 3.3.8 and `javac` 22 and the tests run with a JUnit stand-in.
- The time a capture saves on the reference machine: the three reports do not contain the telemetry
  timing comparison, so only allocation is confirmed there.
- The cause of the 2.5.2 write-path slowdown beyond what the two runs show (see below). The
  `compare-write-path.sh` comparison of the 2.5.1 and current builds on one disk was not run.
- Behaviour with many concurrent query threads: each thread has its own counter holder, but the
  contention on the queue and the counters has only been measured on 2 CPUs.
