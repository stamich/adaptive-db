# PhysicalPlan Wire Format v1

Milestone 1.7 uses UTF-8 JSON only at the FFI boundary. This deliberately avoids
binding the Rust execution engine to a transport-specific generated type.

Examples:

```json
{"op":"point_lookup","row_id":1}
```

`row_id` may also be sent as a decimal JSON string when the identifier exceeds
the portable unsigned-64-bit JSON number range, for example:

```json
{"op":"point_lookup","row_id":"340282366920938463463374607431768211455"}
```

The Rust engine keeps `RowId` as `u128`; the FFI JSON decoder deliberately uses
a separate wire representation so JSON transport limitations do not alter the
engine-native ID type or its binary persistence formats.

Table scans (Milestone 2.0.3) read one entity's key range
`[entity_id << 64, (entity_id + 1) << 64)` and stream it in batches:

```json
{"op":"entity_scan","entity_id":7}
```

`{"op":"scan"}` still exists and scans every entity; it is meant for diagnostics only.

```json
{
  "op":"limit",
  "input": {
    "op":"filter",
    "input":{"op":"entity_scan","entity_id":7},
    "predicate": {
      "kind":"binary",
      "left":{"kind":"column","field_id":1},
      "op":"gt",
      "right":{"kind":"literal","value":{"Int64":100}}
    }
  },
  "limit":20
}
```

This is a bootstrap protocol, not the planned long-term cluster protocol.
Milestone 2.0 should introduce protobuf (or an equivalent versioned binary IR)
and convert it into the same Rust-native `PhysicalPlan` enum before execution.
