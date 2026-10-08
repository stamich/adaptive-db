# Benchmark results

Store machine-specific Milestone 1.6.2 JSON outputs here. Do not commit fabricated baseline
numbers. For useful comparisons record CPU, memory, storage device/filesystem, OS/kernel,
Rust version and whether the filesystem/cache state was warm or cold.

## Milestone 2.2.3 workloads

* `2.2.3-database.json` — `scripts/benchmark-db.sh`: the `rust_native` path (cost-based plans of
  workloads A and B executed directly in Rust) and `ANALYZE` timings.
* `2.2.3-ffi.json` — `scripts/benchmark-ffi.sh`: the `ffi_prepared_plan` and
  `scala_cbo_ffi_rust` paths, the join-order workload in cost and rule mode, and the cost
  calibration (`ms_per_1k_cost`).

Both use the same tables and formulas (`bench_customer`: 100 rows, `bench_orders`: 1,000 rows), so
the `checksum` of a workload must be identical on every path. The recorded files come from a
2-vCPU cloud VM (warm cache, release build, JDK 22); expect run-to-run variation of 10–30%.
