package io.adb.ffm;

import java.math.BigInteger;
import java.nio.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

/**
 * Strict decoder for the bounded ADB Batch Format v2 returned by the Rust engine.
 *
 * <p>v2 differs from v1 in the header only: the former reserved word is a flags word and the
 * per-row 16-byte row ids are present only with {@link #FLAG_ROW_IDS}. Column ids are output
 * slots. See {@code docs/batch-format.md}.
 */
final class BatchDecoder {
    /** Magic prefix {@code "ADBB"} (little-endian) of every batch. */
    static final int MAGIC = 0x41444242;
    /** Supported batch format version. */
    static final int VERSION = 2;
    /** Header flag: row ids follow the header. */
    static final int FLAG_ROW_IDS = 0x0001;
    /** Largest accepted encoded batch, in bytes. */
    static final int MAX_BATCH_BYTES = 64 * 1024 * 1024;
    /** Largest accepted row count per batch. */
    static final int MAX_ROWS = 1_000_000;
    /** Largest accepted column count per batch. */
    static final int MAX_COLUMNS = 4096;

    /** Decodes one batch after validating all counts, lengths, offsets and remaining bytes. */
    static NativeRecordBatch decode(byte[] bytes) {
        Objects.requireNonNull(bytes, "bytes");
        if (bytes.length < 16 || bytes.length > MAX_BATCH_BYTES) throw new IllegalArgumentException("invalid batch length " + bytes.length);
        ByteBuffer b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        if (readInt(b, "magic") != MAGIC) throw new IllegalArgumentException("bad batch magic");
        int version = Short.toUnsignedInt(readShort(b, "version"));
        if (version != VERSION) throw new IllegalArgumentException("unsupported batch version " + version);
        int flags = Short.toUnsignedInt(readShort(b, "flags"));
        if ((flags & ~FLAG_ROW_IDS) != 0) throw new IllegalArgumentException("unknown batch flags " + flags);
        boolean hasRowIds = (flags & FLAG_ROW_IDS) != 0;
        int rows = readInt(b, "row count");
        int columns = readInt(b, "column count");
        if (rows < 0 || rows > MAX_ROWS) throw new IllegalArgumentException("invalid row count " + rows);
        if (columns < 0 || columns > MAX_COLUMNS) throw new IllegalArgumentException("invalid column count " + columns);

        int rowIdCount = hasRowIds ? rows : 0;
        requireRemaining(b, Math.multiplyExact(rowIdCount, 16), "row ids");
        List<BigInteger> rowIds = new ArrayList<>(rowIdCount);
        for (int i = 0; i < rowIdCount; i++) {
            byte[] le = new byte[16]; b.get(le);
            byte[] be = new byte[16];
            for (int j = 0; j < 16; j++) be[j] = le[15-j];
            rowIds.add(new BigInteger(1, be));
        }

        List<NativeColumn> decoded = new ArrayList<>(columns);
        for (int c = 0; c < columns; c++) {
            requireRemaining(b, 16, "column header");
            int slotId = b.getInt();
            PhysicalType type = PhysicalType.fromId(Byte.toUnsignedInt(b.get()));
            b.get(); b.get(); b.get();
            int bitmapLen = b.getInt();
            int payloadLen = b.getInt();
            int expectedBitmap = (rows + 7) / 8;
            if (bitmapLen != expectedBitmap) throw new IllegalArgumentException("invalid null bitmap length " + bitmapLen);
            if (payloadLen < 0 || payloadLen > MAX_BATCH_BYTES) throw new IllegalArgumentException("invalid column payload length " + payloadLen);
            requireRemaining(b, Math.addExact(bitmapLen, payloadLen), "column body");
            byte[] bitmap = new byte[bitmapLen]; b.get(bitmap);
            byte[] payload = new byte[payloadLen]; b.get(payload);
            decoded.add(new NativeColumn(slotId, type, decodeColumn(type, rows, bitmap, payload)));
        }
        if (b.hasRemaining()) throw new IllegalArgumentException("trailing bytes after batch");
        return new NativeRecordBatch(rows, List.copyOf(rowIds), List.copyOf(decoded));
    }

    /** Decodes one typed column while validating fixed widths and variable offsets. */
    private static List<Object> decodeColumn(PhysicalType type, int rows, byte[] bitmap, byte[] payload) {
        ByteBuffer p = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN);
        List<Object> values = new ArrayList<>(rows);
        if (type == PhysicalType.STRING || type == PhysicalType.BYTES) {
            int offsetBytes = Math.multiplyExact(rows + 1, 4);
            requireRemaining(p, offsetBytes, "variable column offsets");
            int[] offsets = new int[rows + 1];
            for (int i = 0; i <= rows; i++) offsets[i] = p.getInt();
            int dataLength = payload.length - offsetBytes;
            if (offsets[0] != 0 || offsets[rows] != dataLength) throw new IllegalArgumentException("invalid variable column boundary offsets");
            for (int i = 0; i < rows; i++) {
                int start = offsets[i], end = offsets[i + 1];
                if (start < 0 || end < start || end > dataLength) throw new IllegalArgumentException("invalid variable column offsets");
                if (isNull(bitmap, i)) { values.add(null); continue; }
                byte[] value = Arrays.copyOfRange(payload, offsetBytes + start, offsetBytes + end);
                values.add(type == PhysicalType.STRING ? new String(value, StandardCharsets.UTF_8) : value);
            }
            return List.copyOf(values);
        }

        int width = switch (type) { case BOOL -> 1; case INT64, FLOAT64 -> 8; default -> throw new IllegalStateException(); };
        if (payload.length != Math.multiplyExact(rows, width)) throw new IllegalArgumentException("invalid fixed-width column payload");
        for (int i = 0; i < rows; i++) {
            Object value = switch (type) { case BOOL -> p.get() != 0; case INT64 -> p.getLong(); case FLOAT64 -> p.getDouble(); default -> throw new IllegalStateException(); };
            values.add(isNull(bitmap, i) ? null : value);
        }
        return List.copyOf(values);
    }

    /** Returns whether the requested row is marked null in a validated bitmap. */
    private static boolean isNull(byte[] bitmap, int index) { return (bitmap[index / 8] & (1 << (index % 8))) != 0; }

    /** Reads an integer after verifying that the buffer still contains it. */
    private static int readInt(ByteBuffer b, String what) { requireRemaining(b, Integer.BYTES, what); return b.getInt(); }

    /** Reads a short after verifying that the buffer still contains it. */
    private static short readShort(ByteBuffer b, String what) { requireRemaining(b, Short.BYTES, what); return b.getShort(); }

    /** Rejects truncated wire input before a relative ByteBuffer read can underflow. */
    private static void requireRemaining(ByteBuffer b, int required, String what) {
        if (required < 0 || b.remaining() < required) throw new IllegalArgumentException("truncated " + what);
    }
}
