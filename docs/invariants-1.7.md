# Milestone 1.7 invariants

1. Execution does not know the physical storage layout.
2. A PhysicalPlan is validated before execution.
3. A query runs against an explicit snapshot timestamp.
4. PointLookup AS OF uses the existing temporal MVCC semantics.
5. Tombstones are never emitted by a current-snapshot Scan.
6. The result API returns batches, not individual JVM/native objects.
7. Rust-allocated FFI memory is released only through the Rust API.
8. A panic never crosses the C ABI.
9. The ABI has an explicit version number.
10. The batch wire format has an explicit version number.
11. Cancellation is observed between batches/operators.
12. The FFI does not expose RowLocation/PageId/B+Tree internals.
