package io.adb.ffm;

import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

/** Loads the Adaptive DB shared library through the Java FFM API, binds every C ABI v3
 * function as a downcall handle, and translates native status codes into exceptions. */
final class NativeLibrary implements AutoCloseable {
    /** Upper bound on the length of a native error message. */
    private static final long MAX_ERROR_BYTES = 1024 * 1024;
    /** Whether {@link #close()} already released the arena. */
    private boolean closed;
    /** C ABI version this binding was written against; any other version is rejected. */
    static final int EXPECTED_ABI = 3;

    /** Shared arena that keeps the library mapped until {@link #close()}. */
    final Arena arena = Arena.ofShared();
    /** Platform linker used to create downcall handles. */
    final Linker linker = Linker.nativeLinker();
    /** Symbol table of the loaded library. */
    final SymbolLookup lookup;

    /** {@code adb_abi_version}: ABI version of the loaded library. */
    final MethodHandle abiVersion;
    /** {@code adb_last_error_ptr}: pointer to the thread-local error message. */
    final MethodHandle lastErrorPtr;
    /** {@code adb_last_error_len}: byte length of the thread-local error message. */
    final MethodHandle lastErrorLen;
    /** {@code adb_open}: opens a database directory. */
    final MethodHandle open;
    /** {@code adb_close}: closes a database handle. */
    final MethodHandle close;
    /** {@code adb_latest_committed_ts}: newest committed MVCC timestamp. */
    final MethodHandle latestCommittedTs;
    /** {@code adb_execute_plan_json}: runs a JSON plan at the latest snapshot. */
    final MethodHandle execute;
    /** {@code adb_execute_plan_json_at}: runs a JSON plan at a given snapshot. */
    final MethodHandle executeAt;
    /** {@code adb_query_next_batch}: fetches the next result batch of a query. */
    final MethodHandle queryNextBatch;
    /** {@code adb_query_cancel}: cancels a running query. */
    final MethodHandle queryCancel;
    /** {@code adb_query_close}: releases a query cursor. */
    final MethodHandle queryClose;
    /** {@code adb_batch_data}: data pointer of a native byte buffer. */
    final MethodHandle batchData;
    /** {@code adb_batch_len}: length of a native byte buffer. */
    final MethodHandle batchLen;
    /** {@code adb_batch_release}: frees a native byte buffer. */
    final MethodHandle batchRelease;
    /** {@code adb_insert_row_json}: inserts one row. */
    final MethodHandle insertRow;
    /** {@code adb_update_fields_json}: updates fields of one row. */
    final MethodHandle updateFields;
    /** {@code adb_delete_row}: deletes one row. */
    final MethodHandle deleteRow;
    /** {@code adb_read_changes_json}: reads committed change events from a log cursor. */
    final MethodHandle readChanges;
    /** {@code adb_change_feed_end}: cursor just past the newest committed change. */
    final MethodHandle changeFeedEnd;
    /** {@code adb_commit_consumer_offset}: durably stores a consumer's cursor. */
    final MethodHandle commitConsumerOffset;
    /** {@code adb_consumer_offset}: loads a consumer's stored cursor. */
    final MethodHandle consumerOffset;
    /** {@code adb_checkpoint}: flushes projections and truncates replay. */
    final MethodHandle checkpoint;
    /** {@code adb_vacuum}: removes tombstones below the snapshot horizon. */
    final MethodHandle vacuum;

    /**
     * Loads the library, binds every downcall, and verifies the ABI version.
     *
     * @param libraryPath path of {@code libadb_ffi}
     * @throws IllegalStateException if a symbol is missing or the ABI version differs
     */
    NativeLibrary(Path libraryPath) {
        lookup = SymbolLookup.libraryLookup(libraryPath, arena);
        abiVersion = downcall("adb_abi_version", FunctionDescriptor.of(ValueLayout.JAVA_INT));
        lastErrorPtr = downcall("adb_last_error_ptr", FunctionDescriptor.of(ValueLayout.ADDRESS));
        lastErrorLen = downcall("adb_last_error_len", FunctionDescriptor.of(ValueLayout.JAVA_LONG));
        open = downcall("adb_open", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        close = downcall("adb_close", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        latestCommittedTs = downcall("adb_latest_committed_ts", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        execute = downcall("adb_execute_plan_json", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        executeAt = downcall("adb_execute_plan_json_at", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        queryNextBatch = downcall("adb_query_next_batch", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        queryCancel = downcall("adb_query_cancel", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        queryClose = downcall("adb_query_close", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        batchData = downcall("adb_batch_data", FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        batchLen = downcall("adb_batch_len", FunctionDescriptor.of(ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        batchRelease = downcall("adb_batch_release", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        insertRow = downcall("adb_insert_row_json", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        updateFields = downcall("adb_update_fields_json", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        deleteRow = downcall("adb_delete_row", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));

        readChanges = downcall("adb_read_changes_json", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        changeFeedEnd = downcall("adb_change_feed_end", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        commitConsumerOffset = downcall("adb_commit_consumer_offset", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG));
        consumerOffset = downcall("adb_consumer_offset", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS));
        checkpoint = downcall("adb_checkpoint", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        vacuum = downcall("adb_vacuum", FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS));

        int abi = invokeInt(abiVersion);
        if (abi != EXPECTED_ABI) throw new IllegalStateException("Expected adb ABI " + EXPECTED_ABI + " but got " + abi);
    }

    /**
     * Creates a downcall handle for an exported symbol.
     *
     * @param name       C symbol name
     * @param descriptor native signature
     * @return the bound handle
     * @throws IllegalStateException if the symbol is not exported
     */
    private MethodHandle downcall(String name, FunctionDescriptor descriptor) {
        var symbol = lookup.find(name).orElseThrow(() -> new IllegalStateException("Missing native symbol: " + name));
        return linker.downcallHandle(symbol, descriptor);
    }

    /**
     * Invokes a native function returning {@code int} (usually an {@link AdbStatus} code).
     *
     * @param handle downcall handle
     * @param args   native arguments
     * @return the native return value
     */
    int invokeInt(MethodHandle handle, Object... args) {
        try { return (int) handle.invokeWithArguments(args); }
        catch (Throwable t) { throw new RuntimeException(t); }
    }

    /**
     * Invokes a native function returning {@code int64_t}/{@code size_t}.
     *
     * @param handle downcall handle
     * @param args   native arguments
     * @return the native return value
     */
    long invokeLong(MethodHandle handle, Object... args) {
        try { return (long) handle.invokeWithArguments(args); }
        catch (Throwable t) { throw new RuntimeException(t); }
    }

    /**
     * Invokes a native function returning a pointer.
     *
     * @param handle downcall handle
     * @param args   native arguments
     * @return the returned pointer as a zero-length segment
     */
    MemorySegment invokeAddress(MethodHandle handle, Object... args) {
        try { return (MemorySegment) handle.invokeWithArguments(args); }
        catch (Throwable t) { throw new RuntimeException(t); }
    }

    /** Copies a native byte buffer ({@code AdbBatchHandle}) into Java and releases it. */
    byte[] takeBuffer(MemorySegment bufferHandle, long maxBytes) {
        if (bufferHandle.address() == 0) throw new IllegalStateException("native call returned OK without a buffer");
        try {
            long len = invokeLong(batchLen, bufferHandle);
            if (len <= 0 || len > maxBytes) throw new IllegalStateException("invalid native buffer length " + len);
            var dataPtr = invokeAddress(batchData, bufferHandle);
            if (dataPtr.address() == 0) throw new IllegalStateException("null native buffer data pointer");
            return dataPtr.reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
        } finally {
            check(invokeInt(batchRelease, bufferHandle));
        }
    }

    /**
     * Throws if a native call did not return {@link AdbStatus#OK}.
     *
     * @param code status returned by the native call
     * @throws NativeException carrying the status and the native error message
     */
    void check(int code) {
        var status = AdbStatus.fromCode(code);
        if (status != AdbStatus.OK) throw new NativeException(status, lastError());
    }

    /**
     * Reads the calling thread's last native error message.
     *
     * @return the message, or a placeholder if it is missing or oversized
     */
    String lastError() {
        long len = invokeLong(lastErrorLen);
        if (len <= 0 || len > MAX_ERROR_BYTES) return "native error (invalid error-buffer length)";
        var ptr = invokeAddress(lastErrorPtr);
        if (ptr.address() == 0) return "native error";
        return new String(ptr.reinterpret(len).toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    /** Unloads the library; idempotent. Handles created from it must not be used afterwards. */
    @Override public synchronized void close() {
        if (!closed) {
            arena.close();
            closed = true;
        }
    }
}
