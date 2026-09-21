package io.adb.ffm;

import java.lang.foreign.*;
import java.util.Optional;

/** Owns one native query cursor and exposes bounded decoded batches. */
public final class NativeQuery implements AutoCloseable {
    private static final long MAX_BATCH_BYTES = 64L * 1024 * 1024;
    private final NativeLibrary library;
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
            var batchHandle = out.get(ValueLayout.ADDRESS, 0);
            if (batchHandle.address() == 0) throw new IllegalStateException("native query returned OK without a batch handle");
            try {
                long len = library.invokeLong(library.batchLen, batchHandle);
                if (len <= 0 || len > MAX_BATCH_BYTES) throw new IllegalStateException("invalid native batch length " + len);
                var dataPtr = library.invokeAddress(library.batchData, batchHandle);
                if (dataPtr.address() == 0) throw new IllegalStateException("null native batch data pointer");
                byte[] bytes = dataPtr.reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
                return Optional.of(BatchDecoder.decode(bytes));
            } finally {
                library.check(library.invokeInt(library.batchRelease, batchHandle));
            }
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
