package io.adb.ffm;
public enum PhysicalType {
    BOOL(1), INT64(2), FLOAT64(3), STRING(4), BYTES(5);
    final int id;
    PhysicalType(int id) { this.id = id; }
    static PhysicalType fromId(int id) {
        for (var v : values()) if (v.id == id) return v;
        throw new IllegalArgumentException("unknown physical type " + id);
    }
}
