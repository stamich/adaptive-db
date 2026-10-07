package io.adb.ffm;
import java.util.List;

/**
 * One decoded column of a {@link NativeRecordBatch}.
 *
 * <p>Since batch format v2 a column is identified by the query's output slot (assigned by the
 * planner), not by a storage field id: a join may output two columns with the same field id.
 *
 * @param slotId       output slot of the column
 * @param physicalType wire type of the values
 * @param values       one value per row ({@code null} for SQL NULL); boxed according to {@code physicalType}
 */
public record NativeColumn(int slotId, PhysicalType physicalType, List<Object> values) {}
