package io.adb.ffm;
import java.math.BigInteger;
import java.util.List;
import java.util.Optional;

/**
 * One columnar batch of query results decoded from the native wire format.
 *
 * <p>A column that is NULL in every row of the batch is omitted by the engine; {@link #column}
 * returns empty for it and readers should treat its values as NULL.
 *
 * @param rowCount number of rows
 * @param rowIds   128-bit storage keys of the rows, in result order; empty for derived rows
 *                 (joins, aggregates), which have no storage identity
 * @param columns  decoded columns; each holds exactly {@code rowCount} values
 */
public record NativeRecordBatch(int rowCount, List<BigInteger> rowIds, List<NativeColumn> columns) {
    /**
     * Returns the column of an output slot, if the batch carries it.
     *
     * @param slotId output slot
     * @return the column, or empty when every value of the slot is NULL in this batch
     */
    public Optional<NativeColumn> column(int slotId) {
        return columns.stream().filter(c -> c.slotId() == slotId).findFirst();
    }
}
