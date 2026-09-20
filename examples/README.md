# Milestone 1.7.2 examples

- `demo`: engine -> PhysicalPlan -> operators -> RecordBatch -> ADB Batch v1 -> C ABI.
- `benchmark`: execution and FFI baseline added on top of the 1.6.x storage baseline.

Run:
```bash
cargo run --release -p adb-demo-1-7-2
cargo run --release -p adb-benchmark-1-7-2 -- --rows 10000 --iters 1000 --output examples/results/1.7.2-local.json
```
