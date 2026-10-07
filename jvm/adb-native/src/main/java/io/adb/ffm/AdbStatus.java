package io.adb.ffm;

/** Status codes of the native C ABI (see {@code include/adb.h}); values are stable. */
public enum AdbStatus {
    OK(0), END_OF_STREAM(1), INVALID_ARGUMENT(2), CONFLICT(3), IO_ERROR(4),
    CORRUPTION(5), CANCELLED(6), NOT_FOUND(7),
    /** The native instance is poisoned (or a commit outcome is unknown): close and reopen. */
    POISONED(8),
    /** The requested change-log position is no longer retained. */
    LOG_TRUNCATED(9),
    INTERNAL(255);

    private final int code;
    AdbStatus(int code) { this.code = code; }
    public int code() { return code; }
    public static AdbStatus fromCode(int code) {
        for (var value : values()) if (value.code == code) return value;
        return INTERNAL;
    }
}
