package io.adb.ffm;

import static org.junit.jupiter.api.Assertions.*;

import java.lang.foreign.MemoryLayout.PathElement;
import java.lang.foreign.ValueLayout;
import org.junit.jupiter.api.Test;

/** Unit tests of {@link NativeQueryTelemetry}: the layout that must match the C structure. */
class NativeQueryTelemetryTest {
    /** The structure is four 64-bit fields, 32 bytes, as {@code AdbQueryTelemetry} in {@code adb.h}. */
    @Test void layoutMatchesTheCStructure() {
        assertEquals(32L, NativeQueryTelemetry.byteSize());
        assertEquals(8L, NativeQueryTelemetry.LAYOUT.byteAlignment());
        var layout = NativeQueryTelemetry.LAYOUT;
        assertEquals(0L, layout.byteOffset(PathElement.groupElement("output_rows")));
        assertEquals(8L, layout.byteOffset(PathElement.groupElement("rows_read")));
        assertEquals(16L, layout.byteOffset(PathElement.groupElement("batches")));
        assertEquals(24L, layout.byteOffset(PathElement.groupElement("peak_memory_bytes")));
    }

    /** A new holder reads all zeros, and each accessor reads its own field. */
    @Test void accessorsReadTheirOwnFields() {
        var holder = new NativeQueryTelemetry();
        assertEquals(0L, holder.outputRows());
        assertEquals(0L, holder.rowsRead());
        assertEquals(0L, holder.batches());
        assertEquals(0L, holder.peakMemoryBytes());
        holder.segment().set(ValueLayout.JAVA_LONG, 0L, 11L);
        holder.segment().set(ValueLayout.JAVA_LONG, 8L, 22L);
        holder.segment().set(ValueLayout.JAVA_LONG, 16L, 33L);
        holder.segment().set(ValueLayout.JAVA_LONG, 24L, 44L);
        assertEquals(11L, holder.outputRows());
        assertEquals(22L, holder.rowsRead());
        assertEquals(33L, holder.batches());
        assertEquals(44L, holder.peakMemoryBytes());
    }

    /** Two holders own separate native memory. */
    @Test void holdersDoNotShareMemory() {
        var a = new NativeQueryTelemetry();
        var b = new NativeQueryTelemetry();
        a.segment().set(ValueLayout.JAVA_LONG, 0L, 5L);
        assertEquals(0L, b.outputRows());
    }
}
