package io.adb.ffm;
import java.util.List;
public record NativeColumn(int fieldId, PhysicalType physicalType, List<Object> values) {}
