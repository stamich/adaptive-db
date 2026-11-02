package io.adb.ffm;

/** States an external projection can be put into with {@link NativeDatabase#setProjectionState}. */
public enum NativeProjectionState {
    /** Being (re)built from commit 0; not readable. */
    BUILDING(0),
    /** Readable up to its frontier. */
    READY(1),
    /** The change log no longer holds what it needs; it must be rebuilt. */
    NEEDS_REBUILD(2),
    /** Stopped after an error. */
    FAILED(3);

    /** Numeric value used on the C ABI. */
    final int code;

    /** Binds a constant to its C ABI value. */
    NativeProjectionState(int code) { this.code = code; }
}
