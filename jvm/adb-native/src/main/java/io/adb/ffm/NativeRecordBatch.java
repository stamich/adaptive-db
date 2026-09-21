package io.adb.ffm;
import java.math.BigInteger;
import java.util.List;
public record NativeRecordBatch(List<BigInteger> rowIds, List<NativeColumn> columns) {
    public int rowCount() { return rowIds.size(); }
}
