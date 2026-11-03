package io.adb.ffm;

/**
 * Identity of one commit: its MVCC timestamp and its position in the global commit order.
 *
 * <p>The sequence number is what a session remembers to read its own writes later: data and DDL
 * commits share one order, so "everything up to {@code commitSeq}" is well defined.
 *
 * @param commitTs  MVCC commit timestamp (the snapshot at which the commit became visible)
 * @param commitSeq position in the global commit order, starting at 1; 0 means "before the first commit"
 */
public record NativeCommitResult(long commitTs, long commitSeq) {}
