package io.adb.ffm;

import static org.junit.jupiter.api.Assertions.*;
import java.nio.*;
import org.junit.jupiter.api.Test;

/** Unit tests of {@link BatchDecoder}. */
class BatchDecoderTest {
    /**
     * Encodes a one-row batch with one INT64 column in slot 2.
     *
     * @param withRowId whether the row-id flag and the row id are written
     * @return the encoded batch
     */
    private static byte[] oneRow(boolean withRowId) {
        ByteBuffer b = ByteBuffer.allocate(16 + (withRowId ? 16 : 0) + 16 + 1 + 8)
            .order(ByteOrder.LITTLE_ENDIAN);
        b.putInt(0x41444242); // magic
        b.putShort((short) 2); b.putShort((short) (withRowId ? 1 : 0)); // version 2, flags
        b.putInt(1); b.putInt(1); // one row, one column
        if (withRowId) { b.putLong(7L); b.putLong(0L); } // u128 row id = 7
        b.putInt(2); b.put((byte) 2); b.put(new byte[]{0, 0, 0}); // slot 2, INT64
        b.putInt(1); b.putInt(8); // bitmap, payload lengths
        b.put((byte) 0); b.putLong(123L);
        return b.array();
    }

    /** A scan batch (row ids present) decodes to the expected row id, slot and value. */
    @Test void decodesInt64BatchWithRowIds() {
        var decoded = BatchDecoder.decode(oneRow(true));
        assertEquals(1, decoded.rowCount());
        assertEquals(7L, decoded.rowIds().getFirst().longValueExact());
        assertEquals(2, decoded.columns().getFirst().slotId());
        assertEquals(123L, decoded.column(2).orElseThrow().values().getFirst());
    }

    /** A derived batch (join/aggregate output) has rows but no row ids. */
    @Test void decodesBatchWithoutRowIds() {
        var decoded = BatchDecoder.decode(oneRow(false));
        assertEquals(1, decoded.rowCount());
        assertTrue(decoded.rowIds().isEmpty());
        assertTrue(decoded.column(9).isEmpty());
    }

    /** A column mixing NULL and non-NULL values decodes NULLs as {@code null} (LEFT JOIN output). */
    @Test void decodesNullValuesInAColumn() {
        ByteBuffer b = ByteBuffer.allocate(16 + (16 + 1 + 16) + (16 + 1 + 12 + 1)) // header, INT64 column, STRING column
            .order(ByteOrder.LITTLE_ENDIAN);
        b.putInt(0x41444242);
        b.putShort((short) 2); b.putShort((short) 0); // version 2, no row ids
        b.putInt(2); b.putInt(2); // two rows, two columns
        b.putInt(0); b.put((byte) 2); b.put(new byte[]{0, 0, 0}); // slot 0, INT64
        b.putInt(1); b.putInt(16);
        b.put((byte) 0b10); b.putLong(5L); b.putLong(0L); // row 1 is NULL
        b.putInt(1); b.put((byte) 4); b.put(new byte[]{0, 0, 0}); // slot 1, STRING
        b.putInt(1); b.putInt(12 + 1);
        b.put((byte) 0b01); b.putInt(0); b.putInt(0); b.putInt(1); b.put((byte) 'x'); // row 0 is NULL
        var decoded = BatchDecoder.decode(b.array());
        assertEquals(java.util.Arrays.asList(5L, null), decoded.column(0).orElseThrow().values());
        assertEquals(java.util.Arrays.asList(null, "x"), decoded.column(1).orElseThrow().values());
    }

    /** Version-1 batches and unknown flags are rejected. */
    @Test void rejectsOtherVersionsAndUnknownFlags() {
        byte[] v1 = oneRow(true);
        v1[4] = 1;
        assertThrows(IllegalArgumentException.class, () -> BatchDecoder.decode(v1));
        byte[] flags = oneRow(true);
        flags[6] = 3;
        assertThrows(IllegalArgumentException.class, () -> BatchDecoder.decode(flags));
    }
}
