package io.adb.ffm;

import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

final class NativeLibrary implements AutoCloseable {
    private static final long MAX_ERROR_BYTES = 1024 * 1024;
    private boolean closed;
    static final int EXPECTED_ABI = 3;

    final Arena arena = Arena.ofShared();
    final Linker linker = Linker.nativeLinker();
    final SymbolLookup lookup;

    final MethodHandle abiVersion;
    final MethodHandle lastErrorPtr;
    final MethodHandle lastErrorLen;
    final MethodHandle open;
    final MethodHandle close;
    final MethodHandle latestCommittedTs;
    final MethodHandle execute;
    final MethodHandle executeAt;
    final MethodHandle queryNextBatch;
    final MethodHandle queryCancel;
    final MethodHandle queryClose;
    final MethodHandle batchData;
    final MethodHandle batchLen;
    final MethodHandle batchRelease;
    final MethodHandle insertRow;
    final MethodHandle updateFields;
    final MethodHandle deleteRow;
    final MethodHandle readChanges;
    final MethodHandle changeFeedEnd;
    final MethodHandle commitConsumerOffset;
    final MethodHandle consumerOffset;
    final MethodHandle checkpoint;
    final MethodHandle vacuum;

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

    private MethodHandle downcall(String name, FunctionDescriptor descriptor) {
        var symbol = lookup.find(name).orElseThrow(() -> new IllegalStateException("Missing native symbol: " + name));
        return linker.downcallHandle(symbol, descriptor);
    }

    int invokeInt(MethodHandle handle, Object... args) {
        try { return (int) handle.invokeWithArguments(args); }
        catch (Throwable t) { throw new RuntimeException(t); }
    }

    long invokeLong(MethodHandle handle, Object... args) {
        try { return (long) handle.invokeWithArguments(args); }
        catch (Throwable t) { throw new RuntimeException(t); }
    }

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

    void check(int code) {
        var status = AdbStatus.fromCode(code);
        if (status != AdbStatus.OK) throw new NativeException(status, lastError());
    }

    String lastError() {
        long len = invokeLong(lastErrorLen);
        if (len <= 0 || len > MAX_ERROR_BYTES) return "native error (invalid error-buffer length)";
        var ptr = invokeAddress(lastErrorPtr);
        if (ptr.address() == 0) return "native error";
        return new String(ptr.reinterpret(len).toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    @Override public synchronized void close() {
        if (!closed) {
            arena.close();
            closed = true;
        }
    }
}
