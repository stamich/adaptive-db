package io.adb.ffm;

import java.util.Objects;

/**
 * One page of the change log as delivered by {@link NativeChangeCursor#nextBatch}.
 *
 * <p>The document is UTF-8 JSON (format in {@code docs/change-log.md}): {@code first_commit_seq},
 * {@code last_commit_seq}, {@code bytes} and the {@code commits} array. The binding does not
 * interpret it; decoding belongs to the consumer.
 *
 * @param json the batch document
 */
public record NativeChangeBatch(String json) {
    /** Rejects a missing document. */
    public NativeChangeBatch {
        Objects.requireNonNull(json, "json");
    }
}
