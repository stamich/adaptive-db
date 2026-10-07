package io.adb.ffm;

/** Failure reported by the native engine, carrying its ABI status and error message. */
public final class NativeException extends RuntimeException {
    /** Status code returned by the failing native call. */
    private final AdbStatus status;

    /**
     * Creates an exception for a failed native call.
     *
     * @param status  status returned by the call
     * @param message thread-local native error message
     */
    public NativeException(AdbStatus status, String message) {
        super(message);
        this.status = status;
    }

    /**
     * Returns the status of the failed call, so callers can distinguish e.g. a retryable
     * {@link AdbStatus#CONFLICT} from a fatal {@link AdbStatus#POISONED}.
     *
     * @return the native status
     */
    public AdbStatus status() { return status; }
}
