package io.adb.ffm;

/** Status codes of the native C ABI (see {@code include/adb.h}); values are stable. */
public enum AdbStatus {
    /** The call succeeded. */
    OK(0),
    /** A query or cursor has no more data. */
    END_OF_STREAM(1),
    /** An argument was null, malformed, out of range, or not valid UTF-8/JSON. */
    INVALID_ARGUMENT(2),
    /** A write-write or serialization conflict, or a duplicate primary key; retry the transaction. */
    CONFLICT(3),
    /** The operating system reported an I/O failure. */
    IO_ERROR(4),
    /** Persisted data failed validation (checksum, framing, or structural check). */
    CORRUPTION(5),
    /** The operation was cancelled by the caller. */
    CANCELLED(6),
    /** The requested row, entity, or consumer does not exist. */
    NOT_FOUND(7),
    /** The native instance is poisoned (or a commit outcome is unknown): close and reopen. */
    POISONED(8),
    /** The requested change-log position is no longer retained. */
    LOG_TRUNCATED(9),
    /** Unexpected engine failure, or a code this binding does not know. */
    INTERNAL(255);

    /** Numeric value used on the C ABI. */
    private final int code;

    /**
     * Binds a constant to its C ABI value.
     *
     * @param code numeric status code
     */
    AdbStatus(int code) { this.code = code; }

    /**
     * Returns the numeric C ABI value of this status.
     *
     * @return the status code
     */
    public int code() { return code; }

    /**
     * Maps a numeric status returned by the native library to its constant.
     *
     * @param code numeric status code
     * @return the matching constant, or {@link #INTERNAL} for unknown codes
     */
    public static AdbStatus fromCode(int code) {
        for (var value : values()) if (value.code == code) return value;
        return INTERNAL;
    }
}
