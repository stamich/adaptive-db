package io.adb.ffm;

import java.lang.foreign.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Objects;
import java.util.Optional;
import java.util.OptionalLong;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * Safe-ish Java ownership wrapper around one native Adaptive DB database handle.
 *
 * <p>Short calls are serialized on the object monitor. {@link #analyzeJson} scans a whole
 * entity and {@link #createIndexJson} builds a whole index, so they run outside the monitor under
 * the read side of {@code lifecycle} (the native database is thread-safe); {@link #close} takes
 * the write side, so the handle is never released while such a call still uses it.
 */
public final class NativeDatabase implements AutoCloseable {
    /** Upper bound on the UTF-8 length of a database directory path. */
    private static final int MAX_PATH_BYTES = 64 * 1024;
    /** Upper bound on the UTF-8 length of a JSON argument (plan, row, or field update). */
    private static final int MAX_JSON_BYTES = 8 * 1024 * 1024;
    /** Upper bound on the size of a change-feed JSON response. */
    private static final long MAX_CHANGES_JSON_BYTES = 64L * 1024 * 1024;
    /** Upper bound on the size of a statistics document (matches the native 4 MiB limit). */
    private static final long MAX_STATISTICS_JSON_BYTES = 4L * 1024 * 1024;
    /** Upper bound on the UTF-8 length of ANALYZE options. */
    private static final int MAX_ANALYZE_OPTIONS_BYTES = 64 * 1024;
    /** Upper bound on the UTF-8 length of a CDC consumer name. */
    private static final int MAX_CONSUMER_NAME_BYTES = 256;
    /** Upper bound on the UTF-8 length of an index specification (matches the native 64 KiB limit). */
    private static final int MAX_INDEX_SPEC_BYTES = 64 * 1024;
    /** Upper bound on the UTF-8 length of an index name. */
    private static final int MAX_INDEX_NAME_BYTES = 128;
    /** Upper bound on the size of an index status or listing document. */
    private static final long MAX_INDEX_JSON_BYTES = 16L * 1024 * 1024;
    /** Upper bound on the UTF-8 length of a schema definition or change (matches the native 1 MiB limit). */
    private static final int MAX_SCHEMA_REQUEST_BYTES = 1024 * 1024;
    /** Upper bound on the size of a schema, catalog or history document. */
    private static final long MAX_SCHEMA_JSON_BYTES = 64L * 1024 * 1024;

    /** Loaded native library and its downcall handles. */
    private final NativeLibrary library;
    /** Native database handle; {@code MemorySegment.NULL} once closed. */
    private volatile MemorySegment handle;
    /** Read side: long native calls outside the monitor; write side: {@link #close}. */
    private final ReentrantReadWriteLock lifecycle = new ReentrantReadWriteLock();

    /** Opens the native library and one database directory. */
    public static NativeDatabase open(Path libraryPath, Path dataPath) {
        return new NativeDatabase(new NativeLibrary(Objects.requireNonNull(libraryPath)), Objects.requireNonNull(dataPath));
    }

    /** Creates a live wrapper and validates the path before crossing the FFI boundary. */
    private NativeDatabase(NativeLibrary library, Path dataPath) {
        this.library = library;
        byte[] path = dataPath.toString().getBytes(StandardCharsets.UTF_8);
        if (path.length == 0 || path.length > MAX_PATH_BYTES) throw new IllegalArgumentException("invalid database path length");
        try (var arena = Arena.ofConfined()) {
            MemorySegment pathSegment = arena.allocate(path.length);
            pathSegment.copyFrom(MemorySegment.ofArray(path));
            MemorySegment out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            int status = library.invokeInt(library.open, pathSegment, (long) path.length, out);
            library.check(status);
            handle = out.get(ValueLayout.ADDRESS, 0);
            if (handle.address() == 0) throw new IllegalStateException("native open succeeded without a database handle");
        }
    }

    /** Returns the latest native committed timestamp. */
    public synchronized long latestCommittedTs() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.latestCommittedTs, handle, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /**
     * Returns the current catalog epoch. Every plan carries the epoch it was made at (plan wire
     * v4); {@link #execute} fails with {@link AdbStatus#SCHEMA_CHANGED} when it moved since.
     */
    public synchronized long currentSchemaEpoch() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.currentSchemaEpoch, handle, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /** Executes one bounded physical-plan JSON document at the latest or supplied snapshot. */
    public synchronized NativeQuery execute(String planJson, Optional<Long> snapshotTs) {
        ensureOpen();
        Objects.requireNonNull(snapshotTs, "snapshotTs");
        byte[] bytes = boundedJson(planJson, "plan");
        try (var arena = Arena.ofConfined()) {
            var input = arena.allocate(bytes.length);
            input.copyFrom(MemorySegment.ofArray(bytes));
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            int status = snapshotTs.isPresent()
                ? library.invokeInt(library.executeAt, handle, input, (long) bytes.length, snapshotTs.get(), out)
                : library.invokeInt(library.execute, handle, input, (long) bytes.length, out);
            library.check(status);
            var queryHandle = out.get(ValueLayout.ADDRESS, 0);
            if (queryHandle.address() == 0) throw new IllegalStateException("native execute succeeded without a query handle");
            return new NativeQuery(library, queryHandle);
        }
    }

    /** Inserts one row encoded by the gateway and returns the commit timestamp. */
    public synchronized long insert(long entityId, long primaryKey, String rowJson) { return mutation(library.insertRow, entityId, primaryKey, rowJson); }

    /** Updates selected fields and returns the commit timestamp. */
    public synchronized long update(long entityId, long primaryKey, String assignmentsJson) { return mutation(library.updateFields, entityId, primaryKey, assignmentsJson); }

    /** Invokes one bounded JSON mutation function. */
    private long mutation(java.lang.invoke.MethodHandle fn, long entityId, long primaryKey, String json) {
        ensureOpen();
        byte[] bytes = boundedJson(json, "mutation");
        try (var arena = Arena.ofConfined()) {
            var input = arena.allocate(bytes.length);
            input.copyFrom(MemorySegment.ofArray(bytes));
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            out.set(ValueLayout.JAVA_LONG, 0, 0L);
            library.check(library.invokeInt(fn, handle, entityId, primaryKey, input, (long) bytes.length, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /** Deletes one row and returns the commit timestamp. */
    public synchronized long delete(long entityId, long primaryKey) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            out.set(ValueLayout.JAVA_LONG, 0, 0L);
            library.check(library.invokeInt(library.deleteRow, handle, entityId, primaryKey, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    // ----- change data capture ------------------------------------------------------------

    /**
     * Reads up to {@code maxEvents} committed transactions after {@code cursor} as one JSON
     * document ({@code {"next_cursor": "...", "events": [...]}}); see {@code include/adb.h}.
     * Cursors are unsigned 64-bit log positions; {@code 0} is the beginning of the log.
     * With no {@code entityIds} every entity is included.
     */
    public synchronized String readChangesJson(long cursor, int maxEvents, long... entityIds) {
        ensureOpen();
        Objects.requireNonNull(entityIds, "entityIds");
        try (var arena = Arena.ofConfined()) {
            var entities = entityIds.length == 0
                ? MemorySegment.NULL
                : arena.allocateFrom(ValueLayout.JAVA_LONG, entityIds);
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.readChanges, handle, cursor, maxEvents,
                entities, (long) entityIds.length, out));
            byte[] json = library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_CHANGES_JSON_BYTES);
            return new String(json, StandardCharsets.UTF_8);
        }
    }

    /** Cursor at the durable end of the log: start here to receive only new changes. */
    public synchronized long changeFeedEnd() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.changeFeedEnd, handle, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /** Durably stores the cursor of a named consumer. */
    public synchronized void commitConsumerOffset(String consumer, long cursor) {
        ensureOpen();
        byte[] name = consumerName(consumer);
        try (var arena = Arena.ofConfined()) {
            var nameSegment = arena.allocateFrom(ValueLayout.JAVA_BYTE, name);
            library.check(library.invokeInt(library.commitConsumerOffset, handle, nameSegment, (long) name.length, cursor));
        }
    }

    /** Stored cursor of a named consumer, or empty if it never committed one. */
    public synchronized OptionalLong consumerOffset(String consumer) {
        ensureOpen();
        byte[] name = consumerName(consumer);
        try (var arena = Arena.ofConfined()) {
            var nameSegment = arena.allocateFrom(ValueLayout.JAVA_BYTE, name);
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            int code = library.invokeInt(library.consumerOffset, handle, nameSegment, (long) name.length, out);
            if (AdbStatus.fromCode(code) == AdbStatus.NOT_FOUND) return OptionalLong.empty();
            library.check(code);
            return OptionalLong.of(out.get(ValueLayout.JAVA_LONG, 0));
        }
    }

    // ----- maintenance ----------------------------------------------------------------------

    /** Persists all in-memory projection changes (also done automatically and on close). */
    public synchronized void checkpoint() {
        ensureOpen();
        library.check(library.invokeInt(library.checkpoint, handle));
    }

    /** Removes expired delete tombstones; returns how many were removed. */
    public synchronized long vacuum() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.vacuum, handle, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    // ----- statistics (ABI 5) ----------------------------------------------------------------

    /**
     * Runs {@code ANALYZE} on one entity and returns its new statistics document as JSON
     * (see {@code docs/statistics.md}). {@code optionsJson} may be {@code null} or empty for
     * the default options.
     */
    public String analyzeJson(long entityId, String optionsJson) {
        lifecycle.readLock().lock();
        try {
            return analyzeOpen(entityId, optionsJson);
        } finally {
            lifecycle.readLock().unlock();
        }
    }

    /** Body of {@link #analyzeJson}; the caller holds the read side of {@code lifecycle}. */
    private String analyzeOpen(long entityId, String optionsJson) {
        ensureOpen();
        byte[] options = optionsJson == null ? new byte[0] : optionsJson.getBytes(StandardCharsets.UTF_8);
        if (options.length > MAX_ANALYZE_OPTIONS_BYTES) throw new IllegalArgumentException("ANALYZE options JSON longer than " + MAX_ANALYZE_OPTIONS_BYTES + " bytes");
        try (var arena = Arena.ofConfined()) {
            var optionsSegment = options.length == 0 ? MemorySegment.NULL : arena.allocateFrom(ValueLayout.JAVA_BYTE, options);
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.analyzeEntity, handle, entityId, optionsSegment, (long) options.length, out));
            return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_STATISTICS_JSON_BYTES), StandardCharsets.UTF_8);
        }
    }

    /** The statistics document of one entity as JSON, or empty if it was never analyzed. */
    public synchronized Optional<String> statisticsJson(long entityId) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            int code = library.invokeInt(library.statistics, handle, entityId, out);
            if (AdbStatus.fromCode(code) == AdbStatus.NOT_FOUND) return Optional.empty();
            library.check(code);
            return Optional.of(new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_STATISTICS_JSON_BYTES), StandardCharsets.UTF_8));
        }
    }

    /**
     * Generation of an entity's statistics document (0 if there is none); it changes whenever
     * {@code ANALYZE} publishes a new document, so decoded documents can be cached by it.
     */
    public synchronized long statisticsGeneration(long entityId) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.statisticsGeneration, handle, entityId, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /** Row mutations committed to an entity since its last {@code ANALYZE} (since creation if never analyzed). */
    public synchronized long modificationsSinceAnalyze(long entityId) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.modificationsSinceAnalyze, handle, entityId, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    // ----- secondary indexes (ABI 6) ----------------------------------------------------------

    /**
     * Creates and builds a secondary index from a JSON specification
     * ({@code {"name":..,"entity_id":..,"columns":[{"field_id":..,"direction":"asc"}],"include":[..]}})
     * and returns its status as JSON (see {@code include/adb.h} and {@code docs/indexes.md}).
     * The build blocks commits of the native database until the index is published.
     */
    public String createIndexJson(String specJson) {
        lifecycle.readLock().lock();
        try {
            ensureOpen();
            Objects.requireNonNull(specJson, "specJson");
            byte[] spec = specJson.getBytes(StandardCharsets.UTF_8);
            if (spec.length == 0 || spec.length > MAX_INDEX_SPEC_BYTES) throw new IllegalArgumentException("index spec length outside 1.." + MAX_INDEX_SPEC_BYTES);
            try (var arena = Arena.ofConfined()) {
                var specSegment = arena.allocateFrom(ValueLayout.JAVA_BYTE, spec);
                var out = arena.allocate(ValueLayout.ADDRESS);
                out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
                library.check(library.invokeInt(library.createIndex, handle, specSegment, (long) spec.length, out));
                return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_INDEX_JSON_BYTES), StandardCharsets.UTF_8);
            }
        } finally {
            lifecycle.readLock().unlock();
        }
    }

    /** Drops the index named {@code name} (case-insensitive); a missing index throws {@link NativeException} with {@code NOT_FOUND}. */
    public synchronized void dropIndex(String name) {
        ensureOpen();
        Objects.requireNonNull(name, "name");
        byte[] bytes = name.getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_INDEX_NAME_BYTES) throw new IllegalArgumentException("index name length outside 1.." + MAX_INDEX_NAME_BYTES);
        try (var arena = Arena.ofConfined()) {
            var nameSegment = arena.allocateFrom(ValueLayout.JAVA_BYTE, bytes);
            library.check(library.invokeInt(library.dropIndex, handle, nameSegment, (long) bytes.length));
        }
    }

    /** Every index with the snapshots it serves, as JSON {@code {"indexes":[...]}}. */
    public synchronized String listIndexesJson() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.listIndexes, handle, out));
            return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_INDEX_JSON_BYTES), StandardCharsets.UTF_8);
        }
    }

    // ----- entity schemas (ABI 7) -------------------------------------------------------------

    /**
     * Creates an entity ({@code CREATE TABLE}) from a JSON definition
     * ({@code {"name":..,"entity_id":..,"primary_key":..,"fields":[{"name":..,"value_type":..,"nullable":..,"default":..}]}})
     * and returns its first schema version as JSON (see {@code include/adb.h} and {@code docs/schema.md}).
     * The call commits one catalog record and advances the catalog epoch.
     */
    public synchronized String createEntityJson(String specJson) {
        return schemaRequest(library.createEntity, specJson, "entity spec");
    }

    /**
     * Applies one schema change ({@code {"entity_id":..,"change":{"op":"add_field"|"rename_field"|"drop_field",..}}})
     * and returns the new schema version as JSON. Existing rows are not rewritten.
     */
    public synchronized String alterEntityJson(String requestJson) {
        return schemaRequest(library.alterEntity, requestJson, "alter request");
    }

    /** Drops an entity ({@code DROP TABLE}); a missing entity throws {@link NativeException} with {@code NOT_FOUND}. */
    public synchronized void dropEntity(long entityId) {
        ensureOpen();
        library.check(library.invokeInt(library.dropEntity, handle, entityId));
    }

    /**
     * The catalog as JSON {@code {"schema_epoch":N,"entities":[schema,..]}}: the latest schema of every live
     * entity, consistent with the epoch, which is what a planner caches.
     */
    public synchronized String listSchemasJson() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.listSchemas, handle, out));
            return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_SCHEMA_JSON_BYTES), StandardCharsets.UTF_8);
        }
    }

    /** Every schema version of one entity, oldest first, as JSON {@code {"entity_id":N,"versions":[..]}}. */
    public synchronized String schemaHistoryJson(long entityId) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.schemaHistory, handle, entityId, out));
            return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_SCHEMA_JSON_BYTES), StandardCharsets.UTF_8);
        }
    }

    /** Sends a bounded JSON request to a schema function of the form {@code (db, ptr, len, out_json)}. */
    private String schemaRequest(java.lang.invoke.MethodHandle function, String json, String what) {
        ensureOpen();
        Objects.requireNonNull(json, what);
        byte[] bytes = json.getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_SCHEMA_REQUEST_BYTES) throw new IllegalArgumentException(what + " length outside 1.." + MAX_SCHEMA_REQUEST_BYTES);
        try (var arena = Arena.ofConfined()) {
            var segment = arena.allocateFrom(ValueLayout.JAVA_BYTE, bytes);
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(function, handle, segment, (long) bytes.length, out));
            return new String(library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_SCHEMA_JSON_BYTES), StandardCharsets.UTF_8);
        }
    }

    /** Checkpoints and releases the native database exactly once, then closes the library arena. */
    @Override public synchronized void close() {
        lifecycle.writeLock().lock();
        try {
            if (handle != null && handle.address() != 0) {
                library.check(library.invokeInt(library.close, handle));
                handle = MemorySegment.NULL;
            }
            library.close();
        } finally {
            lifecycle.writeLock().unlock();
        }
    }

    /** Rejects method calls after native ownership has been released. */
    private void ensureOpen() {
        if (handle == null || handle.address() == 0) throw new IllegalStateException("database is closed");
    }

    /** Validates a consumer name and encodes it as UTF-8. */
    private static byte[] consumerName(String consumer) {
        Objects.requireNonNull(consumer, "consumer");
        byte[] bytes = consumer.getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_CONSUMER_NAME_BYTES) throw new IllegalArgumentException("consumer name length outside 1.." + MAX_CONSUMER_NAME_BYTES);
        return bytes;
    }

    /** Converts a Java string to a bounded UTF-8 payload before native allocation/call. */
    private static byte[] boundedJson(String json, String kind) {
        Objects.requireNonNull(json, kind);
        byte[] bytes = json.getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_JSON_BYTES) throw new IllegalArgumentException(kind + " JSON length outside 1.." + MAX_JSON_BYTES);
        return bytes;
    }
}
