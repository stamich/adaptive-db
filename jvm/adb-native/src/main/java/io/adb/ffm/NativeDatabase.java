package io.adb.ffm;

import java.lang.foreign.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Objects;
import java.util.Optional;

/** Safe-ish Java ownership wrapper around one native Adaptive DB database handle. */
public final class NativeDatabase implements AutoCloseable {
    private static final int MAX_PATH_BYTES = 64 * 1024;
    private static final int MAX_JSON_BYTES = 8 * 1024 * 1024;

    private final NativeLibrary library;
    private MemorySegment handle;

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

    /** Releases the native database exactly once and then closes the library arena. */
    @Override public synchronized void close() {
        if (handle != null && handle.address() != 0) {
            library.check(library.invokeInt(library.close, handle));
            handle = MemorySegment.NULL;
        }
        library.close();
    }

    /** Rejects method calls after native ownership has been released. */
    private void ensureOpen() {
        if (handle == null || handle.address() == 0) throw new IllegalStateException("database is closed");
    }

    /** Converts a Java string to a bounded UTF-8 payload before native allocation/call. */
    private static byte[] boundedJson(String json, String kind) {
        Objects.requireNonNull(json, kind);
        byte[] bytes = json.getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_JSON_BYTES) throw new IllegalArgumentException(kind + " JSON length outside 1.." + MAX_JSON_BYTES);
        return bytes;
    }
}
