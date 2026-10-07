package io.adb.ffm;

/** Physical value types of the native columnar batch format. */
public enum PhysicalType {
    /** Boolean, one byte per value. */
    BOOL(1),
    /** Signed 64-bit integer, decoded as {@link Long}. */
    INT64(2),
    /** IEEE-754 double, decoded as {@link Double}. */
    FLOAT64(3),
    /** UTF-8 string, decoded as {@link String}. */
    STRING(4),
    /** Opaque bytes, decoded as {@code byte[]}. */
    BYTES(5);

    /** Type tag used in the wire format. */
    final int id;

    /**
     * Binds a constant to its wire tag.
     *
     * @param id wire type tag
     */
    PhysicalType(int id) { this.id = id; }

    /**
     * Maps a wire type tag to its constant.
     *
     * @param id wire type tag
     * @return the matching type
     * @throws IllegalArgumentException if the tag is unknown
     */
    static PhysicalType fromId(int id) {
        for (var v : values()) if (v.id == id) return v;
        throw new IllegalArgumentException("unknown physical type " + id);
    }
}
