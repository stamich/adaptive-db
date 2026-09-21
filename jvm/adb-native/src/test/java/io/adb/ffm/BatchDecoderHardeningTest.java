package io.adb.ffm;

import static org.junit.jupiter.api.Assertions.assertThrows;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import org.junit.jupiter.api.Test;

/** Regression tests proving malformed native batch metadata is rejected rather than over-allocated. */
final class BatchDecoderHardeningTest {
    /** Rejects a negative/on-wire huge row count before ArrayList allocation. */
    @Test void negativeRowCountIsRejected() {
        byte[] bytes = new byte[16];
        ByteBuffer b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        b.putInt(BatchDecoder.MAGIC).putShort((short) BatchDecoder.VERSION).putShort((short) 0).putInt(-1).putInt(0);
        assertThrows(IllegalArgumentException.class, () -> BatchDecoder.decode(bytes));
    }

    /** Rejects a truncated column body before ByteBuffer underflow can escape. */
    @Test void truncatedColumnIsRejected() {
        byte[] bytes = new byte[16 + 16];
        ByteBuffer b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        b.putInt(BatchDecoder.MAGIC).putShort((short) BatchDecoder.VERSION).putShort((short) 0).putInt(0).putInt(1);
        b.putInt(1).put((byte) 2).put(new byte[3]).putInt(0).putInt(1024);
        assertThrows(IllegalArgumentException.class, () -> BatchDecoder.decode(bytes));
    }
}
