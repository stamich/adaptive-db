package io.adb.ffm;

public final class NativeException extends RuntimeException {
    private final AdbStatus status;
    public NativeException(AdbStatus status, String message) {
        super(message);
        this.status = status;
    }
    public AdbStatus status() { return status; }
}
