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
    +-- Scan
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

`adb-execution` zależy wyłącznie od `adb-core`. Dostęp do danych odbywa się przez `DataSource`, więc execution layer nie zna PageId, SlotId, B+Tree node layout ani WAL.
