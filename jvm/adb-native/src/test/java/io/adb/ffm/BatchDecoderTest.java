package io.adb.ffm;

import static org.junit.jupiter.api.Assertions.*;
import java.nio.*;
import org.junit.jupiter.api.Test;

class BatchDecoderTest {
    @Test void decodesInt64Batch() {
        ByteBuffer b = ByteBuffer.allocate(16 + 16 + 16 + 1 + 8)
            .order(ByteOrder.LITTLE_ENDIAN);
        b.putInt(0x41444242); // magic
        b.putShort((short)1); b.putShort((short)0);
        b.putInt(1); b.putInt(1); // one row, one column
        b.putLong(7L); b.putLong(0L); // u128 row id = 7
        b.putInt(2); b.put((byte)2); b.put(new byte[]{0,0,0}); // field=2, INT64
        b.putInt(1); b.putInt(8); // bitmap, payload lengths
        b.put((byte)0); b.putLong(123L);

        var decoded = BatchDecoder.decode(b.array());
        assertEquals(1, decoded.rowCount());
        assertEquals(7, decoded.rowIds().getFirst().longValueExact());
        assertEquals(123L, decoded.columns().getFirst().values().getFirst());
    }
}
