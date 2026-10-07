package io.adb.ffm;
import java.util.List;

/**
 * One decoded column of a {@link NativeRecordBatch}.
 *
 * @param fieldId      engine field identifier of the column
 * @param physicalType wire type of the values
 * @param values       one value per row ({@code null} for SQL NULL); boxed according to {@code physicalType}
 */
public record NativeColumn(int fieldId, PhysicalType physicalType, List<Object> values) {}
