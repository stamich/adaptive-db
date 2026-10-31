# Schema evolution (Milestone 2.4.0)

From 2.4.0 the engine owns the catalog. Every table has a chain of **schema versions**; `ALTER
TABLE` publishes the next version without rewriting a single row. Rows remember the version they
were written under and are **adapted** when read, so a column added yesterday reads its default
in a row written last year, and a dropped column disappears from every read while its values stay
in the log for history.

```text
                 DDL (CREATE / ALTER / DROP TABLE, CREATE / DROP INDEX)
                                   │ commit_catalog: validate on a copy, then log
                                   ▼
  CANONICAL LOG ── Catalog{epoch, Schema | DropEntity | Epoch} ──► SchemaRegistry (projection)
       │                                                            │ checkpoint → schema/registry.meta
       │ Put rows stamped with their version                        │
       ▼                                                            ▼
  current / version stores ─── read ──► RowAdapter(row version → version at the query's epoch)
                                                     │ hide dropped fields, add virtual defaults
                                                     ▼
                                     scans, point lookups, row fetches, indexes, ANALYZE
```

## Model

| Concept | Meaning |
|---|---|
| **Entity schema** (`adb_schema::EntitySchema`) | One version of one table: name, version, parent version, the epoch it was published at, the primary key, the next free field id and **every field ever defined**. |
| **Field** (`FieldDef`) | Stable id, current name, type, nullability, default, `introduced_in`, `dropped_in`, foreign-key hint. Ids are never reused; a dropped field stays in later versions with `dropped_in` set. |
| **Schema version** | 1 for a new table, +1 per change. Rows carry it (0 = unversioned: written before 2.4.0 or to a table without a schema; read as version 1). |
| **Catalog epoch** (`SchemaEpoch`) | A database-wide counter advanced by every committed DDL statement, index DDL included. Plans record it; the engine refuses a plan of another epoch. |
| **Registry** (`SchemaRegistry`) | All histories by entity id, the current epoch and the next entity id; a projection of the catalog records in the log. |

The system field 0 (the JVM stores the entity id in it) is outside every schema: always accepted
and always kept.

### Changes

| SQL | `SchemaChange` | Rule |
|---|---|---|
| `ADD [COLUMN] c type [NOT NULL] [DEFAULT literal]` | `add_field` | `NOT NULL` needs a default (existing rows have no value); the default must have the column's type; names are unique case-insensitively. |
| `RENAME [COLUMN] a TO b` | `rename_field` | Metadata only: the field id, stored values, indexes and statistics stay. A case-only rename is allowed. |
| `DROP [COLUMN] c [RESTRICT]` | `drop_field` | Logical drop. Refused for the primary key and while an index uses the field as a key or `INCLUDE` column. `CASCADE` is refused. |
| `CREATE TABLE` (with `DEFAULT`) | `EntitySpec` | Version 1; field ids 1, 2, 3, ... in order (explicit ids only when importing). |
| `DROP TABLE t [RESTRICT]` | `DropEntity` | Refused while indexes exist on `t` or another live table has a foreign-key hint to it. The name becomes free; the id never does; the history stays readable. |

Type changes, table renames, column splits and merges are not part of 2.4.0 (they need row
rewriting or computed adapters; see [roadmap.md](roadmap.md)).

Limits: 4,096 fields per table (dropped ones included), 65,535 versions per table, 10,000 tables,
256-character names (letters, digits and `_`, not starting with a digit), defaults up to 1 KiB.

## Writes

`Database::commit` normalizes every `Put` of a table with a schema (`SchemaRegistry::prepare_write`):

1. an unversioned row is stamped with the latest version and the defaults of the fields it omits
   are filled in;
2. a row stamped with an older version is adapted to the latest (lazy migration);
3. a row stamped with a newer version than the engine knows is refused;
4. the result is checked (`validate::check_row`): only active fields, types match, `NOT NULL`
   fields present, the primary-key field equals the row's key.

Rows of tables without a schema are stored exactly as written (2.3.0 behaviour). A refused row
fails the commit with `DbError::Schema` (`INVALID_ARGUMENT` over the C ABI).

## Reads

Rows are stored as written and adapted on the way out. `RowAdapter::compile(history, from, to)`
keeps the fields active in `to` (plus field 0) and adds the defaults of fields introduced after
`from`; a row already at `to` passes unchanged. Adapters are cached per `(entity, from, to)`.

* `get`, `scan_page`, point lookups and row fetches adapt to the version current at the **read
  view's epoch** (`ReadView { snapshot_ts, schema_epoch }`). A query pins the epoch it starts at
  (or the epoch its plan was made at), so DDL committing while it runs never changes the shape of
  its rows: a column dropped mid-scan stays readable to that scan.
* `AS OF VERSION` reads old rows through the current schema (the planner binds against it).
* `history()` returns rows as stored.
* Index maintenance and builds use adapted rows, so a new column with a default is indexed with
  the default for rows that predate it. Indexed fields cannot be dropped, so index keys never
  change under an index.
* `ANALYZE` reads adapted rows and records the version in the statistics document (format 3).

## DDL as transactions

`Database::create_entity`, `alter_entity` and `drop_entity` run `commit_catalog` under the commit
lock: build the event, validate it by applying it to a **copy** of the registry, then log one
transaction holding a `WalRecord::Catalog`, apply it, sync and publish. A refused statement
leaves no trace. Index DDL logs an epoch-only event so that plans made before it are re-planned.
The registry is checkpointed to `schema/registry.meta` (envelope `ADBSCH01`) through the
checkpoint journal and rebuilt from the log by `rebuild_projections`.

## Plans and epochs

The JVM reads the epoch before it binds (`adb_current_schema_epoch`), binds against the schemas
of that epoch and sends it with the plan (plan wire v4, [plan-wire-format.md](plan-wire-format.md)).
`Database::execute_planned` compares it with the current epoch: a mismatch is
`ExecutionError::SchemaChanged` / status 13 `ADB_SCHEMA_CHANGED`, raised before any row is
produced. The gateway then refreshes its catalog and plans once more.

## The JVM side

| Piece | Module | Role |
|---|---|---|
| `EngineCatalog` | `adb-catalog` | The binder's catalog, a cached copy of `adb_list_schemas_json` at one epoch; `ensureCurrent()` reloads it when the engine's epoch moved. DDL goes through the C ABI. |
| `CatalogImport` | `adb-catalog` | One-time import of a 2.0–2.3 `catalog.properties` with the same entity and field ids; the gateway renames the file to `catalog.properties.imported`. |
| `SchemaEvolutionPlanner` | `adb-schema` | The engine's rules checked before the round trip, with SQL-level messages; predicts the next version. |
| `DependencyGraph` | `adb-schema` | Primary keys, index columns and foreign-key hints; explains RESTRICT refusals (`cannot drop column ledger.account: index idx_ledger_account (key column) depends on it`). |
| `SchemaHistory`, `SchemaJson` | `adb-schema` | Version lookups by epoch, reconstruction of each change (`SHOW SCHEMA`), the engine's JSON forms. |
| `SchemaStatistics` | `adb-statistics` | Columns added after `ANALYZE` get statistics derived from their default: every analyzed row reads it. |

`EXPLAIN` has a **Schema** section: the plan's catalog epoch, the schema version of every table
read and the columns whose estimates come from their defaults.

## Change feed

A `CREATE`/`ALTER TABLE` appears in the feed as a `schema` entry with the full new version and a
`DROP TABLE` as a `drop_table` entry, in commit order with the row changes; every row change
names the schema version it was written under ([cdc.md](cdc.md)).

## Data format 3

Data format 3 adds the row version and the catalog records. Neither changes how 2.3.0 data is
encoded: an unversioned row is byte-for-byte a 2.3.0 row, and the catalog record is a new WAL
variant appended after the existing ones. A format-2 directory is therefore **upgraded in place**
on open (the marker is restamped; nothing is rewritten, LSNs and CDC offsets stay). Format 1
still needs `adb-migrate`, which now writes format 3. A directory of a newer format is refused
with `ADB_INCOMPATIBLE_FORMAT`.

A database written before 2.4.0 has no schemas in the engine; the gateway imports its JVM
catalog at the first open. Its rows stay unversioned and read as version 1 until rewritten.
