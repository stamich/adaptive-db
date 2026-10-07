package io.adb.ffm;

import java.lang.foreign.*;
import java.util.Optional;

/** Owns one native query cursor and exposes bounded decoded batches. */
public final class NativeQuery implements AutoCloseable {
    /** Largest accepted native batch buffer, in bytes. */
    private static final long MAX_BATCH_BYTES = 64L * 1024 * 1024;
    /** Native library that owns the downcall handles. */
    private final NativeLibrary library;
    /** Native query cursor handle; {@code MemorySegment.NULL} once closed. */
    private MemorySegment handle;

    /** Creates a wrapper around a non-null query handle returned by the native ABI. */
    NativeQuery(NativeLibrary library, MemorySegment handle) {
        this.library = library;
        this.handle = handle;
        if (handle == null || handle.address() == 0) throw new IllegalArgumentException("null native query handle");
    }

    /** Returns the next decoded batch or empty at end of stream. */
    public synchronized Optional<NativeRecordBatch> nextBatch() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            int code = library.invokeInt(library.queryNextBatch, handle, out);
            var status = AdbStatus.fromCode(code);
            if (status == AdbStatus.END_OF_STREAM) return Optional.empty();
            library.check(code);
            byte[] bytes = library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_BATCH_BYTES);
            return Optional.of(BatchDecoder.decode(bytes));
        }
    }

    /**
     * Returns the query's per-operator runtime profile (rows, batches, time, counters, peak
     * memory) as JSON; complete once {@link #nextBatch()} returned empty. Used by EXPLAIN ANALYZE.
     *
     * @return the profile document
     */
    public synchronized String profileJson() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            library.check(library.invokeInt(library.queryProfile, handle, out));
            byte[] bytes = library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_BATCH_BYTES);
            return new String(bytes, java.nio.charset.StandardCharsets.UTF_8);
        }
    }

    /** Requests cooperative cancellation of this live query. */
    public synchronized void cancel() { ensureOpen(); library.check(library.invokeInt(library.queryCancel, handle)); }

    /** Releases the native query handle exactly once. */
    @Override public synchronized void close() {
        if (handle != null && handle.address() != 0) {
            library.check(library.invokeInt(library.queryClose, handle));
            handle = MemorySegment.NULL;
        }
    }

    /** Rejects operations after query ownership has been released. */
    private void ensureOpen() {
        if (handle == null || handle.address() == 0) throw new IllegalStateException("query is closed");
    }
}
