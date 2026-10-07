# Execution layer

```text
PhysicalPlan
    |
    v
Executor::execute
    |
    v
Operator tree (pull, batch-oriented)
    |
    +-- PointLookup
    +-- Scan / EntityScan
    +-- Filter
    +-- Project
    +-- Limit
    |
    v
RowBatch (internal)
    |
    v
RecordBatch (columnar public result)
```

`adb-execution` depends only on `adb-core`. All data access goes through `DataSource`, so the execution layer knows nothing about PageId, TupleSlot, the B+Tree node layout or the WAL.

Since Milestone 2.0.3 scans are key-range scans paged through `DataSource::scan_page` (`Scan` = every entity, `EntityScan` = one entity), so a scan never holds more than one batch in memory.
