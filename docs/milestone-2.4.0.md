# Milestone 2.4.0: schema evolution

2.4.0 implements Milestone 2.4 on top of 2.3.0: versioned schemas owned by the engine, changed
by `ALTER TABLE` without rewriting rows. It follows the 2.4.0 proposal, which reconciled the 2.4,
2.4.1 and 2.4.2 concepts into 25 tasks in six phases. This document records how the proposal was
implemented and adapted. The design is described in [schema.md](schema.md).

## Decisions taken before implementation

1. **The engine owns the catalog.** Schemas are catalog records in the canonical log and a
   projection (`SchemaRegistry`) like the stores and indexes. DDL is transactional and durable
   with the data, recovery replays both together, and every client sees the same versions. The
   JVM keeps a cached copy for binding and nothing else.
2. **No row is rewritten by DDL.** Rows carry the version they were written under and are
   adapted on read (virtual defaults, hidden drops); a write migrates the row it writes (lazy
   migration). `ALTER TABLE` costs one log record whatever the table's size.
3. **Field ids are forever.** A dropped field keeps its id and stays in the history, so old rows
   stay decodable and `AS OF` keeps working. Indexed fields cannot be dropped (RESTRICT), so
   index keys never change under an index.
4. **Epochs make stale plans impossible to run.** Every DDL statement (index DDL included)
   advances the catalog epoch; plans carry it and the engine refuses a mismatch before
   producing a row. The gateway re-plans once.
5. **Open questions of the proposal, resolved:** the import of the JVM catalog stays simple (the
   gateway imports `catalog.properties` at the first open, with explicit ids, and retires the
   file; `adb-migrate` handles format 1 → 3); `SHOW SCHEMA t` lists the version history.

## Delivered, task by task

Each task is one commit on the 2.4.0 branch, with its tests and documentation.

| # | Phase | Task | Main pieces |
|---|---|---|---|
| 1 | A foundations | Schema primitives and data format 3 | `SchemaVersion`, `SchemaEpoch`, versioned `Row` encoding, `ValueType::of/admits`, format marker 3 |
| 2 | A | Crate `adb-schema` | `EntitySchema`, `FieldDef`, `SchemaChange`, `create`, `apply`, validation, limits |
| 3 | A | Row adapter | `RowAdapter` (virtual defaults, hidden drops, both directions) |
| 4 | A | Catalog records | `WalRecord::Catalog`, `CatalogEvent`, `CommittedTx.catalog` |
| 5 | B registry and DDL | Registry projection | `SchemaRegistry`, `schema/registry.meta`, checkpoint, rebuild |
| 6 | B | Engine DDL | `create_entity`, `alter_entity`, `drop_entity`, `commit_catalog`, epochs of index DDL |
| 7 | B | Migration | `adb-migrate` writes format 3 |
| 8 | C runtime | Write validation | `prepare_write`, `check_row` |
| 9 | C | Lazy migration | adapted `get`, rewrites at the latest version |
| 10 | C | Read views | `ReadView { snapshot_ts, schema_epoch }`, epoch pinned per query |
| 11 | C | Indexes and statistics over adapted rows | adapted before-images and builds, statistics format 3 |
| 12 | D boundary and JVM | Plan wire v4 | `schema_epoch` in the envelope, `execute_planned`, status 13, `adb_current_schema_epoch` |
| 13 | D | C ABI 7 | five schema functions, error mapping, Java bindings |
| 14 | D | JVM `adb-schema` | `SchemaHistory`, `SchemaEvolutionPlanner`, `Compatibility`, `DependencyGraph`, `SchemaJson` |
| 15 | D | `EngineCatalog` | `SchemaCatalog`, `SchemaBackend`, `CatalogImport` |
| 16 | D | SQL | `ALTER TABLE`, `DROP TABLE`, `SHOW SCHEMA`, `DEFAULT` |
| 17 | D | Gateway | `AdaptiveDatabase.open`, import, `ensureCurrent`, replan on `SCHEMA_CHANGED` |
| 18 | D | Planner | `SchemaStatistics`, EXPLAIN Schema section |
| 19 | E deps, CDC, hardening | RESTRICT | dropping indexed or referenced objects refused; indexes need active fields |
| 20 | E | CDC | `CatalogChange`, `schema_version` per row change |
| 21 | E | Regression checks | primary keys, `AS OF` bounds, aggregates, slots |
| 22 | E | Hardening | fuzzing of schema JSON, catalog events, adapter; concurrent DDL |
| 23 | F benchmarks, demo, docs | Benchmarks | workloads F–H, ALTER latency, write validation |
| 24 | F | Demo | tour phase 2.4.0 |
| 25 | F | Documentation and version | this document, version 2.4.0, `build-milestone-2.4.0.sh` |

## Changes to the proposal

1. **No log rewrite for data format 3.** The proposal planned to rewrite the log into format 3,
   as 2.3.0 did for format 2. It turned out to be unnecessary: the row version is encoded so
   that an unversioned row is byte-for-byte a 2.3.0 row (a reserved field id carries the version
   only when there is one), and the catalog record is a WAL variant appended after the existing
   ones. A format-2 directory is upgraded in place on open; nothing is rewritten, so LSNs and CDC
   offsets stay. Only format 1 still goes through `adb-migrate`.
2. **RESTRICT also covers foreign-key hints.** `DROP TABLE` is refused while another live table
   holds a hint to it, so hints never dangle; the JVM `DependencyGraph` explains it with the same
   words as the engine.
3. **Statistics of new columns are derived, not missing.** Instead of only warning that a column
   has no statistics, the planner derives them from the default (exact for every analyzed row)
   and says so in EXPLAIN.
4. **One regression check failed and was fixed:** `AS OF VERSION -1` was accepted and would have
   wrapped around to a huge unsigned timestamp. The other checks passed unchanged.

## Definition of done

| Item | Proven by |
|---|---|
| versions chain and validate; refused changes leave no trace | `validation::*`, `schema_ddl::refused_ddl_leaves_no_trace` |
| old rows read defaults, dropped fields are hidden, both directions | `adapter::*`, `schema_reads::every_access_path_reads_adapted_rows` |
| DDL is durable with the data and replayed on recovery | `schema_registry::recovery_replays_catalog_records`, `::rebuild_restores_the_registry`, `ffi schema::schemas_survive_reopen` |
| format 2 upgrades in place; format 1 migrates to 3 | `format::format_two_directories_are_upgraded_in_place`, `migrate::*` |
| every write is validated and completed | `schema_writes::*`, `ffi schema::row_functions_validate_against_the_schema` |
| a running query keeps its schemas | `schema_reads::running_queries_keep_their_schema`, `schema_hardening::ddl_writes_and_scans_run_concurrently` |
| stale plans are refused, current ones run | `schema_reads::plans_of_another_epoch_are_refused`, `ffi index::stale_plans_are_refused_over_the_abi`, workload H |
| indexes and statistics agree with adapted rows | `schema_reads::indexes_and_statistics_see_adapted_rows`, `SchemaStatisticsTest` |
| RESTRICT | `schema_restrict::*`, `DependencyGraphTest`, `SchemaEvolutionPlannerTest` |
| SQL, binder and gateway | `SchemaSqlParserTest`, `SchemaBinderTest`, `EngineCatalogTest`, `SchemaGatewayTest` |
| change feed carries DDL and row versions | `schema_cdc::*`, `ffi schema::change_feed_carries_schema_events`, `CdcSmoke` |
| nothing panics on damaged input | `fuzz::*`, `catalog::damaged_catalog_frames_are_detected` |
| identical results whatever share of rows is migrated | equal checksums of workload G in `examples/results/2.4.0-database.json`, checked by `scripts/check-benchmarks.py` |
| `ALTER` costs the same on any table size | `alter_latency.*.log_bytes` in the same report |

## Supported SQL (2.4.0 additions)

```sql
CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL DEFAULT 0, note STRING);
ALTER TABLE account ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR';
ALTER TABLE account RENAME COLUMN note TO memo;
ALTER TABLE account DROP COLUMN memo;          -- RESTRICT; refused while an index uses it
SHOW SCHEMA account;                           -- every version: epoch, change, columns
DROP TABLE account;                            -- refused while indexes or hints depend on it
INSERT INTO account (id) VALUES (1);           -- balance and currency take their defaults
```

Everything from [milestone-2.3.0.md](milestone-2.3.0.md#supported-sql-230-additions) still
applies.

## Out of scope

Type changes, column splits and merges, computed adapters, table renames, `CASCADE`, background
migration of old rows, online index builds.
