package io.adb.ffm;

/** How {@link NativeDatabase#waitForProjection} ended; the codes are those of the C ABI. */
public enum NativeWaitOutcome {
    /** The projection is ready and has consumed the requested commit. */
    REACHED(0),
    /** The time limit elapsed first. */
    TIMED_OUT(1),
    /** The projection failed or needs a rebuild: waiting longer cannot help. */
    NOT_READABLE(2),
    /** The projection is not registered. */
    UNKNOWN(3);

    /** Numeric value used on the C ABI. */
    private final int code;

    /** Binds a constant to its C ABI value. */
    NativeWaitOutcome(int code) { this.code = code; }

    /**
     * Maps the numeric outcome returned by the native library.
     *
     * @param code outcome code
     * @return the matching constant
     * @throws IllegalStateException for a code this binding does not know
     */
    static NativeWaitOutcome fromCode(int code) {
        for (var value : values()) if (value.code == code) return value;
        throw new IllegalStateException("unknown projection wait outcome " + code);
    }
}
