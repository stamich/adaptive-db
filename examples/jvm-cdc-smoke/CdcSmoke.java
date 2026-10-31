import io.adb.ffm.*;
import java.nio.file.*;
import java.util.Optional;

/** Java FFM -> native engine smoke test (ABI 7): change feed, offsets, vacuum, entity scan, statistics, indexes, schemas. Needs only a JDK 22. */
public class CdcSmoke {
    /**
     * Fails the smoke test unless {@code ok} holds, otherwise prints a pass line.
     *
     * @param ok   condition that must be true
     * @param what human-readable description of the check
     */
    static void check(boolean ok, String what) { if (!ok) throw new AssertionError(what); System.out.println("ok  " + what); }
    /**
     * Runs the scenario: writes, reads the change feed, stores a consumer offset, vacuums, scans,
     * analyzes, creates an index, checks error statuses, then reopens the database and verifies
     * the offset, the index, new events and a negative primary key; finally evolves a schema and
     * checks the schema events of the change feed and the refusal of a stale plan.
     *
     * @param args {@code args[0]} is the path of the native library
     * @throws Exception on any unexpected failure
     */
    public static void main(String[] args) throws Exception {
        Path lib = Path.of(args[0]);
        Path data = Files.createTempDirectory("adb-e2e");
        long cursor;
        try (var db = NativeDatabase.open(lib, data)) {
            db.insert(1, 10, "{\"fields\":{\"1\":{\"Int64\":100}}}");
            db.insert(2, 20, "{\"fields\":{\"1\":{\"Int64\":200}}}");
            db.update(1, 10, "{\"1\":{\"Int64\":101}}");
            db.delete(2, 20);
            String all = db.readChangesJson(0, 100);
            check(all.contains("\"kind\":\"insert\"") && all.contains("\"kind\":\"update\"") && all.contains("\"kind\":\"delete\""), "change feed has insert/update/delete");
            String onlyOne = db.readChangesJson(0, 100, 1L);
            check(!onlyOne.contains("\"entity_id\":2"), "entity filter excludes entity 2");
            cursor = db.changeFeedEnd();
            check(all.contains("\"next_cursor\":\"" + Long.toUnsignedString(cursor) + "\""), "next_cursor equals feed end");
            check(db.consumerOffset("indexer").isEmpty(), "unknown consumer has no offset");
            db.commitConsumerOffset("indexer", cursor);
            check(db.vacuum() == 1, "vacuum removed the tombstone");
            try (var q = db.execute("{\"wire_version\":4,\"schema_epoch\":0,\"plan\":{\"op\":\"entity_scan\",\"entity_id\":1,\"columns\":[{\"field_id\":1,\"slot\":0}]}}", Optional.empty())) {
                var batch = q.nextBatch();
                check(batch.isPresent() && batch.get().rowCount() == 1, "entity_scan returns entity 1 only");
            }
            check(db.statisticsJson(1).isEmpty(), "entity 1 has no statistics before ANALYZE");
            check(db.modificationsSinceAnalyze(1) == 2, "insert + update counted as 2 modifications");
            String stats = db.analyzeJson(1, null);
            check(stats.contains("\"row_count\":1"), "ANALYZE counts entity 1's row");
            check(db.statisticsJson(1).orElseThrow().equals(stats), "statistics document readable after ANALYZE");
            check(db.modificationsSinceAnalyze(1) == 0, "ANALYZE resets the modification delta");
            check(db.statisticsGeneration(1) > 0 && db.statisticsGeneration(2) == 0, "statistics generation identifies documents");
            try { db.analyzeJson(1, "{\"sample_rows\":0}"); check(false, "invalid ANALYZE options"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "invalid ANALYZE options are INVALID_ARGUMENT"); }
            db.checkpoint();
            try { db.readChangesJson(3, 10); check(false, "mid-frame cursor rejected"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "mid-frame cursor rejected with INVALID_ARGUMENT"); }
            try { db.insert(1, 10, "{\"fields\":{}}"); check(false, "duplicate pk"); }
            catch (NativeException e) { check(e.status() == AdbStatus.CONFLICT, "duplicate primary key is CONFLICT"); }
            String created = db.createIndexJson("{\"name\":\"idx_value\",\"entity_id\":1,\"columns\":[{\"field_id\":1}]}");
            check(created.contains("\"name\":\"idx_value\"") && created.contains("\"state\":\"active\""), "CREATE INDEX returns an active index");
            try { db.createIndexJson("{\"name\":\"IDX_VALUE\",\"entity_id\":1,\"columns\":[{\"field_id\":1}]}"); check(false, "taken index name"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "a taken index name is INVALID_ARGUMENT"); }
        }
        try (var db = NativeDatabase.open(lib, data)) {
            check(db.consumerOffset("indexer").getAsLong() == cursor, "consumer offset survives reopen");
            check(db.readChangesJson(cursor, 10).contains("\"events\":[]"), "no new events after stored cursor");
            db.insert(1, 11, "{\"fields\":{\"1\":{\"Int64\":7}}}");
            check(db.readChangesJson(cursor, 10).contains("\"primary_key\":\"11\""), "new event visible after reopen");
            check(db.listIndexesJson().contains("\"name\":\"idx_value\""), "the index survives reopen");
            db.dropIndex("idx_value");
            try { db.dropIndex("idx_value"); check(false, "drop of a missing index"); }
            catch (NativeException e) { check(e.status() == AdbStatus.NOT_FOUND, "dropping a missing index is NOT_FOUND"); }
            long before = db.changeFeedEnd();
            db.insert(1, -5, "{\"fields\":{\"1\":{\"Int64\":1}}}");
            check(db.readChangesJson(before, 10).contains("\"primary_key\":\"-5\""), "negative primary keys round-trip (data format 2)");

            // Schemas (ABI 7): DDL and row versions in the change feed, validation, stale plans.
            long schemaStart = db.changeFeedEnd();
            long epoch0 = db.currentSchemaEpoch();
            String v1 = db.createEntityJson("{\"name\":\"account\",\"entity_id\":5,\"primary_key\":\"id\",\"fields\":[{\"name\":\"id\",\"value_type\":\"Int64\",\"nullable\":false},{\"name\":\"balance\",\"value_type\":\"Int64\",\"nullable\":false}]}");
            check(v1.contains("\"version\":1") && db.currentSchemaEpoch() == epoch0 + 1, "CREATE TABLE publishes version 1 and moves the epoch");
            db.insert(5, 1, "{\"fields\":{\"1\":{\"Int64\":1},\"2\":{\"Int64\":100}}}");
            String stalePlan = "{\"wire_version\":4,\"schema_epoch\":" + db.currentSchemaEpoch() + ",\"plan\":{\"op\":\"entity_scan\",\"entity_id\":5,\"columns\":[{\"field_id\":3,\"slot\":0}]}}";
            String v2 = db.alterEntityJson("{\"entity_id\":5,\"change\":{\"op\":\"add_field\",\"field\":{\"name\":\"currency\",\"value_type\":\"String\",\"nullable\":false,\"default\":{\"String\":\"EUR\"}}}}");
            check(v2.contains("\"version\":2") && v2.contains("\"introduced_in\":2"), "ADD COLUMN publishes version 2");
            db.insert(5, 2, "{\"fields\":{\"1\":{\"Int64\":2},\"2\":{\"Int64\":200}}}");
            try { db.insert(5, 3, "{\"fields\":{\"1\":{\"Int64\":3},\"2\":{\"String\":\"x\"}}}"); check(false, "mistyped value"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "a row that does not fit its schema is INVALID_ARGUMENT"); }
            try (var q = db.execute(stalePlan, Optional.empty())) { check(false, "stale plan"); }
            catch (NativeException e) { check(e.status() == AdbStatus.SCHEMA_CHANGED, "a plan of an older epoch is SCHEMA_CHANGED"); }
            String fresh = "{\"wire_version\":4,\"schema_epoch\":" + db.currentSchemaEpoch() + ",\"plan\":{\"op\":\"entity_scan\",\"entity_id\":5,\"columns\":[{\"field_id\":3,\"slot\":0}]}}";
            try (var q = db.execute(fresh, Optional.empty())) {
                var batch = q.nextBatch().orElseThrow();
                check(batch.rowCount() == 2 && "EUR".equals(batch.columns().get(0).values().get(0)), "an old row reads the default of a new column");
            }
            check(db.listSchemasJson().contains("\"currency\"") && db.schemaHistoryJson(5).contains("\"version\":2"), "catalog listing and history");
            db.dropEntity(5);
            String feed = db.readChangesJson(schemaStart, 100, 5L);
            check(feed.contains("\"kind\":\"schema\"") && feed.contains("\"kind\":\"drop_table\""), "the change feed carries CREATE/ALTER and DROP TABLE");
            check(feed.contains("\"schema_version\":1") && feed.contains("\"schema_version\":2"), "row changes name their schema version");
            try { db.dropEntity(5); check(false, "drop of a dropped table"); }
            catch (NativeException e) { check(e.status() == AdbStatus.NOT_FOUND, "dropping a missing table is NOT_FOUND"); }
        }
        System.out.println("CDC e2e smoke: PASS");
    }
}
