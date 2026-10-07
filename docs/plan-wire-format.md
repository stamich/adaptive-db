# Physical plan wire format v2

The JVM planner sends physical plans to the native engine as UTF-8 JSON through
`adb_execute_plan_json[_at]`. Version 2 (Milestone 2.1) wraps the plan in an envelope:

```json
{"wire_version": 2, "plan": { ...plan node... }}
```

`adb-plan-wire::decode_json` is the only native entry point. It

1. rejects empty documents and documents over **8 MiB**;
2. reads `wire_version` **before** interpreting the plan: a 2.0-style bare plan is reported as
   "unsupported plan wire version 1", not as a confusing parse error;
3. rejects unknown fields anywhere (`deny_unknown_fields`), so a producer speaking another dialect
   is never half-understood;
4. validates the decoded plan (limits and slot consistency, see [execution.md](execution.md)).

`encode_json` is the symmetric, validating producer (Rust tools and tests). The Scala producer is
`io.adb.physical.PlanJsonEncoder`; `crates/adb-plan-wire/tests/fixtures/jvm-relational-plan.json`
pins its output, and tests on both sides check that fixture (cross-language contract).

## Plan nodes (`"op"`)

| `op` | Fields |
|---|---|
| `point_lookup` | `row_id` (decimal string of the u128 `(entity << 64) \| pk`; small JSON numbers accepted), `columns` |
| `scan` | `columns` (every entity; diagnostics) |
| `entity_scan` | `entity_id`, `columns` |
| `filter` | `input`, `predicate` |
| `project` | `input`, `slots` (output order) |
| `limit` | `input`, `limit` (≤ 1,000,000) |
| `hash_join` | `left` (probe), `right` (build), `join_type` (`inner` \| `left`), `keys` `[{"left":s,"right":s}]`, optional `residual` |
| `nested_loop_join` | `left`, `right`, `join_type`, optional `predicate` (absent = cross join) |
| `aggregate` | `input`, `group_by` (slots), `aggregates` `[{"function":"count\|sum\|min\|max\|avg","input":s?,"output":s}]` (`input` omitted only for `COUNT(*)`) |
| `sort` | `input`, `keys` `[{"slot":s,"descending":bool}]` |
| `top_k` | `input`, `keys`, `limit` (≤ 1,000,000) |

`columns` is `[{"field_id": f, "slot": s}]`: the stored field `f` is read into slot `s`.

## Expressions (`"kind"`)

| `kind` | Fields |
|---|---|
| `slot` | `slot` |
| `literal` | `value`: `"Null"`, `{"Bool":b}`, `{"Int64":n}`, `{"Float64":x}` (finite), `{"String":s}`, `{"Bytes":[..]}` |
| `binary` | `left`, `op` (`eq ne lt le gt ge and or`), `right` |
| `not` | `expr` |

## Example

`SELECT c.city, SUM(o.amount) AS total FROM orders o JOIN customer c ON o.customer_id = c.id
GROUP BY c.city ORDER BY total DESC LIMIT 2`:

```json
{"wire_version":2,"plan":{"op":"top_k","limit":2,
  "keys":[{"slot":4,"descending":true}],
  "input":{"op":"aggregate","group_by":[3],
    "aggregates":[{"function":"sum","input":1,"output":4}],
    "input":{"op":"hash_join","join_type":"inner","keys":[{"left":0,"right":2}],
      "left":{"op":"entity_scan","entity_id":2,"columns":[{"field_id":2,"slot":0},{"field_id":3,"slot":1}]},
      "right":{"op":"entity_scan","entity_id":1,"columns":[{"field_id":1,"slot":2},{"field_id":3,"slot":3}]}}}}}
```

## History

* v1 (Milestones 1.7–2.0.3): bare plan, expressions and projections by storage `field_id`,
  operators `point_lookup`, `scan`, `entity_scan`, `filter`, `project`, `limit`.
* v2 (2.1): envelope, slots, joins, aggregation, sorting, unknown fields rejected.

JSON remains a bootstrap protocol; `proto/` sketches the intended binary IR, which would decode
into the same Rust `PhysicalPlan`.
