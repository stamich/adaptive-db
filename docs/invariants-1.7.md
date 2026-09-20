# Milestone 1.7 invariants

1. Execution nie zna fizycznego layoutu storage.
2. PhysicalPlan jest walidowany przed execution.
3. Query pracuje względem jawnego snapshot timestamp.
4. PointLookup AS OF używa istniejącej temporalnej semantyki MVCC.
5. Tombstones nie są emitowane przez current Scan.
6. Result API zwraca batche, nie pojedyncze JVM/native obiekty.
7. Rust-allocated FFI memory jest zwalniane wyłącznie przez Rust API.
8. Panic nie może przekroczyć C ABI.
9. ABI ma jawny numer wersji.
10. Batch wire format ma jawny numer wersji.
11. Cancellation jest obserwowana pomiędzy batchami/operatorami.
12. FFI nie ujawnia RowLocation/PageId/B+Tree internals.
