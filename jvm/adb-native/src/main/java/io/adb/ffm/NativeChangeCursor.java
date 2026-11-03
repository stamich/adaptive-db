package io.adb.ffm;

import java.lang.foreign.*;
import java.nio.charset.StandardCharsets;
import java.util.Optional;

/**
 * Owns one native change-log cursor (ABI 8) and exposes bounded pages of committed changes.
 *
 * <p>The cursor delivers each commit exactly once, in commit order, and remembers where it is.
 * Close it when done; closing twice is harmless.
 */
public final class NativeChangeCursor implements AutoCloseable {
    /** Largest accepted batch document, in bytes (the native response limit plus headroom). */
    private static final long MAX_BATCH_JSON_BYTES = 64L * 1024 * 1024;
    /** Native library that owns the downcall handles. */
    private final NativeLibrary library;
    /** Native cursor handle; {@code MemorySegment.NULL} once closed. */
    private MemorySegment handle;

    /** Wraps a non-null cursor handle returned by the native ABI. */
    NativeChangeCursor(NativeLibrary library, MemorySegment handle) {
        this.library = library;
        this.handle = handle;
        if (handle == null || handle.address() == 0) throw new IllegalArgumentException("null native change cursor handle");
    }

    /**
     * Returns the next page, or empty when the consumer is caught up.
     *
     * @param maxCommits most commits in the page, 1..10000
     * @param maxBytes   about how many bytes of change data the page may hold, 1..64 MiB; a page
     *                   always holds at least one commit when one is available
     * @return the page, or empty at the end of the log
     * @throws NativeException with {@link AdbStatus#LOG_TRUNCATED} if the commits were pruned
     */
    public synchronized Optional<NativeChangeBatch> nextBatch(int maxCommits, long maxBytes) {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.ADDRESS);
            out.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
            int code = library.invokeInt(library.changeCursorNextBatch, handle, maxCommits, maxBytes, out);
            if (AdbStatus.fromCode(code) == AdbStatus.END_OF_STREAM) return Optional.empty();
            library.check(code);
            byte[] json = library.takeBuffer(out.get(ValueLayout.ADDRESS, 0), MAX_BATCH_JSON_BYTES);
            return Optional.of(new NativeChangeBatch(new String(json, StandardCharsets.UTF_8)));
        }
    }

    /** The last commit sequence number this cursor delivered (its starting point before the first page). */
    public synchronized long position() {
        ensureOpen();
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(ValueLayout.JAVA_LONG);
            library.check(library.invokeInt(library.changeCursorPosition, handle, out));
            return out.get(ValueLayout.JAVA_LONG, 0);
        }
    }

    /** Releases the native cursor exactly once. */
    @Override public synchronized void close() {
        if (handle != null && handle.address() != 0) {
            library.check(library.invokeInt(library.changeCursorClose, handle));
            handle = MemorySegment.NULL;
        }
    }

    /** Rejects operations after ownership has been released. */
    private void ensureOpen() {
        if (handle == null || handle.address() == 0) throw new IllegalStateException("change cursor is closed");
    }
}
