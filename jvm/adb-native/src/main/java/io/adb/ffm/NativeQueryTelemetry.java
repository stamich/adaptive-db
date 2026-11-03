package io.adb.ffm;

import java.lang.foreign.Arena;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/**
 * Reusable holder of the counters {@code adb_query_telemetry} writes (C ABI 9).
 *
 * <p>The native structure is four 64-bit integers: rows returned, rows read from storage, batches
 * returned and peak memory. An instance owns one native segment of that layout and is filled by
 * {@link NativeQuery#readTelemetry(NativeQueryTelemetry)} and read back with primitive accessors.
 * Creating it once and reusing it means a captured query needs no JSON document, no
 * {@code String} and no per-call native allocation, which is the point of the call.</p>
 *
 * <p>Instances are <strong>not</strong> thread safe: use one per thread (the SQL gateway keeps a
 * {@link ThreadLocal}). The segment is owned by an automatic arena, so it is released when the
 * holder is garbage collected.</p>
 */
public final class NativeQueryTelemetry {
    /** Exact layout of the C structure {@code AdbQueryTelemetry}: four 64-bit fields, no padding. */
    static final MemoryLayout LAYOUT = MemoryLayout.structLayout(
        ValueLayout.JAVA_LONG.withName("output_rows"),
        ValueLayout.JAVA_LONG.withName("rows_read"),
        ValueLayout.JAVA_LONG.withName("batches"),
        ValueLayout.JAVA_LONG.withName("peak_memory_bytes"));

    /** Byte offset of {@code output_rows}. */
    private static final long OUTPUT_ROWS = 0L;
    /** Byte offset of {@code rows_read}. */
    private static final long ROWS_READ = 8L;
    /** Byte offset of {@code batches}. */
    private static final long BATCHES = 16L;
    /** Byte offset of {@code peak_memory_bytes}. */
    private static final long PEAK_MEMORY_BYTES = 24L;

    /** Native memory the library fills; zero-initialised by the arena. */
    private final MemorySegment segment = Arena.ofAuto().allocate(LAYOUT);

    /** Creates an empty holder (all counters zero). */
    public NativeQueryTelemetry() {}

    /** Returns the native segment passed to {@code adb_query_telemetry}; package-private on purpose. */
    MemorySegment segment() { return segment; }

    /** Returns the size in bytes of the native structure (32). */
    public static long byteSize() { return LAYOUT.byteSize(); }

    /** Returns the rows returned to the caller, as of the last read. */
    public long outputRows() { return segment.get(ValueLayout.JAVA_LONG, OUTPUT_ROWS); }

    /** Returns the rows the plan's leaf operators produced (rows read from storage). */
    public long rowsRead() { return segment.get(ValueLayout.JAVA_LONG, ROWS_READ); }

    /** Returns the batches returned to the caller. */
    public long batches() { return segment.get(ValueLayout.JAVA_LONG, BATCHES); }

    /** Returns the highest number of bytes the query's blocking operators held at once. */
    public long peakMemoryBytes() { return segment.get(ValueLayout.JAVA_LONG, PEAK_MEMORY_BYTES); }
}
