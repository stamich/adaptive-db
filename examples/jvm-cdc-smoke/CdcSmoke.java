import io.adb.ffm.*;
import java.nio.file.*;
import java.util.Optional;

/** Java FFM -> native engine smoke test (ABI 5): change feed, offsets, vacuum, entity scan, statistics. Needs only a JDK 22. */
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
     * checks error statuses, then reopens the database and verifies the offset and new events.
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
            try (var q = db.execute("{\"wire_version\":2,\"plan\":{\"op\":\"entity_scan\",\"entity_id\":1,\"columns\":[{\"field_id\":1,\"slot\":0}]}}", Optional.empty())) {
                var batch = q.nextBatch();
                check(batch.isPresent() && batch.get().rowCount() == 1, "entity_scan returns entity 1 only");
            }
            check(db.statisticsJson(1).isEmpty(), "entity 1 has no statistics before ANALYZE");
            check(db.modificationsSinceAnalyze(1) == 2, "insert + update counted as 2 modifications");
            String stats = db.analyzeJson(1, null);
            check(stats.contains("\"row_count\":1"), "ANALYZE counts entity 1's row");
            check(db.statisticsJson(1).orElseThrow().equals(stats), "statistics document readable after ANALYZE");
            check(db.modificationsSinceAnalyze(1) == 0, "ANALYZE resets the modification delta");
            try { db.analyzeJson(1, "{\"sample_rows\":0}"); check(false, "invalid ANALYZE options"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "invalid ANALYZE options are INVALID_ARGUMENT"); }
            db.checkpoint();
            try { db.readChangesJson(3, 10); check(false, "mid-frame cursor rejected"); }
            catch (NativeException e) { check(e.status() == AdbStatus.INVALID_ARGUMENT, "mid-frame cursor rejected with INVALID_ARGUMENT"); }
            try { db.insert(1, 10, "{\"fields\":{}}"); check(false, "duplicate pk"); }
            catch (NativeException e) { check(e.status() == AdbStatus.CONFLICT, "duplicate primary key is CONFLICT"); }
        }
        try (var db = NativeDatabase.open(lib, data)) {
            check(db.consumerOffset("indexer").getAsLong() == cursor, "consumer offset survives reopen");
            check(db.readChangesJson(cursor, 10).contains("\"events\":[]"), "no new events after stored cursor");
            db.insert(1, 11, "{\"fields\":{\"1\":{\"Int64\":7}}}");
            check(db.readChangesJson(cursor, 10).contains("\"primary_key\":\"11\""), "new event visible after reopen");
        }
        System.out.println("CDC e2e smoke: PASS");
    }
}
