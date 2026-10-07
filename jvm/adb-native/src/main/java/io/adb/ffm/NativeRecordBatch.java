package io.adb.ffm;
import java.math.BigInteger;
import java.util.List;

/**
 * One columnar batch of query results decoded from the native wire format.
 *
 * @param rowIds  128-bit storage keys of the rows, in result order
 * @param columns decoded columns; each holds exactly {@code rowIds.size()} values
 */
public record NativeRecordBatch(List<BigInteger> rowIds, List<NativeColumn> columns) {
    /**
     * Returns the number of rows in the batch.
     *
     * @return row count
     */
    public int rowCount() { return rowIds.size(); }
}
